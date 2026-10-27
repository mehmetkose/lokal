//! Parsers for what Linux shows about processes and sockets: the
//! `/proc/<pid>/stat` line, `ss` lines and `/etc/os-release`. Pure functions,
//! compiled on every OS so that `cargo test` on a Mac covers them too; only
//! `platform/linux/` calls them (VERIFY V9 and V11 on Linux, SPEC §12).

use std::collections::BTreeSet;

/// The fields lokal reads from `/proc/<pid>/stat`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ProcStat {
    pub pid: u32,
    /// `comm`, the kernel's name for the process: at most 15 bytes, and it
    /// may hold spaces and parentheses.
    pub comm: String,
    pub ppid: u32,
    /// Its process group (`pgrp`).
    pub pgid: u32,
}

/// Parses a `/proc/<pid>/stat` line: `412 (node) S 1 412 412 0 -1 …`
/// (proc(5): pid, comm, state, ppid, pgrp, …). The name runs from the first
/// `(` to the *last* `)`, because it may contain both, and the fields after
/// it are separated by spaces. `None` if it doesn't have that shape.
pub(super) fn parse_stat(line: &str) -> Option<ProcStat> {
    let open = line.find('(')?;
    let close = line.rfind(')')?;
    if close < open {
        return None;
    }
    let pid = line[..open].trim().parse().ok()?;
    let mut rest = line[close + 1..].split_whitespace();
    let _state = rest.next()?;
    let ppid = rest.next()?.parse().ok()?;
    let pgid = rest.next()?.parse().ok()?;
    Some(ProcStat {
        pid,
        comm: line[open + 1..close].to_string(),
        ppid,
        pgid,
    })
}

/// One TCP socket from an `ss -H -t -n -p` line:
/// `LISTEN 0 511 127.0.0.1:5173 0.0.0.0:* users:(("node",pid=412,fd=23))`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SsSocket {
    /// `LISTEN`, `ESTAB`, `UNCONN` (bound only), `TIME-WAIT`, …
    pub state: String,
    /// The local address's port.
    pub local_port: u16,
    /// It has no peer (`*:*`, `0.0.0.0:*`, `[::]:*`): it listens, or is only
    /// bound. Otherwise it's a connection.
    pub unconnected: bool,
    /// The processes that have it open, as `(pid, name)`, from `users:(…)`.
    /// Empty when ss shows none: another user's socket, which only root may
    /// see the owner of.
    pub users: Vec<(u32, String)>,
}

/// Parses one line of `ss -H -t -n -p` (`-a` or `-l`): state, receive and
/// send queues, local address, peer address, then `users:(…)` when ss knows
/// the processes. A leading `tcp` (the Netid column, which ss adds when it
/// lists more than one kind of socket) is skipped. The users part is found by
/// its text, not by columns, because a process name may contain spaces.
/// `None` if the line doesn't have that shape.
pub(super) fn parse_ss_line(line: &str) -> Option<SsSocket> {
    const USERS: &str = "users:(";
    let (columns, users) = match line.find(USERS) {
        Some(i) => (&line[..i], parse_users(&line[i + USERS.len()..])),
        None => (line, vec![]),
    };
    let mut it = columns.split_whitespace();
    let mut state = it.next()?;
    if state == "tcp" {
        state = it.next()?;
    }
    let (_recv_q, _send_q) = (it.next()?, it.next()?);
    let local = it.next()?;
    let peer = it.next()?;
    // Addresses end in `:<port>`: `127.0.0.1:5173`, `[::1]:5173`, `*:5173`,
    // `127.0.0.53%lo:53`. A peer's port is `*` when there is no peer.
    let local_port = local.rsplit_once(':')?.1.parse().ok()?;
    let unconnected = peer.rsplit_once(':')?.1 == "*";
    Some(SsSocket {
        state: state.to_string(),
        local_port,
        unconnected,
        users,
    })
}

/// `("node",pid=412,fd=23),("node",pid=413,fd=23))`, the part after
/// `users:(`, as `(pid, name)` pairs. Stops at the first entry that doesn't
/// have that shape.
fn parse_users(s: &str) -> Vec<(u32, String)> {
    const PID: &str = "\",pid=";
    let mut out = vec![];
    let mut rest = s;
    while let Some(entry) = rest.strip_prefix("(\"") {
        let Some(end) = entry.find(PID) else {
            break;
        };
        let name = &entry[..end];
        let after = &entry[end + PID.len()..];
        let digits = after
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(after.len());
        let Ok(pid) = after[..digits].parse() else {
            break;
        };
        out.push((pid, name.to_string()));
        // Skip `,fd=23)` and the `,` before the next entry.
        let Some(close) = after.find(')') else {
            break;
        };
        let next = &after[close + 1..];
        rest = next.strip_prefix(',').unwrap_or(next);
    }
    out
}

/// The processes in `ss -H -t -a -n -p 'sport = :<port>'` output that hold
/// `port`, as `(pid, name, listening)`, each once, in the order ss lists
/// them: those with an unconnected socket on it, listening or only bound.
/// `listening` if one of those listens. A connection doesn't count, whichever
/// end the port is (VERIFY V11). Sockets without `users:` (another user's)
/// name no process.
pub(super) fn ss_port_holders(ss: &str, port: u16) -> Vec<(u32, String, bool)> {
    let mut out: Vec<(u32, String, bool)> = vec![];
    for sock in ss.lines().filter_map(parse_ss_line) {
        let listening = sock.state == "LISTEN";
        if sock.local_port != port || !(listening || sock.unconnected) {
            continue;
        }
        for (pid, name) in sock.users {
            match out.iter_mut().find(|o| o.0 == pid) {
                Some(o) => o.2 |= listening,
                None => out.push((pid, name, listening)),
            }
        }
    }
    out
}

/// The local ports of the listening sockets in `ss -H -t -l -n -p` output
/// that a process in `pids` has open, sorted and without duplicates.
pub(super) fn ss_listen_ports(ss: &str, pids: &BTreeSet<u32>) -> Vec<u16> {
    let mut ports: Vec<u16> = ss
        .lines()
        .filter_map(parse_ss_line)
        .filter(|s| s.state == "LISTEN" && s.users.iter().any(|(pid, _)| pids.contains(pid)))
        .map(|s| s.local_port)
        .collect();
    ports.sort_unstable();
    ports.dedup();
    ports
}

/// `PRETTY_NAME` from an os-release file (`PRETTY_NAME="Ubuntu 24.04.1
/// LTS"`), without its quotes and with shell escapes (`\"`, `\\`, `\$`, `` \` ``)
/// undone. `None` if it's missing or empty.
pub(super) fn pretty_name(os_release: &str) -> Option<String> {
    let value = os_release
        .lines()
        .find_map(|l| l.trim().strip_prefix("PRETTY_NAME="))?
        .trim();
    let unquoted = ['"', '\'']
        .iter()
        .find_map(|q| value.strip_prefix(*q)?.strip_suffix(*q))
        .unwrap_or(value);
    let mut out = String::new();
    let mut chars = unquoted.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => out.extend(chars.next()),
            c => out.push(c),
        }
    }
    (!out.is_empty()).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_proc_stat() {
        let line =
            "412 (node) S 1 412 412 0 -1 4194560 2163 0 0 0 1 0 0 0 20 0 11 0 9000 1000000 4000";
        assert_eq!(
            parse_stat(line),
            Some(ProcStat {
                pid: 412,
                comm: "node".into(),
                ppid: 1,
                pgid: 412,
            })
        );
    }

    /// `comm` may hold spaces and parentheses, even `) S 1 2`: only the last
    /// `)` ends it.
    #[test]
    fn a_proc_stat_name_with_spaces_and_parens() {
        let line = "9001 (Web (Content) S 7 8) R 4242 9000 9000 0 -1 0\n";
        assert_eq!(
            parse_stat(line),
            Some(ProcStat {
                pid: 9001,
                comm: "Web (Content) S 7 8".into(),
                ppid: 4242,
                pgid: 9000,
            })
        );
        // The kernel's own threads have ppid 0 or 2 and pgid 0.
        assert_eq!(
            parse_stat("2 (kthreadd) S 0 0 0 0 -1").map(|s| (s.ppid, s.pgid)),
            Some((0, 0))
        );
        for bad in [
            "",
            "412 node S 1 412",
            "412 (node) S 1",
            "x (node) S 1 2",
            ") 1 (a",
        ] {
            assert_eq!(parse_stat(bad), None, "{bad:?}");
        }
    }

    /// ss lines for one port: a listener on 127.0.0.1 and ::1 with an accepted
    /// connection, the client of that connection, a socket that's only bound
    /// and another user's listener (no `users:`).
    const SS_PORT: &str = "\
LISTEN 0      128        127.0.0.1:41000      0.0.0.0:*    users:((\"node\",pid=412,fd=23))
LISTEN 0      511            [::1]:41000         [::]:*    users:((\"node\",pid=412,fd=24))
ESTAB  0      0          127.0.0.1:41000    127.0.0.1:52000 users:((\"node\",pid=412,fd=25))
ESTAB  0      0          127.0.0.1:52000    127.0.0.1:41000 users:((\"curl\",pid=500,fd=3))
UNCONN 0      0          127.0.0.1:41000      0.0.0.0:*    users:((\"holder\",pid=600,fd=7))
LISTEN 0      4096               *:41000            *:*
";

    #[test]
    fn parses_ss_lines() {
        let s = parse_ss_line(SS_PORT.lines().next().unwrap()).unwrap();
        assert_eq!(
            s,
            SsSocket {
                state: "LISTEN".into(),
                local_port: 41000,
                unconnected: true,
                users: vec![(412, "node".into())],
            }
        );
        let v6 = parse_ss_line(SS_PORT.lines().nth(1).unwrap()).unwrap();
        assert_eq!((v6.local_port, v6.unconnected), (41000, true));
        let estab = parse_ss_line(SS_PORT.lines().nth(2).unwrap()).unwrap();
        assert_eq!(
            (estab.state.as_str(), estab.local_port, estab.unconnected),
            ("ESTAB", 41000, false)
        );
        // No `users:`: the process isn't known.
        let other = parse_ss_line(SS_PORT.lines().nth(5).unwrap()).unwrap();
        assert_eq!((other.local_port, other.users.len()), (41000, 0));
        // The Netid column, an interface after the address, IPv4-mapped.
        let s = parse_ss_line(
            "tcp LISTEN 0 4096 127.0.0.53%lo:53 0.0.0.0:* users:((\"systemd-resolve\",pid=611,fd=15))",
        )
        .unwrap();
        assert_eq!(
            (s.local_port, s.users),
            (53, vec![(611, "systemd-resolve".into())])
        );
        let s =
            parse_ss_line("ESTAB 0 0 [::ffff:127.0.0.1]:8080 [::ffff:127.0.0.1]:41234").unwrap();
        assert_eq!((s.local_port, s.unconnected), (8080, false));
        for bad in ["", "LISTEN 0 128", "LISTEN 0 128 127.0.0.1:http 0.0.0.0:*"] {
            assert_eq!(parse_ss_line(bad), None, "{bad:?}");
        }
    }

    /// Several processes in one `users:` (a socket shared after fork), and a
    /// name with spaces, commas and parentheses.
    #[test]
    fn parses_every_process_of_a_socket() {
        let s = parse_ss_line(
            "LISTEN 0 511 *:3000 *:* users:((\"node\",pid=7,fd=20),(\"Web (Co), x\",pid=8,fd=20),(\"npm\",pid=6,fd=4))",
        )
        .unwrap();
        assert_eq!(
            s.users,
            [
                (7, "node".to_string()),
                (8, "Web (Co), x".to_string()),
                (6, "npm".to_string())
            ]
        );
        // A malformed entry ends the list; what came before stays.
        assert_eq!(
            parse_users("(\"a\",pid=1,fd=2),(\"b\",pid=x,fd=3))"),
            [(1, "a".to_string())]
        );
        assert!(parse_users(")").is_empty());
    }

    #[test]
    fn port_holders_are_the_unconnected_sockets_processes() {
        assert_eq!(
            ss_port_holders(SS_PORT, 41000),
            [
                (412, "node".to_string(), true),
                (600, "holder".to_string(), false)
            ]
        );
        // The client's local port: a connection holds no port.
        assert!(ss_port_holders(SS_PORT, 52000).is_empty());
        assert!(ss_port_holders("", 41000).is_empty());
        // Listening on one socket and only bound on another is listening.
        let both = "UNCONN 0 0 127.0.0.1:9 0.0.0.0:* users:((\"a\",pid=3,fd=4))\n\
LISTEN 0 1 [::1]:9 [::]:* users:((\"a\",pid=3,fd=5))\n";
        assert_eq!(ss_port_holders(both, 9), [(3, "a".to_string(), true)]);
    }

    #[test]
    fn listen_ports_of_a_set_of_pids() {
        let ss = "\
LISTEN 0 511 127.0.0.1:5173 0.0.0.0:* users:((\"node\",pid=101,fd=23))
LISTEN 0 511 [::1]:5173 [::]:* users:((\"node\",pid=101,fd=24))
LISTEN 0 511 *:24678 *:* users:((\"node\",pid=102,fd=30))
LISTEN 0 128 127.0.0.1:41000 0.0.0.0:* users:((\"lokal\",pid=60,fd=9))
LISTEN 0 128 0.0.0.0:22 0.0.0.0:*
ESTAB 0 0 127.0.0.1:6000 127.0.0.1:41000 users:((\"node\",pid=101,fd=31))
";
        assert_eq!(
            ss_listen_ports(ss, &BTreeSet::from([100, 101, 102])),
            [5173, 24678]
        );
        assert!(ss_listen_ports(ss, &BTreeSet::from([999])).is_empty());
        assert!(ss_listen_ports("", &BTreeSet::from([101])).is_empty());
    }

    #[test]
    fn reads_pretty_name() {
        let ubuntu = "PRETTY_NAME=\"Ubuntu 24.04.1 LTS\"\nNAME=\"Ubuntu\"\nVERSION_ID=\"24.04\"\n";
        assert_eq!(pretty_name(ubuntu).as_deref(), Some("Ubuntu 24.04.1 LTS"));
        assert_eq!(
            pretty_name("NAME=Arch\nPRETTY_NAME='Arch Linux'\n").as_deref(),
            Some("Arch Linux")
        );
        assert_eq!(
            pretty_name("PRETTY_NAME=Gentoo\n").as_deref(),
            Some("Gentoo")
        );
        assert_eq!(
            pretty_name("PRETTY_NAME=\"A \\\"quoted\\\" \\$name\"").as_deref(),
            Some("A \"quoted\" $name")
        );
        assert_eq!(pretty_name("NAME=\"Ubuntu\"\n"), None);
        assert_eq!(pretty_name("PRETTY_NAME=\"\"\n"), None);
    }
}

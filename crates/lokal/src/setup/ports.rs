//! Ports another program holds (SPEC §8.1, "A port another program holds").
//! When another program holds one of lokal's addresses as the service starts,
//! the service manager can't open it, and the daemon serves without it and
//! reports it (`listeners_missing`). This finds what's there now, and says
//! what it costs and how to fix it, for setup, start, restart, upgrade and
//! doctor. It also finds programs that listen on every address on lokal's
//! ports, which lokal hides on loopback (VERIFY V14, item 7).

use std::collections::BTreeMap;
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

use crate::ipc;
use crate::platform::{self, Platform, PortOwner};

/// How long a connect may take to show that something answers (as `lokal
/// run`'s busy check). A bind can't tell: macOS lets no user bind 127.0.0.1
/// or ::1 below port 1024 (VERIFY V14, item 10).
const CONNECT: Duration = Duration::from_millis(50);

/// lokal's HTTP port. Losing it costs only the `http://` redirect.
const HTTP_PORT: u16 = 80;

/// What holds a missing address now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Holder {
    /// Something answers a connect there: another program, named when lokal
    /// may see it (`nginx (pid 812)`), which needs it to be the user's own.
    Program(Option<String>),
    /// Nothing answers: it was taken when the service started, and is free
    /// now (or, on Linux, lokal may not use the port yet).
    Free,
    /// A UDP address, which a connect can't check.
    Unknown,
}

/// An address the service manager couldn't open for lokal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Blocked {
    /// `127.0.0.1:443`, `[::1]:80`, `127.0.0.1:53535/udp`.
    pub addr: String,
    pub port: u16,
    pub udp: bool,
    pub holder: Holder,
}

/// Looks at each address in `status.listeners_missing`.
pub fn find(p: &Platform, status: &ipc::Status) -> Vec<Blocked> {
    let mut names: BTreeMap<u16, Option<String>> = BTreeMap::new();
    let mut out = vec![];
    for a in &status.listeners_missing {
        let (text, udp) = match a.strip_suffix("/udp") {
            Some(t) => (t, true),
            None => (a.as_str(), false),
        };
        let Ok(addr) = text.parse::<SocketAddr>() else {
            continue;
        };
        let holder = if udp {
            Holder::Unknown
        } else if TcpStream::connect_timeout(&addr, CONNECT).is_ok() {
            let name = names.entry(addr.port()).or_insert_with(|| {
                platform::owner_text(&others(p.system.port_owners(addr.port()), status.pid))
            });
            Holder::Program(name.clone())
        } else {
            Holder::Free
        };
        out.push(Blocked {
            addr: a.clone(),
            port: addr.port(),
            udp,
            holder,
        });
    }
    out
}

/// The owners that aren't lokal's daemon, which holds the addresses it got.
fn others(owners: Vec<PortOwner>, lokal_pid: Option<u32>) -> Vec<PortOwner> {
    owners
        .into_iter()
        .filter(|o| Some(o.pid) != lokal_pid)
        .collect()
}

/// Are any of these missing addresses more than port 80? Then lokal can't
/// do its job: setup and doctor fail. Port 80 alone costs only the
/// `http://` redirect, a warning.
pub fn fatal(missing: &[String]) -> bool {
    missing.iter().any(|a| {
        a.ends_with("/udp")
            || a.parse::<SocketAddr>()
                .map_or(true, |s| s.port() != HTTP_PORT)
    })
}

/// What [`find`]'s addresses mean for the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    /// See [`fatal`].
    pub fatal: bool,
    /// `127.0.0.1:443 is taken by nginx (pid 812), so lokal serves HTTPS only
    /// on [::1]:443`.
    pub problem: String,
    /// `stop nginx (pid 812) or move it to another port, then run 'lokal
    /// restart'`.
    pub hint: String,
}

/// The report for `blocked`, or None if nothing is. `served` is what the
/// daemon listens on (`status.listeners`); `port_floor` says that this OS
/// doesn't let lokal use ports below 1024 yet (`ServiceManager::port_config`,
/// Linux); `lookup` gives the command that names another user's program on a
/// port ([`crate::platform::System::port_lookup`]).
pub fn report(
    blocked: &[Blocked],
    served: &[String],
    port_floor: bool,
    lookup: &dyn Fn(u16, bool) -> String,
) -> Option<Report> {
    if blocked.is_empty() {
        return None;
    }
    let missing: Vec<String> = blocked.iter().map(|b| b.addr.clone()).collect();
    let fatal = fatal(&missing);
    if port_floor {
        return Some(Report {
            fatal,
            problem: format!(
                "lokal couldn't open {}: {} doesn't let it use ports below 1024 yet",
                and_list(&missing),
                platform::THIS_MACHINE
            ),
            hint: "run 'lokal setup' (it will ask for your password)".into(),
        });
    }
    // One sentence per port and holder, in the daemon's order.
    let mut groups: Vec<(&Blocked, Vec<String>)> = vec![];
    for b in blocked {
        match groups
            .iter_mut()
            .find(|(g, _)| g.port == b.port && g.udp == b.udp && g.holder == b.holder)
        {
            Some((_, addrs)) => addrs.push(b.addr.clone()),
            None => groups.push((b, vec![b.addr.clone()])),
        }
    }
    let mut sentences = vec![];
    for (b, addrs) in &groups {
        let one = addrs.len() == 1;
        let (is, was) = if one { ("is", "was") } else { ("are", "were") };
        let list = and_list(addrs);
        let what = match &b.holder {
            Holder::Program(Some(name)) => format!("{list} {is} taken by {name}"),
            Holder::Program(None) | Holder::Unknown => {
                format!("{list} {is} taken by another program")
            }
            Holder::Free => format!(
                "{list} {was} taken when lokal's background service started, and {} free now",
                if one { "is" } else { "are" }
            ),
        };
        sentences.push(format!("{what}, so {}", cost(b, served)));
    }
    // The fix: stop what holds them, or just restart if nothing does now.
    let takers: Vec<&Blocked> = groups
        .iter()
        .map(|(b, _)| *b)
        .filter(|b| b.holder != Holder::Free)
        .collect();
    let mut names: Vec<String> = vec![];
    for b in &takers {
        let n = match &b.holder {
            Holder::Program(Some(n)) => n.clone(),
            _ => "that program".into(),
        };
        if !names.contains(&n) {
            names.push(n);
        }
    }
    let hint = match names.as_slice() {
        [] => format!(
            "run 'lokal restart' so lokal takes {}",
            if missing.len() == 1 { "it" } else { "them" }
        ),
        [one] => format!("stop {one} or move it to another port, then run 'lokal restart'"),
        _ => "stop those programs or move them to other ports, then run 'lokal restart'".into(),
    };
    // Name the command that finds a program lokal can't see (another user's).
    let unnamed = takers
        .iter()
        .find(|b| !matches!(b.holder, Holder::Program(Some(_))));
    let hint = match unnamed {
        Some(b) => format!(
            "{hint}. To see what holds port {}: {}",
            b.port,
            lookup(b.port, b.udp)
        ),
        None => hint,
    };
    Some(Report {
        fatal,
        problem: sentences.join("; "),
        hint,
    })
}

/// What a missing address costs, after "so".
fn cost(b: &Blocked, served: &[String]) -> String {
    if b.udp {
        return ".lokal names don't resolve".into();
    }
    if b.port == HTTP_PORT {
        return "lokal doesn't redirect http:// to https there".into();
    }
    let same_port: Vec<String> = served
        .iter()
        .filter(|s| {
            !s.ends_with("/udp") && s.parse::<SocketAddr>().is_ok_and(|a| a.port() == b.port)
        })
        .cloned()
        .collect();
    if same_port.is_empty() {
        "lokal serves no HTTPS".into()
    } else {
        format!("lokal serves HTTPS only on {}", and_list(&same_port))
    }
}

/// Other programs that listen on every address on `port` (`*:443`), which
/// macOS lets them do when they were there first (VERIFY V14, item 7): on
/// 127.0.0.1 and ::1, lokal answers instead. A warning for doctor, or None.
pub fn shadowed(owners: Vec<PortOwner>, lokal_pid: Option<u32>, port: u16) -> Option<String> {
    let wide: Vec<PortOwner> = others(owners, lokal_pid)
        .into_iter()
        .filter(|o| o.everywhere)
        .collect();
    let names = platform::owner_text(&wide)?;
    let url = if port == HTTP_PORT {
        "http://localhost".to_string()
    } else if port == 443 {
        "https://localhost".to_string()
    } else {
        format!("localhost:{port}")
    };
    let fix = if wide.len() == 1 {
        "If you use that program on"
    } else {
        "If you use those programs on"
    };
    let (verb, port_word) = if wide.len() == 1 {
        ("listens", "give it another port")
    } else {
        ("listen", "give them other ports")
    };
    Some(format!(
        "{names} also {verb} on port {port}, on every address: lokal answers on 127.0.0.1 and ::1, \
         so {url} reaches lokal instead. {fix} {}, {port_word}",
        platform::THIS_MACHINE
    ))
}

/// `a`, `a and b`, `a, b and c`.
fn and_list(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(addr: &str, holder: Holder) -> Blocked {
        let udp = addr.ends_with("/udp");
        let port = addr
            .trim_end_matches("/udp")
            .parse::<SocketAddr>()
            .unwrap()
            .port();
        Blocked {
            addr: addr.into(),
            port,
            udp,
            holder,
        }
    }

    fn lookup(port: u16, udp: bool) -> String {
        format!("look {port}{}", if udp { "/udp" } else { "" })
    }

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    const SERVED: &[&str] = &["[::1]:443", "[::1]:80", "127.0.0.1:53535/udp"];

    #[test]
    fn a_program_lokal_can_name_on_one_https_address() {
        let r = report(
            &[b(
                "127.0.0.1:443",
                Holder::Program(Some("nginx (pid 812)".into())),
            )],
            &s(SERVED),
            false,
            &lookup,
        )
        .unwrap();
        assert!(r.fatal);
        assert_eq!(
            r.problem,
            "127.0.0.1:443 is taken by nginx (pid 812), so lokal serves HTTPS only on [::1]:443"
        );
        assert_eq!(
            r.hint,
            "stop nginx (pid 812) or move it to another port, then run 'lokal restart'"
        );
    }

    /// Another user's program (a web server running as root) on 443 and 80:
    /// one sentence per port, and the command that names it.
    #[test]
    fn another_users_program_on_both_ports() {
        let r = report(
            &[
                b("127.0.0.1:443", Holder::Program(None)),
                b("[::1]:443", Holder::Program(None)),
                b("127.0.0.1:80", Holder::Program(None)),
                b("[::1]:80", Holder::Program(None)),
            ],
            &s(&["127.0.0.1:53535/udp"]),
            false,
            &lookup,
        )
        .unwrap();
        assert!(r.fatal);
        assert_eq!(
            r.problem,
            "127.0.0.1:443 and [::1]:443 are taken by another program, so lokal serves no HTTPS; \
             127.0.0.1:80 and [::1]:80 are taken by another program, so lokal doesn't redirect http:// to https there"
        );
        assert_eq!(
            r.hint,
            "stop that program or move it to another port, then run 'lokal restart'. \
             To see what holds port 443: look 443"
        );
    }

    /// Port 80 alone is a warning: HTTPS works.
    #[test]
    fn port_80_alone_is_not_fatal() {
        let missing = s(&["127.0.0.1:80", "[::1]:80"]);
        assert!(!fatal(&missing));
        let r = report(
            &[
                b(
                    "127.0.0.1:80",
                    Holder::Program(Some("httpd (pid 90)".into())),
                ),
                b("[::1]:80", Holder::Program(Some("httpd (pid 90)".into()))),
            ],
            &s(&["127.0.0.1:443", "[::1]:443"]),
            false,
            &lookup,
        )
        .unwrap();
        assert!(!r.fatal);
        assert_eq!(
            r.problem,
            "127.0.0.1:80 and [::1]:80 are taken by httpd (pid 90), so lokal doesn't redirect http:// to https there"
        );
        assert!(fatal(&s(&["127.0.0.1:443"])));
        assert!(fatal(&s(&["127.0.0.1:53535/udp"])));
        assert!(!fatal(&[]));
    }

    #[test]
    fn free_now_needs_only_a_restart() {
        let r = report(
            &[b("127.0.0.1:443", Holder::Free)],
            &s(SERVED),
            false,
            &lookup,
        )
        .unwrap();
        assert_eq!(
            r.problem,
            "127.0.0.1:443 was taken when lokal's background service started, and is free now, \
             so lokal serves HTTPS only on [::1]:443"
        );
        assert_eq!(r.hint, "run 'lokal restart' so lokal takes it");
    }

    #[test]
    fn dns_and_two_programs() {
        let r = report(
            &[
                b(
                    "127.0.0.1:443",
                    Holder::Program(Some("caddy (pid 7)".into())),
                ),
                b("127.0.0.1:53535/udp", Holder::Unknown),
            ],
            &s(&["[::1]:443"]),
            false,
            &lookup,
        )
        .unwrap();
        assert_eq!(
            r.problem,
            "127.0.0.1:443 is taken by caddy (pid 7), so lokal serves HTTPS only on [::1]:443; \
             127.0.0.1:53535/udp is taken by another program, so .lokal names don't resolve"
        );
        assert_eq!(
            r.hint,
            "stop those programs or move them to other ports, then run 'lokal restart'. \
             To see what holds port 53535: look 53535/udp"
        );
    }

    /// On Linux, ports below 1024 need a setting that setup makes.
    #[test]
    fn a_port_floor_sends_to_setup() {
        let r = report(
            &[
                b("127.0.0.1:443", Holder::Free),
                b("[::1]:443", Holder::Free),
            ],
            &[],
            true,
            &lookup,
        )
        .unwrap();
        assert_eq!(
            r.problem,
            format!(
                "lokal couldn't open 127.0.0.1:443 and [::1]:443: {} doesn't let it use ports below 1024 yet",
                platform::THIS_MACHINE
            )
        );
        assert_eq!(r.hint, "run 'lokal setup' (it will ask for your password)");
        assert_eq!(report(&[], &[], true, &lookup), None);
    }

    fn owner(pid: u32, name: &str, everywhere: bool) -> PortOwner {
        PortOwner {
            pid,
            pgid: pid,
            name: name.into(),
            listening: true,
            everywhere,
        }
    }

    /// A program on `*:443` (Docker, Caddy) next to lokal's loopback
    /// listeners; lokal's own listeners don't count, nor one on one address.
    #[test]
    fn names_a_program_that_listens_everywhere() {
        let owners = vec![
            owner(100, "lokal", false),
            owner(200, "com.docker.backend", true),
        ];
        assert_eq!(
            shadowed(owners, Some(100), 443).unwrap(),
            format!(
                "com.docke (pid 200) also listens on port 443, on every address: lokal answers on \
                 127.0.0.1 and ::1, so https://localhost reaches lokal instead. If you use that \
                 program on {}, give it another port",
                platform::THIS_MACHINE
            )
        );
        assert_eq!(
            shadowed(vec![owner(100, "lokal", true)], Some(100), 443),
            None
        );
        assert_eq!(shadowed(vec![owner(5, "x", false)], Some(100), 80), None);
        let note = shadowed(vec![owner(5, "httpd", true)], None, 80).unwrap();
        assert!(note.contains("so http://localhost reaches lokal"), "{note}");
    }

    #[test]
    fn lists_with_and() {
        assert_eq!(and_list(&s(&["a"])), "a");
        assert_eq!(and_list(&s(&["a", "b"])), "a and b");
        assert_eq!(and_list(&s(&["a", "b", "c"])), "a, b and c");
    }
}

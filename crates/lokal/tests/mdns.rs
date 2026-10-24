//! SPEC §15 M3: with `--name-backend mdns-local-only`, `getaddrinfo` sees
//! random `lt-<rand>.local` names as soon as `reload` returns, and they are
//! gone as soon as removal returns. macOS only (mDNSResponder); LocalOnly
//! names never leave this Mac.

mod common;

use common::Daemon;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

fn has_both(host: &str) -> (bool, f64) {
    let (r, d) = testkit::probe::resolve(host);
    let ok = r
        .map(|a| {
            a.contains(&IpAddr::V4(Ipv4Addr::LOCALHOST))
                && a.contains(&IpAddr::V6(Ipv6Addr::LOCALHOST))
        })
        .unwrap_or(false);
    (ok, d.as_secs_f64() * 1000.0)
}

#[test]
fn names_resolve_on_return_and_vanish_on_removal() {
    if !cfg!(target_os = "macos") {
        eprintln!("skipped: mDNSResponder backend is macOS-only");
        return;
    }
    let d = Daemon::start("mdns-local-only");
    let p = format!("lt-{:06x}", std::process::id() ^ 0x5a5a5a);
    d.ok(&["project", "create", &p, "--domain", &format!("{p}.local")]);
    d.ok(&["service", "add", "api", "4999"]);
    d.ok(&["service", "add", "v1.api", "4998"]);
    d.ok(&["service", "add", "@", "4997"]);
    let names = [
        format!("api.{p}.local"),
        format!("v1.api.{p}.local"),
        format!("{p}.local"),
    ];
    for n in &names {
        let (ok, ms) = has_both(n);
        assert!(ok, "{n} not resolvable right after `service add` returned");
        assert!(ms < 100.0, "{n} took {ms}ms");
    }
    let st = d.ok(&["status", "--json"]);
    assert!(
        st.contains("\"name_backend\": \"mdns-local-only+dns\""),
        "{st}"
    );
    assert_eq!(st.matches("\"registered\"").count(), 3, "{st}");

    d.ok(&["project", "remove", &p, "--force"]);
    // Negative .local lookups wait for the multicast timeout (~5s); check in parallel.
    let handles: Vec<_> = names
        .iter()
        .cloned()
        .map(|n| std::thread::spawn(move || (n.clone(), has_both(&n).0)))
        .collect();
    for h in handles {
        let (n, still) = h.join().unwrap();
        assert!(!still, "{n} still resolves after removal returned");
    }
}

/// `.lokal` names are answered by the built-in DNS server (sandbox port,
/// queried directly) with TTL 0. Every name under `.lokal` answers loopback
/// (no negative answers, which mDNSResponder would cache for ~75s); whether a
/// name is *served* is decided by routes and certificates.
#[test]
fn lokal_names_answered_by_dns_server_on_return() {
    let d = Daemon::start("none");
    let q = |name: &str, qtype: u16| -> (u8, Vec<Vec<u8>>, u32) {
        let sock = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        sock.set_read_timeout(Some(std::time::Duration::from_secs(2)))
            .unwrap();
        sock.send_to(
            &lokal::dns::wire::query(42, name, qtype),
            ("127.0.0.1", d.dns),
        )
        .unwrap();
        let mut buf = [0u8; 1500];
        let n = sock.recv(&mut buf).unwrap();
        let r = &buf[..n];
        let rcode = r[3] & 0x0F;
        let an = u16::from_be_bytes([r[6], r[7]]);
        let qe = lokal::dns::wire::parse_question(r).unwrap().end;
        let mut i = qe;
        let mut rdatas = vec![];
        let mut ttl = u32::MAX;
        for _ in 0..an {
            ttl = u32::from_be_bytes(r[i + 6..i + 10].try_into().unwrap());
            let len = u16::from_be_bytes([r[i + 10], r[i + 11]]) as usize;
            rdatas.push(r[i + 12..i + 12 + len].to_vec());
            i += 12 + len;
        }
        (rcode, rdatas, ttl)
    };
    d.ok(&["project", "create", "dnsp"]);
    d.ok(&["service", "add", "api", "4999"]);
    let (rc, a, ttl) = q("api.dnsp.lokal", 1);
    assert_eq!((rc, a, ttl), (0, vec![vec![127, 0, 0, 1]], 0));
    let (rc, aaaa, _) = q("API.dnsp.lokal", 28);
    let mut six = vec![0u8; 16];
    six[15] = 1;
    assert_eq!((rc, aaaa), (0, vec![six]));
    assert_eq!(
        q("nope.dnsp.lokal", 1),
        (0, vec![vec![127, 0, 0, 1]], 0),
        "unconfigured names answer loopback too"
    );
    // The status lists the name as registered by the dns backend.
    let st = d.ok(&["status", "--json"]);
    assert!(st.contains("api.dnsp.lokal") && st.contains("+dns"), "{st}");
    d.ok(&["service", "remove", "api"]);
    let st = d.ok(&["status", "--json"]);
    assert!(
        !st.contains("api.dnsp.lokal"),
        "removed name still published: {st}"
    );
}

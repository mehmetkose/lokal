//! Engine tests (SPEC §15 M3) against a sandbox daemon:
//! `lokal daemon run --foreground --https-port <p> --http-port <q>`
//! with a `LOKAL_HOME` sandbox CA, probed with `--connect`/`--cacert` semantics.

mod common;

use std::net::{IpAddr, SocketAddr};
use std::time::{Duration, Instant};

use bytes::Bytes;
use common::{Daemon, V4, V6, free_port};
use http_body_util::{BodyExt, StreamBody};
use hyper::body::Frame;
use testkit::probe::{self, Opts};

async fn echo(ip: IpAddr, marker: &str) -> u16 {
    testkit::echo::spawn(ip, 0, marker).await.unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn h1_h2_forwarded_headers_and_host_modes() {
    let d = Daemon::start();
    let a = echo(V4, "AAA").await;
    d.ok(&["project", "create", "p"]);
    let out = d.ok(&["service", "add", "api", &a.to_string()]);
    assert!(out.contains("ready in"), "{out}");
    d.ok(&[
        "service",
        "add",
        "up",
        &a.to_string(),
        "--host-header",
        "upstream",
    ]);

    let r = probe::get("https://api.p.lokal/hello?x=1", &d.opts())
        .await
        .unwrap();
    assert_eq!(r.status, 200);
    assert_eq!(r.version, "HTTP/1.1");
    let j = r.json().unwrap();
    assert_eq!(j["marker"], "AAA");
    assert_eq!(j["path"], "/hello");
    assert_eq!(j["headers"]["host"], "api.p.lokal");
    assert_eq!(j["headers"]["x-forwarded-proto"], "https");
    assert_eq!(j["headers"]["x-forwarded-host"], "api.p.lokal");
    assert_eq!(j["headers"]["x-forwarded-port"], "443");
    assert_eq!(j["headers"]["x-forwarded-for"], "127.0.0.1");
    assert!(
        r.header("strict-transport-security").is_none(),
        "never HSTS"
    );

    let r = probe::request(
        "https://api.p.lokal/",
        &d.opts(),
        "GET",
        &[("x-forwarded-for", "10.1.2.3")],
        probe::empty_body(),
    )
    .await
    .unwrap();
    assert_eq!(
        r.json().unwrap()["headers"]["x-forwarded-for"],
        "10.1.2.3, 127.0.0.1"
    );

    let r = probe::get("https://api.p.lokal/h2", &d.opts_h2())
        .await
        .unwrap();
    assert_eq!(r.status, 200);
    assert_eq!(r.version, "HTTP/2.0");
    let j = r.json().unwrap();
    assert_eq!(j["marker"], "AAA");
    assert_eq!(
        j["headers"]["host"], "api.p.lokal",
        "h2 :authority becomes Host upstream"
    );

    let r = probe::get("https://up.p.lokal/", &d.opts()).await.unwrap();
    let j = r.json().unwrap();
    assert_eq!(j["headers"]["host"], format!("localhost:{a}"));
    assert_eq!(j["headers"]["x-forwarded-host"], "up.p.lokal");
}

#[tokio::test(flavor = "multi_thread")]
async fn sse_events_arrive_promptly() {
    let d = Daemon::start();
    let a = echo(V4, "AAA").await;
    d.ok(&["project", "create", "p"]);
    d.ok(&["service", "add", "api", &a.to_string()]);
    let lags = probe::sse("https://api.p.lokal/sse", &d.opts(), 3)
        .await
        .unwrap();
    assert_eq!(lags.len(), 3);
    assert!(lags.iter().all(|l| *l <= 300), "{lags:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn websocket_echo() {
    let d = Daemon::start();
    let a = echo(V4, "AAA").await;
    d.ok(&["project", "create", "p"]);
    d.ok(&["service", "add", "api", &a.to_string()]);
    let reply = probe::ws_echo("wss://api.p.lokal/ws", &d.opts())
        .await
        .unwrap();
    assert!(reply.starts_with("AAA:hello-"), "{reply}");
}

#[tokio::test(flavor = "multi_thread")]
async fn streamed_50mb_post() {
    let d = Daemon::start();
    let a = echo(V4, "AAA").await;
    d.ok(&["project", "create", "p"]);
    d.ok(&["service", "add", "api", &a.to_string()]);
    let chunk = Bytes::from(vec![b'x'; 1 << 20]);
    let stream = futures_util::stream::iter(
        (0..50).map(move |_| Ok::<_, std::io::Error>(Frame::data(chunk.clone()))),
    );
    let body = StreamBody::new(stream).boxed();
    for opts in [d.opts(), d.opts_h2()] {
        let chunk = Bytes::from(vec![b'x'; 1 << 20]);
        let s = futures_util::stream::iter(
            (0..50).map(move |_| Ok::<_, std::io::Error>(Frame::data(chunk.clone()))),
        );
        let r = probe::request(
            "https://api.p.lokal/upload",
            &opts,
            "POST",
            &[],
            StreamBody::new(s).boxed(),
        )
        .await
        .unwrap();
        assert_eq!(r.status, 200);
        assert_eq!(r.json().unwrap()["received_bytes"], 50 * 1024 * 1024);
    }
    drop(body);
}

#[tokio::test(flavor = "multi_thread")]
async fn upstream_down_is_502_naming_the_upstream() {
    let d = Daemon::start();
    let closed = free_port();
    d.ok(&["project", "create", "p"]);
    let out = d.ok(&["service", "add", "down", &closed.to_string()]);
    assert!(
        out.contains(&format!(
            "note: nothing is listening on localhost:{closed} yet"
        )),
        "{out}"
    );
    let r = probe::get("https://down.p.lokal/", &d.opts())
        .await
        .unwrap();
    assert_eq!(r.status, 502);
    let body = r.text();
    assert!(
        body.contains(&format!(
            "down.p.lokal → localhost:{closed} is not responding. Is the service running?"
        )),
        "{body}"
    );
    let r = probe::request(
        "https://down.p.lokal/",
        &d.opts(),
        "GET",
        &[("accept", "application/json")],
        probe::empty_body(),
    )
    .await
    .unwrap();
    assert_eq!(r.status, 502);
    assert_eq!(r.json().unwrap()["status"], 502);
}

#[tokio::test(flavor = "multi_thread")]
async fn unknown_host_404_and_unknown_or_missing_sni_fails() {
    let d = Daemon::start();
    let a = echo(V4, "AAA").await;
    d.ok(&["project", "create", "p"]);
    d.ok(&["service", "add", "api", &a.to_string()]);
    // Unknown SNI: handshake fails.
    let e = probe::get("https://nope.p.lokal/", &d.opts())
        .await
        .unwrap_err();
    assert!(format!("{e:#}").contains("TLS handshake"), "{e:#}");
    // No SNI (IP address as server name): handshake fails.
    let cfg = probe::client_config(Some(&d.cacert()), &[]).unwrap();
    let t = probe::Target {
        tls: true,
        host: "127.0.0.1".into(),
        port: d.https,
        path: "/".into(),
    };
    assert!(probe::open(&t, None, &cfg).await.is_err());
    // Known SNI, unknown Host → 421 (misdirected), see below; plain HTTP unknown host → 404 page listing URLs.
    let r = probe::get(
        &format!("http://nope.p.lokal:{}/", d.http),
        &Opts {
            connect: Some(SocketAddr::new(V4, d.http)),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(r.status, 404);
    assert!(r.text().contains("https://api.p.lokal"), "{}", r.text());
}

#[tokio::test(flavor = "multi_thread")]
async fn http_redirects_with_308() {
    let d = Daemon::start();
    let a = echo(V4, "AAA").await;
    d.ok(&["project", "create", "p"]);
    d.ok(&["service", "add", "api", &a.to_string()]);
    let o = Opts {
        connect: Some(SocketAddr::new(V4, d.http)),
        ..Default::default()
    };
    let r = probe::request(
        "http://api.p.lokal/x?y=1",
        &o,
        "GET",
        &[("host", "api.p.lokal")],
        probe::empty_body(),
    )
    .await
    .unwrap();
    assert_eq!(r.status, 308);
    assert_eq!(r.header("location"), Some("https://api.p.lokal/x?y=1"));
}

#[tokio::test(flavor = "multi_thread")]
async fn host_sni_mismatch_is_421() {
    let d = Daemon::start();
    let a = echo(V4, "AAA").await;
    d.ok(&["project", "create", "p"]);
    d.ok(&["service", "add", "api", &a.to_string()]);
    d.ok(&["service", "add", "web", &a.to_string()]);
    let r = probe::request(
        "https://api.p.lokal/",
        &d.opts(),
        "GET",
        &[("host", "web.p.lokal")],
        probe::empty_body(),
    )
    .await
    .unwrap();
    assert_eq!(r.status, 421);
}

#[tokio::test(flavor = "multi_thread")]
async fn localhost_upstreams_on_v6_only_and_v4_only() {
    let d = Daemon::start();
    let v6 = echo(V6, "SIX").await;
    let v4 = echo(V4, "FOUR").await;
    d.ok(&["project", "create", "p"]);
    d.ok(&["service", "add", "six", &v6.to_string()]);
    d.ok(&["service", "add", "four", &v4.to_string()]);
    for _ in 0..3 {
        let r = probe::get("https://six.p.lokal/", &d.opts()).await.unwrap();
        assert_eq!(r.json().unwrap()["marker"], "SIX");
        let r = probe::get("https://four.p.lokal/", &d.opts())
            .await
            .unwrap();
        assert_eq!(r.json().unwrap()["marker"], "FOUR");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn reload_is_ready_on_return_and_in_flight_requests_survive() {
    let d = Daemon::start();
    let a = echo(V4, "AAA").await;
    let b = echo(V4, "BBB").await;
    d.ok(&["project", "create", "p"]);
    d.ok(&["service", "add", "api", &a.to_string()]);
    let opts = d.opts();
    let slow =
        tokio::spawn(async move { probe::get("https://api.p.lokal/slow?ms=700", &opts).await });
    tokio::time::sleep(Duration::from_millis(100)).await;
    // Hot add: usable on the first try.
    let out = d.ok(&["service", "add", "new", &b.to_string()]);
    assert!(out.contains("ready in"), "{out}");
    let r = probe::get("https://new.p.lokal/", &d.opts()).await.unwrap();
    assert_eq!(r.json().unwrap()["marker"], "BBB");
    // Re-adding the identical service is a no-op that still reports ready.
    let out = d.ok(&["service", "add", "new", &b.to_string()]);
    assert!(out.contains("already configured, ready in"), "{out}");
    let r = probe::get("https://new.p.lokal/", &d.opts()).await.unwrap();
    assert_eq!(r.json().unwrap()["marker"], "BBB");
    // Hot update.
    d.ok(&["service", "update", "api", "--port", &b.to_string()]);
    let r = probe::get("https://api.p.lokal/", &d.opts()).await.unwrap();
    assert_eq!(r.json().unwrap()["marker"], "BBB");
    // Hot remove: handshake for the removed name fails at once.
    d.ok(&["service", "remove", "new"]);
    assert!(probe::get("https://new.p.lokal/", &d.opts()).await.is_err());
    // Several names in one command: one reload waits for all of them, so
    // every handshake fails as soon as it returns.
    for sub in ["one", "two"] {
        d.ok(&["service", "add", sub, &b.to_string()]);
    }
    let out = d.ok(&["service", "remove", "one", "two"]);
    assert_eq!(out.matches("✓ Removed").count(), 2, "{out}");
    assert!(out.contains("two.p.lokal  (done in "), "{out}");
    for url in ["https://one.p.lokal/", "https://two.p.lokal/"] {
        assert!(probe::get(url, &d.opts()).await.is_err(), "{url}");
    }
    // The request that was in flight during the reloads completed.
    let r = slow.await.unwrap().unwrap();
    assert_eq!(r.status, 200);
    assert_eq!(r.json().unwrap()["marker"], "AAA");
}

#[tokio::test(flavor = "multi_thread")]
async fn tls_sessions_resume_and_alpn() {
    let d = Daemon::start();
    let a = echo(V4, "AAA").await;
    d.ok(&["project", "create", "p"]);
    d.ok(&["service", "add", "api", &a.to_string()]);
    let (times, alpn) = probe::tls_handshakes("api.p.lokal", d.https, 10, &d.opts())
        .await
        .unwrap();
    assert_eq!(alpn, "h2");
    assert_eq!(times.len(), 10);
}

#[tokio::test(flavor = "multi_thread")]
async fn hand_edited_state_is_picked_up() {
    let d = Daemon::start();
    let a = echo(V4, "AAA").await;
    d.ok(&["project", "create", "p"]);
    let state = d.dir.path().join("state.toml");
    let mut text = std::fs::read_to_string(&state).unwrap();
    text.push_str(&format!(
        "\n[projects.p.services.hand]\nhost = \"localhost\"\nport = {a}\n"
    ));
    std::fs::write(&state, text).unwrap();
    // The watcher is a backup to IPC reload; give FSEvents a moment.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(r) = probe::get("https://hand.p.lokal/", &d.opts()).await {
            assert_eq!(r.json().unwrap()["marker"], "AAA");
            break;
        }
        assert!(Instant::now() < deadline, "watcher did not reload");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// A `state.toml` from lokal 0.5 or earlier with a project on `.local`
/// still loads: the `.lokal` project is served, the `.local` one gets no
/// route, certificate or name, `status`, `project list` and `doctor` say
/// how to move it, nothing can be added to it, and it can be removed.
#[tokio::test(flavor = "multi_thread")]
async fn a_local_project_from_0_5_loads_but_is_not_served() {
    let d = Daemon::start();
    let a = echo(V4, "AAA").await;
    std::fs::write(
        d.dir.path().join("state.toml"),
        format!(
            "version = 1\n\
[projects.old]\ndomain = \"old.local\"\ncreated_at = \"\"\n\
[projects.old.services.api]\nhost = \"localhost\"\nport = {a}\n\
[projects.old.services.\"@\"]\nhost = \"localhost\"\nport = {a}\n"
        ),
    )
    .unwrap();
    d.ok(&["project", "create", "p"]);
    let out = d.ok(&["service", "add", "api", &a.to_string(), "-p", "p"]);
    assert!(out.contains("ready in"), "{out}");

    let r = probe::get("https://api.p.lokal/", &d.opts()).await.unwrap();
    assert_eq!(r.json().unwrap()["marker"], "AAA");
    for url in ["https://api.old.local/", "https://old.local/"] {
        assert!(probe::get(url, &d.opts()).await.is_err(), "{url}");
    }

    let st: serde_json::Value = serde_json::from_str(&d.ok(&["status", "--json"])).unwrap();
    assert_eq!(st["name_backend"], "dns");
    let names: Vec<&str> = st["names"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["fqdn"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["api.p.lokal"]);
    let fix = "move it to .lokal: lokal project remove old --force, then lokal project create old";
    let out = d.ok(&["status"]);
    assert!(out.contains("names:     1/1 registered (dns)"), "{out}");
    assert!(
        out.contains("warning: old.local isn't served: lokal serves only .lokal names since 0.6.0"),
        "{out}"
    );
    assert!(out.contains(fix), "{out}");
    let out = d.ok(&["project", "list"]);
    assert!(
        out.contains("old  old.local  2 service(s)  (not served)"),
        "{out}"
    );
    assert!(out.contains(fix), "{out}");

    // Doctor fails here anyway (nothing trusts the sandbox CA): only its
    // checks matter.
    let o = d.lokal(&["doctor", "--json"]);
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    let checks = v["checks"].as_array().unwrap();
    let check = |id: &str| checks.iter().find(|c| c["id"] == id);
    let c = check("project.old").expect("project.old");
    assert_eq!(c["status"], "warn");
    let detail = c["detail"].as_str().unwrap();
    assert!(
        detail.starts_with("old.local isn't served: lokal serves only .lokal names since 0.6.0; "),
        "{detail}"
    );
    assert!(detail.contains(fix), "{detail}");
    assert!(check("project.p").is_none());
    assert!(check("tls.api.p.lokal").is_some());
    assert!(check("tls.api.old.local").is_none());

    // Nothing can be added to it or changed in it.
    for args in [
        &["service", "add", "web", &a.to_string(), "-p", "old"][..],
        &["service", "update", "api", "--port", "4001", "-p", "old"][..],
    ] {
        let o = d.lokal(args);
        assert_eq!(o.status.code(), Some(1), "{args:?}");
        let err = String::from_utf8_lossy(&o.stderr);
        assert!(
            err.contains(
                "project `old` is on old.local, and lokal serves only .lokal names since 0.6.0"
            ),
            "{err}"
        );
        assert!(err.contains(fix), "{err}");
    }

    // Removing it works.
    let out = d.ok(&["service", "remove", "api", "-p", "old"]);
    assert!(out.contains("Removed api.old.local"), "{out}");
    let out = d.ok(&["project", "remove", "old", "--force"]);
    assert!(out.contains("Removed old.local and 1 service(s)"), "{out}");
    assert!(!d.ok(&["status"]).contains("isn't served"));
    let r = probe::get("https://api.p.lokal/", &d.opts()).await.unwrap();
    assert_eq!(r.json().unwrap()["marker"], "AAA");
}

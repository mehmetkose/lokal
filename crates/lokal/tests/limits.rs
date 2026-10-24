//! Connection limits (SPEC §9.3) against a sandbox daemon. Before them, idle
//! connections from any local account were never reclaimed: hyper-util's
//! protocol sniffing had no deadline, and neither did a quiet connection
//! after the TLS handshake or an h2 connection that opened no stream. Nothing
//! capped them either, so enough of them took every file descriptor and
//! blocked every lokal URL and the CLI's IPC.

mod common;

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use common::{Daemon, V4};
use futures_util::{SinkExt, StreamExt};
use testkit::probe;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;

const H2_PREFACE: &[u8] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";
/// An empty SETTINGS frame.
const H2_SETTINGS: &[u8] = &[0, 0, 0, 4, 0, 0, 0, 0, 0];

/// A sandbox daemon with `api.p.lokal` → a local echo server.
async fn daemon(extra: &[&str], nofile: Option<u64>) -> Daemon {
    let d = Daemon::start_with("none", extra, nofile);
    let port = testkit::echo::spawn(V4, 0, "AAA").await.unwrap();
    d.ok(&["project", "create", "p"]);
    d.ok(&["service", "add", "api", &port.to_string()]);
    d
}

/// A TLS connection for api.p.lokal offering `alpn` (nothing if empty).
async fn tls(d: &Daemon, alpn: &[&[u8]]) -> tokio_rustls::client::TlsStream<TcpStream> {
    let cfg = probe::client_config(Some(&d.cacert()), alpn).unwrap();
    let tcp = TcpStream::connect(SocketAddr::new(V4, d.https))
        .await
        .unwrap();
    let name = rustls::pki_types::ServerName::try_from("api.p.lokal").unwrap();
    tokio_rustls::TlsConnector::from(cfg)
        .connect(name, tcp)
        .await
        .unwrap()
}

/// When the server closed `s` (counted from `since`), reading and dropping
/// whatever it sends first; `None` if it is still open at `since + limit`.
async fn closed_by<S: AsyncRead + Unpin>(
    mut s: S,
    since: Instant,
    limit: Duration,
) -> Option<Duration> {
    let mut buf = [0u8; 4096];
    loop {
        match tokio::time::timeout_at((since + limit).into(), s.read(&mut buf)).await {
            Err(_) => return None,
            Ok(Ok(0)) | Ok(Err(_)) => return Some(since.elapsed()),
            Ok(Ok(_)) => {}
        }
    }
}

fn status_ok(d: &Daemon) {
    let out = d.lokal(&["status", "--json"]);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success() && text.contains("\"running\": true"),
        "status: {text}{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Every state a connection can idle in before (or between) requests is
/// closed by its deadline. A proxied request and a WebSocket aren't.
#[tokio::test(flavor = "multi_thread")]
async fn idle_connections_are_closed_by_their_deadlines() {
    let d = daemon(
        &[
            "--pre-request-timeout-ms",
            "500",
            "--idle-timeout-ms",
            "1500",
        ],
        None,
    )
    .await;
    let http = SocketAddr::new(V4, d.http);
    let t0 = Instant::now();
    let limit = Duration::from_secs(4);
    let mut checks = vec![];
    macro_rules! check {
        ($what:expr, $s:expr) => {
            checks.push(($what, tokio::spawn(closed_by($s, t0, limit))));
        };
    }

    check!(
        "port 80, nothing sent",
        TcpStream::connect(http).await.unwrap()
    );
    // Port 80 is HTTP/1.1 only: an h2 preface is refused at once instead of
    // waiting in protocol sniffing.
    let mut preface = TcpStream::connect(http).await.unwrap();
    preface.write_all(&H2_PREFACE[..23]).await.unwrap();
    check!("port 80, most of an h2 preface", preface);
    let mut partial = TcpStream::connect(http).await.unwrap();
    partial.write_all(b"GET / HTT").await.unwrap();
    check!("port 80, part of a request line", partial);
    check!("TLS http/1.1, nothing sent", tls(&d, &[b"http/1.1"]).await);
    check!("TLS h2, no preface", tls(&d, &[b"h2"]).await);
    let mut h2 = tls(&d, &[b"h2"]).await;
    h2.write_all(&[H2_PREFACE, H2_SETTINGS].concat())
        .await
        .unwrap();
    check!("TLS h2, SETTINGS but no stream", h2);
    check!("TLS without ALPN, nothing sent", tls(&d, &[]).await);
    // A page lokal builds itself (421: Host isn't the SNI) never makes the
    // connection busy: the idle deadline still applies after it.
    let mut page = tls(&d, &[b"http/1.1"]).await;
    page.write_all(b"GET / HTTP/1.1\r\nhost: other.p.lokal\r\n\r\n")
        .await
        .unwrap();
    check!("after a lokal-built 421 page", page);

    // Proxied exchanges are busy: neither deadline cuts them (SPEC §9.2).
    let opts = d.opts();
    let slow =
        tokio::spawn(async move { probe::get("https://api.p.lokal/slow?ms=2500", &opts).await });
    let mut ws = {
        let io = tls(&d, &[b"http/1.1"]).await;
        let (ws, resp) = tokio_tungstenite::client_async("wss://api.p.lokal/ws", io)
            .await
            .unwrap();
        assert_eq!(resp.status(), 101);
        ws
    };

    for (what, h) in checks {
        let closed = h.await.unwrap();
        assert!(closed.is_some(), "{what}: still open after {limit:?}");
    }
    let r = slow.await.unwrap().unwrap();
    assert_eq!(
        r.status, 200,
        "a 2.5s proxied request outlives both deadlines"
    );
    let quiet = t0.elapsed();
    assert!(quiet > Duration::from_millis(2500), "{quiet:?}");
    ws.send(Message::text("still-here")).await.unwrap();
    let reply = tokio::time::timeout(Duration::from_secs(3), ws.next())
        .await
        .expect("WebSocket reply")
        .unwrap()
        .unwrap();
    assert!(
        reply.into_text().unwrap().ends_with("still-here"),
        "a quiet WebSocket outlives both deadlines"
    );
}

/// With the client cap full of idle connections, a new one evicts the one
/// idle longest: real requests and the CLI keep working.
#[tokio::test(flavor = "multi_thread")]
async fn a_full_cap_of_idle_connections_still_admits_real_requests() {
    let d = daemon(&["--max-client-connections", "8"], None).await;
    let t0 = Instant::now();
    let mut idle = vec![];
    for i in 0..12 {
        let port = if i % 2 == 0 { d.http } else { d.https };
        idle.push(TcpStream::connect(SocketAddr::new(V4, port)).await.unwrap());
    }
    let r = probe::get("https://api.p.lokal/", &d.opts()).await.unwrap();
    assert_eq!(r.status, 200);
    status_ok(&d);
    let closed: Vec<_> = idle
        .into_iter()
        .map(|s| tokio::spawn(closed_by(s, t0, Duration::from_secs(2))))
        .collect();
    let mut n = 0;
    for h in closed {
        n += usize::from(h.await.unwrap().is_some());
    }
    assert!(
        n >= 5,
        "12 idle + 1 real into 8 slots evicts at least 5, got {n}"
    );
}

/// A new connection is refused only when every admitted one has a proxied
/// request in flight, and the CLI still reaches the daemon then.
#[tokio::test(flavor = "multi_thread")]
async fn with_every_connection_busy_new_ones_are_refused_and_the_cli_still_works() {
    let d = daemon(&["--max-client-connections", "4"], None).await;
    let slow: Vec<_> = (0..4)
        .map(|_| {
            let opts = d.opts();
            tokio::spawn(async move { probe::get("https://api.p.lokal/slow?ms=2000", &opts).await })
        })
        .collect();
    tokio::time::sleep(Duration::from_millis(700)).await;
    let refused = probe::get("https://api.p.lokal/", &d.opts()).await;
    assert!(refused.is_err(), "{refused:?}");
    status_ok(&d);
    for s in slow {
        assert_eq!(s.await.unwrap().unwrap().status, 200);
    }
    let r = probe::get("https://api.p.lokal/", &d.opts()).await.unwrap();
    assert_eq!(r.status, 200, "room again once they finish");
}

/// At the upstream cap, a request that needs a new upstream socket gets a
/// fast 503 instead of the daemon running out of descriptors.
#[tokio::test(flavor = "multi_thread")]
async fn the_upstream_cap_answers_503() {
    let d = daemon(&["--max-upstream-connections", "1"], None).await;
    let opts = d.opts();
    let slow =
        tokio::spawn(async move { probe::get("https://api.p.lokal/slow?ms=1500", &opts).await });
    tokio::time::sleep(Duration::from_millis(500)).await;
    let r = probe::get("https://api.p.lokal/", &d.opts()).await.unwrap();
    assert_eq!(r.status, 503, "{}", r.text());
    assert!(
        r.text().contains("too many upstream connections"),
        "{}",
        r.text()
    );
    assert_eq!(slow.await.unwrap().unwrap().status, 200);
    // The finished request's upstream connection is pooled and reused.
    let r = probe::get("https://api.p.lokal/", &d.opts()).await.unwrap();
    assert_eq!(r.status, 200);
}

/// The audit's reproduction, at a 48-fd limit: 40 idle sockets used to take
/// every descriptor, so new HTTPS connections and `lokal status` failed. The
/// caps come from the limit and leave a reserve.
#[tokio::test(flavor = "multi_thread")]
async fn idle_connections_cannot_exhaust_file_descriptors() {
    let d = daemon(&[], Some(48)).await;
    let mut held = vec![];
    for i in 0..40 {
        let port = if i % 2 == 0 { d.http } else { d.https };
        held.push(TcpStream::connect(SocketAddr::new(V4, port)).await.unwrap());
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    let r = probe::get("https://api.p.lokal/", &d.opts()).await.unwrap();
    assert_eq!(r.status, 200);
    status_ok(&d);
    drop(held);
}

//! Probes for the live test (docs/TESTING.md). Each prints one JSON line and
//! exits 0 on success, 1 on failure. TLS is verified by
//! rustls-platform-verifier (what macOS trusts). `--cacert`/`--connect`
//! exist only for sandbox integration tests; the live test never passes them.

use std::net::SocketAddr;
use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};
use serde_json::json;
use testkit::probe::{self, Opts};

#[derive(Parser)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Args, Clone)]
struct Common {
    /// Trust only this CA file (sandbox tests only)
    #[arg(long)]
    cacert: Option<PathBuf>,
    /// Connect to this address instead of resolving (sandbox tests only)
    #[arg(long)]
    connect: Option<SocketAddr>,
}

impl Common {
    fn opts(&self, http2: bool) -> Opts {
        Opts {
            connect: self.connect,
            cacert: self.cacert.clone(),
            http2,
        }
    }
}

#[derive(Subcommand)]
enum Cmd {
    /// getaddrinfo: addresses and elapsed time (first and repeated lookup)
    Resolve { host: String },
    /// TLS handshake(s) verified by the OS trust store
    Tls {
        host: String,
        #[arg(long, default_value_t = 443)]
        port: u16,
        /// Number of handshakes (sessions resume after the first)
        #[arg(short, default_value_t = 1)]
        n: usize,
        #[command(flatten)]
        c: Common,
    },
    /// HTTP(S) GET; prints status, version, headers and body
    Get {
        url: String,
        #[arg(long)]
        http2: bool,
        #[command(flatten)]
        c: Common,
    },
    /// WebSocket echo
    Ws {
        url: String,
        #[command(flatten)]
        c: Common,
    },
    /// Server-sent events: 3 events, each within 300ms of being sent
    Sse {
        url: String,
        #[command(flatten)]
        c: Common,
    },
    /// Keep-alive latency distribution
    Bench {
        url: String,
        #[arg(short, default_value_t = 200)]
        n: usize,
        #[command(flatten)]
        c: Common,
    },
}

fn ms(d: std::time::Duration) -> f64 {
    (d.as_secs_f64() * 1e6).round() / 1e3
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let cli = Cli::parse();
    let (ok, v) = run(cli.cmd).await;
    println!("{v}");
    std::process::exit(if ok { 0 } else { 1 });
}

async fn run(cmd: Cmd) -> (bool, serde_json::Value) {
    match cmd {
        Cmd::Resolve { host } => {
            let (r1, d1) = probe::resolve(&host);
            match r1 {
                Ok(addrs) => {
                    // Repeat only after a success (a failed .local lookup takes ~5s).
                    let (r2, d2) = probe::resolve(&host);
                    let s: Vec<String> = addrs.iter().map(|a| a.to_string()).collect();
                    (
                        !addrs.is_empty(),
                        json!({ "check": "resolve", "ok": !addrs.is_empty(), "host": host, "addrs": s,
                                 "first_ms": ms(d1), "repeat_ms": ms(d2),
                                 "repeat_addrs": r2.map(|a| a.iter().map(|x| x.to_string()).collect::<Vec<_>>()).unwrap_or_default() }),
                    )
                }
                Err(e) => (
                    false,
                    json!({ "check": "resolve", "ok": false, "host": host, "error": e.to_string(), "first_ms": ms(d1) }),
                ),
            }
        }
        Cmd::Tls { host, port, n, c } => {
            match probe::tls_handshakes(&host, port, n, &c.opts(false)).await {
                Ok((v, alpn)) => {
                    let all: Vec<f64> = v.iter().map(|d| ms(*d)).collect();
                    let resumed: Vec<std::time::Duration> = v.iter().skip(1).copied().collect();
                    (
                        true,
                        json!({ "check": "tls", "ok": true, "host": host, "alpn": alpn, "handshakes_ms": all,
                             "first_ms": all.first(), "resumed_p95_ms": probe::pct(&resumed, 95.0) }),
                    )
                }
                Err(e) => (
                    false,
                    json!({ "check": "tls", "ok": false, "host": host, "error": format!("{e:#}") }),
                ),
            }
        }
        Cmd::Get { url, http2, c } => match probe::get(&url, &c.opts(http2)).await {
            Ok(r) => {
                let headers: serde_json::Map<String, serde_json::Value> = r
                    .headers
                    .iter()
                    .map(|(k, v)| (k.clone(), json!(v)))
                    .collect();
                (
                    true,
                    json!({ "check": "get", "ok": true, "url": url, "status": r.status, "version": r.version,
                               "headers": headers, "body": r.text() }),
                )
            }
            Err(e) => (
                false,
                json!({ "check": "get", "ok": false, "url": url, "error": format!("{e:#}") }),
            ),
        },
        Cmd::Ws { url, c } => match probe::ws_echo(&url, &c.opts(false)).await {
            Ok(echo) => (
                true,
                json!({ "check": "ws", "ok": true, "url": url, "echo": echo }),
            ),
            Err(e) => (
                false,
                json!({ "check": "ws", "ok": false, "url": url, "error": format!("{e:#}") }),
            ),
        },
        Cmd::Sse { url, c } => match probe::sse(&url, &c.opts(false), 3).await {
            Ok(lags) => {
                let ok = lags.len() == 3 && lags.iter().all(|l| *l <= 300);
                (
                    ok,
                    json!({ "check": "sse", "ok": ok, "url": url, "events": lags.len(), "lags_ms": lags }),
                )
            }
            Err(e) => (
                false,
                json!({ "check": "sse", "ok": false, "url": url, "error": format!("{e:#}") }),
            ),
        },
        Cmd::Bench { url, n, c } => match probe::bench(&url, n, &c.opts(false)).await {
            Ok(v) => (
                true,
                json!({ "check": "bench", "ok": true, "url": url, "n": n,
                         "p50_ms": probe::pct(&v, 50.0), "p95_ms": probe::pct(&v, 95.0), "p99_ms": probe::pct(&v, 99.0) }),
            ),
            Err(e) => (
                false,
                json!({ "check": "bench", "ok": false, "url": url, "error": format!("{e:#}") }),
            ),
        },
    }
}

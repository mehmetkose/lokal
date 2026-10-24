//! TLS termination and the accept loops (SPEC §9). rustls 0.23 with ring,
//! TLS 1.2 + 1.3, ALPN h2/http1.1, session resumption, never HSTS.
//! Connection deadlines and caps: `limits` (SPEC §9.3).

pub mod connector;
pub mod forward;
pub mod limits;
pub mod pages;
pub mod routes;

use std::convert::Infallible;
use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use http_body_util::combinators::BoxBody;
use hyper::server::conn::{http1, http2};
use hyper::service::service_fn;
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use tokio::net::TcpListener;
use tokio::sync::watch;

use crate::ca::resolver::{CertStore, Resolver};
pub use forward::{ConnInfo, Proxy};
pub use limits::{Budget, Limits};
use limits::{Finish, Slot};

pub type BoxError = Box<dyn std::error::Error + Send + Sync>;
pub type Body = BoxBody<Bytes, BoxError>;

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
/// How long a graceful close (GOAWAY, or the end of the current response)
/// may take before the connection is dropped. It needs the peer's help.
const CLOSE_GRACE: Duration = Duration::from_secs(2);
const H2_MAX_STREAMS: u32 = 100;
const H2_KEEP_ALIVE: Duration = Duration::from_secs(20);
const H2_KEEP_ALIVE_TIMEOUT: Duration = Duration::from_secs(10);

pub fn tls_config(certs: Arc<CertStore>) -> Arc<rustls::ServerConfig> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut cfg = rustls::ServerConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13, &rustls::version::TLS12])
        .expect("TLS versions supported by ring")
        .with_no_client_auth()
        .with_cert_resolver(Arc::new(Resolver(certs)));
    cfg.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    cfg.session_storage = rustls::server::ServerSessionMemoryCache::new(4096);
    match rustls::crypto::ring::Ticketer::new() {
        Ok(t) => cfg.ticketer = t,
        Err(e) => tracing::warn!("TLS session tickets disabled: {e}"),
    }
    Arc::new(cfg)
}

/// Counts live connections so shutdown can wait for in-flight requests.
#[derive(Clone, Default)]
pub struct ConnTracker(Arc<()>);

impl ConnTracker {
    pub fn active(&self) -> usize {
        Arc::strong_count(&self.0) - 1
    }

    /// Waits until no connection is active, up to `limit`.
    pub async fn drain(&self, limit: Duration) {
        let deadline = tokio::time::Instant::now() + limit;
        while self.active() > 0 && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
}

/// HTTP/1.1. hyper's header-read timer (30s) also closes a keep-alive
/// connection that sends no next request.
fn h1() -> http1::Builder {
    let mut b = http1::Builder::new();
    b.keep_alive(true).timer(TokioTimer::new());
    b
}

/// HTTP/2. Keep-alive pings reap dead peers; the stream limit bounds how
/// many upstream sockets one connection can ask for at once.
fn h2() -> http2::Builder<TokioExecutor> {
    let mut b = http2::Builder::new(TokioExecutor::new());
    b.timer(TokioTimer::new())
        .max_concurrent_streams(H2_MAX_STREAMS)
        .keep_alive_interval(H2_KEEP_ALIVE)
        .keep_alive_timeout(H2_KEEP_ALIVE_TIMEOUT);
    b
}

/// Serves `conn` until it ends, its deadline passes, it is evicted, or the
/// daemon shuts down (SPEC §9.3). Dropping `conn` closes the socket, which
/// needs nothing from the peer; `graceful` (GOAWAY, or close after the
/// current response) gets a bounded grace first when a request was seen.
async fn drive<C, E>(
    conn: C,
    graceful: fn(Pin<&mut C>),
    slot: &Slot,
    mut shutdown: watch::Receiver<bool>,
    peer: SocketAddr,
) where
    C: Future<Output = Result<(), E>>,
    E: std::fmt::Display,
{
    tokio::pin!(conn);
    loop {
        tokio::select! {
            r = conn.as_mut() => {
                if let Err(e) = r {
                    tracing::debug!(%peer, "connection: {e}");
                }
                return;
            }
            _ = tokio::time::sleep_until(slot.deadline()) => {
                if !slot.expired() {
                    continue;
                }
                if slot.seen_request() {
                    graceful(conn.as_mut());
                    let _ = tokio::time::timeout(CLOSE_GRACE, conn.as_mut()).await;
                }
                return;
            }
            _ = slot.evicted() => {
                // Picked while idle, so normally dropped at once. A proxied
                // exchange that started since gets a bounded grace.
                if slot.busy() {
                    graceful(conn.as_mut());
                    let _ = tokio::time::timeout(CLOSE_GRACE, conn.as_mut()).await;
                }
                return;
            }
            _ = shutdown.changed() => {
                graceful(conn.as_mut());
                let _ = conn.as_mut().await;
                return;
            }
        }
    }
}

/// Serves HTTPS on `listener` until `shutdown` flips to true.
pub async fn serve_https(
    listener: TcpListener,
    tls: Arc<rustls::ServerConfig>,
    proxy: Arc<Proxy>,
    mut shutdown: watch::Receiver<bool>,
    tracker: ConnTracker,
    budget: Budget,
) {
    let acceptor = tokio_rustls::TlsAcceptor::from(tls);
    loop {
        let (tcp, peer) = tokio::select! {
            r = listener.accept() => match r {
                Ok(x) => x,
                Err(e) => {
                    tracing::warn!("accept: {e}");
                    tokio::time::sleep(Duration::from_millis(10)).await;
                    continue;
                }
            },
            _ = shutdown.changed() => break,
        };
        let Some(permit) = budget.admit().await else {
            continue; // every connection is busy: refuse this one (drops `tcp`)
        };
        let slot = budget.open(permit);
        let _ = tcp.set_nodelay(true);
        let acceptor = acceptor.clone();
        let proxy = proxy.clone();
        let shutdown = shutdown.clone();
        let guard = tracker.clone();
        tokio::spawn(async move {
            let _guard = guard;
            let _finish = Finish(slot.clone());
            let tls = tokio::select! {
                r = tokio::time::timeout(HANDSHAKE_TIMEOUT, acceptor.accept(tcp)) => match r {
                    Ok(Ok(s)) => s,
                    Ok(Err(e)) => {
                        tracing::debug!(%peer, "TLS handshake failed: {e}");
                        return;
                    }
                    Err(_) => return,
                },
                _ = slot.evicted() => return,
            };
            // The pre-request deadline counts from the end of the handshake.
            slot.touch();
            let conn_info = tls.get_ref().1;
            let sni = conn_info
                .server_name()
                .map(|s| s.trim_end_matches('.').to_ascii_lowercase());
            // The protocol comes from ALPN, never from sniffing the first
            // bytes: hyper-util's sniffing has no deadline.
            let is_h2 = conn_info.alpn_protocol() == Some(&b"h2"[..]);
            let info = Arc::new(ConnInfo {
                peer,
                sni,
                slot: Some(slot.clone()),
            });
            let svc = service_fn(move |req| {
                let proxy = proxy.clone();
                let info = info.clone();
                async move {
                    if let Some(s) = &info.slot {
                        s.request_started();
                    }
                    Ok::<_, Infallible>(proxy.handle(req, &info).await)
                }
            });
            let io = TokioIo::new(tls);
            if is_h2 {
                let conn = h2().serve_connection(io, svc);
                drive(conn, |c| c.graceful_shutdown(), &slot, shutdown, peer).await;
            } else {
                let conn = h1().serve_connection(io, svc).with_upgrades();
                drive(conn, |c| c.graceful_shutdown(), &slot, shutdown, peer).await;
            }
        });
    }
}

/// Serves plain HTTP (redirects) on `listener` until `shutdown`. HTTP/1.1
/// only: nothing here is proxied, so there is no h2c.
pub async fn serve_http(
    listener: TcpListener,
    proxy: Arc<Proxy>,
    mut shutdown: watch::Receiver<bool>,
    tracker: ConnTracker,
    budget: Budget,
) {
    loop {
        let (tcp, peer) = tokio::select! {
            r = listener.accept() => match r {
                Ok(x) => x,
                Err(e) => {
                    tracing::warn!("accept: {e}");
                    tokio::time::sleep(Duration::from_millis(10)).await;
                    continue;
                }
            },
            _ = shutdown.changed() => break,
        };
        let Some(permit) = budget.admit().await else {
            continue;
        };
        let slot = budget.open(permit);
        let proxy = proxy.clone();
        let shutdown = shutdown.clone();
        let guard = tracker.clone();
        tokio::spawn(async move {
            let _guard = guard;
            let _finish = Finish(slot.clone());
            let s = slot.clone();
            let svc = service_fn(move |req| {
                let proxy = proxy.clone();
                s.request_started();
                async move { Ok::<_, Infallible>(proxy.handle_http(req).await) }
            });
            let conn = h1().serve_connection(TokioIo::new(tcp), svc);
            drive(conn, |c| c.graceful_shutdown(), &slot, shutdown, peer).await;
        });
    }
}

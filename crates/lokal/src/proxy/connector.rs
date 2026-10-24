//! Upstream connector for the hyper-util client. `localhost` upstreams race
//! `::1` and `127.0.0.1` (happy-eyeballs style) and remember the winner per
//! port for 30s: Node binds `localhost` to ::1 only, Python to 127.0.0.1.
//!
//! Every upstream socket holds a permit from the connection budget for as
//! long as it is open (SPEC §9.3), so the daemon can't run out of file
//! descriptors: at the cap, a connect fails fast and the request gets a 503.

use std::collections::HashMap;
use std::future::Future;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, ToSocketAddrs};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use http::Uri;
use hyper_util::client::legacy::connect::{Connected, Connection};
use hyper_util::rt::TokioIo;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;
use tokio::sync::OwnedSemaphorePermit;

use super::limits::Budget;

pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const WINNER_TTL: Duration = Duration::from_secs(30);

/// The connector's error when the upstream cap (or the lookup cap) is
/// reached. `forward` turns it into a 503.
#[derive(Debug)]
pub struct UpstreamCapReached;

impl std::fmt::Display for UpstreamCapReached {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("lokal's upstream connection limit is reached")
    }
}

impl std::error::Error for UpstreamCapReached {}

/// An upstream socket and its permit. The permit stays with the socket while
/// it is idle in the pool and inside an upgraded connection.
pub struct UpstreamStream {
    tcp: TcpStream,
    _permit: Option<OwnedSemaphorePermit>,
}

impl AsyncRead for UpstreamStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.tcp).poll_read(cx, buf)
    }
}

impl AsyncWrite for UpstreamStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.tcp).poll_write(cx, buf)
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.tcp).poll_write_vectored(cx, bufs)
    }

    fn is_write_vectored(&self) -> bool {
        self.tcp.is_write_vectored()
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.tcp).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.tcp).poll_shutdown(cx)
    }
}

impl Connection for UpstreamStream {
    fn connected(&self) -> Connected {
        self.tcp.connected()
    }
}

/// Connects to `a` and `b` in parallel; the first success wins.
async fn race(a: SocketAddr, b: SocketAddr) -> io::Result<TcpStream> {
    let a = TcpStream::connect(a);
    let b = TcpStream::connect(b);
    tokio::pin!(a, b);
    let (first, other_is_b) = tokio::select! {
        r = &mut a => (r, true),
        r = &mut b => (r, false),
    };
    match first {
        Ok(s) => Ok(s),
        Err(e1) => {
            let r = if other_is_b {
                (&mut b).await
            } else {
                (&mut a).await
            };
            r.map_err(|e2| io::Error::new(e2.kind(), format!("{e1}; {e2}")))
        }
    }
}

#[derive(Clone, Default)]
pub struct Connector {
    winners: Arc<Mutex<HashMap<u16, (IpAddr, Instant)>>>,
    /// Caps upstream sockets and name lookups. `None`: uncapped.
    budget: Option<Budget>,
}

impl Connector {
    pub fn new(budget: Option<Budget>) -> Self {
        Self {
            winners: Default::default(),
            budget,
        }
    }

    /// A permit for one more upstream socket (none needed when uncapped).
    fn permit(&self) -> io::Result<Option<OwnedSemaphorePermit>> {
        match &self.budget {
            None => Ok(None),
            Some(b) => b
                .upstream(1)
                .map(Some)
                .ok_or_else(|| io::Error::other(UpstreamCapReached)),
        }
    }

    fn cached(&self, port: u16) -> Option<IpAddr> {
        let w = self.winners.lock().ok()?;
        w.get(&port)
            .filter(|(_, at)| at.elapsed() < WINNER_TTL)
            .map(|(ip, _)| *ip)
    }

    fn remember(&self, port: u16, ip: IpAddr) {
        if let Ok(mut w) = self.winners.lock() {
            w.insert(port, (ip, Instant::now()));
        }
    }

    fn forget(&self, port: u16) {
        if let Ok(mut w) = self.winners.lock() {
            w.remove(&port);
        }
    }

    async fn connect_localhost(&self, port: u16) -> io::Result<UpstreamStream> {
        if let Some(ip) = self.cached(port) {
            let permit = self.permit()?;
            match TcpStream::connect(SocketAddr::new(ip, port)).await {
                Ok(tcp) => {
                    return Ok(UpstreamStream {
                        tcp,
                        _permit: permit,
                    });
                }
                Err(_) => self.forget(port),
            }
        }
        let permit = self.permit()?;
        let v6 = SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), port);
        let v4 = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
        let res = match self.permit() {
            // A second permit for the race; it goes back when the race ends.
            Ok(_race) => race(v6, v4).await,
            // At the cap: one family after the other, on one permit.
            Err(_) => match TcpStream::connect(v6).await {
                Ok(s) => Ok(s),
                Err(e1) => TcpStream::connect(v4)
                    .await
                    .map_err(|e2| io::Error::new(e2.kind(), format!("{e1}; {e2}"))),
            },
        };
        if let Ok(s) = &res
            && let Ok(peer) = s.peer_addr()
        {
            self.remember(port, peer.ip());
        }
        res.map(|tcp| UpstreamStream {
            tcp,
            _permit: permit,
        })
    }

    /// Resolves `host` on a blocking thread. The lookup permit moves into
    /// that thread: `getaddrinfo` keeps running after a connect gives up.
    async fn lookup(&self, host: &str, port: u16) -> io::Result<Vec<SocketAddr>> {
        let permit = match &self.budget {
            None => None,
            Some(b) => Some(
                b.lookup()
                    .ok_or_else(|| io::Error::other(UpstreamCapReached))?,
            ),
        };
        let host = host.to_owned();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            (host.as_str(), port).to_socket_addrs().map(|a| a.collect())
        })
        .await
        .map_err(io::Error::other)?
    }

    pub async fn connect(&self, host: &str, port: u16) -> io::Result<UpstreamStream> {
        let fut = async {
            if host == "localhost" {
                return self.connect_localhost(port).await;
            }
            if let Ok(ip) = host.parse::<IpAddr>() {
                let permit = self.permit()?;
                let tcp = TcpStream::connect(SocketAddr::new(ip, port)).await?;
                return Ok(UpstreamStream {
                    tcp,
                    _permit: permit,
                });
            }
            let mut last =
                io::Error::new(io::ErrorKind::NotFound, format!("{host} does not resolve"));
            let addrs = self.lookup(host, port).await?;
            let permit = self.permit()?;
            for addr in addrs {
                match TcpStream::connect(addr).await {
                    Ok(tcp) => {
                        return Ok(UpstreamStream {
                            tcp,
                            _permit: permit,
                        });
                    }
                    Err(e) => last = e,
                }
            }
            Err(last)
        };
        let s = tokio::time::timeout(CONNECT_TIMEOUT, fut)
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "connect timed out after 2s"))??;
        s.tcp.set_nodelay(true)?;
        Ok(s)
    }
}

impl tower_service::Service<Uri> for Connector {
    type Response = TokioIo<UpstreamStream>;
    type Error = io::Error;
    type Future = Pin<Box<dyn Future<Output = io::Result<Self::Response>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, uri: Uri) -> Self::Future {
        let this = self.clone();
        Box::pin(async move {
            let host = uri
                .host()
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no host"))?
                .trim_start_matches('[')
                .trim_end_matches(']')
                .to_string();
            let port = uri.port_u16().unwrap_or(80);
            Ok(TokioIo::new(this.connect(&host, port).await?))
        })
    }
}

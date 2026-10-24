//! Probes behind `check` and the sandbox integration tests. TLS is verified
//! by `rustls-platform-verifier` (exactly what macOS trusts), or by a CA
//! file in sandbox tests (`--cacert`, never in the live test).

use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use http::Request;
use http_body_util::{BodyExt, Empty, combinators::BoxBody};
use hyper_util::rt::{TokioExecutor, TokioIo};
use rustls::pki_types::ServerName;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;

pub type ReqBody = BoxBody<Bytes, std::io::Error>;

#[derive(Debug, Clone)]
pub struct Target {
    pub tls: bool,
    pub host: String,
    pub port: u16,
    /// Path and query.
    pub path: String,
}

pub fn parse_url(url: &str) -> Result<Target> {
    let (scheme, rest) = url
        .split_once("://")
        .ok_or_else(|| anyhow!("bad url {url}"))?;
    let tls = match scheme {
        "https" | "wss" => true,
        "http" | "ws" => false,
        s => bail!("unsupported scheme {s}"),
    };
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], rest[i..].to_string()),
        None => (rest, "/".to_string()),
    };
    let (host, port) = if let Some(h) = authority.strip_prefix('[') {
        let (h, p) = h.split_once(']').ok_or_else(|| anyhow!("bad host"))?;
        (
            h.to_string(),
            p.strip_prefix(':').map(|p| p.parse()).transpose()?,
        )
    } else {
        match authority.rsplit_once(':') {
            Some((h, p)) => (h.to_string(), Some(p.parse()?)),
            None => (authority.to_string(), None),
        }
    };
    Ok(Target {
        tls,
        port: port.unwrap_or(if tls { 443 } else { 80 }),
        host,
        path,
    })
}

#[derive(Debug, Clone, Default)]
pub struct Opts {
    /// Connect here instead of resolving the host (sandbox tests only).
    pub connect: Option<SocketAddr>,
    /// Trust only this CA (sandbox tests only); otherwise the OS trust store.
    pub cacert: Option<std::path::PathBuf>,
    pub http2: bool,
}

pub fn ensure_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

pub fn client_config(cacert: Option<&Path>, alpn: &[&[u8]]) -> Result<Arc<rustls::ClientConfig>> {
    ensure_provider();
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let b = rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()?;
    let mut cfg = match cacert {
        None => b
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(rustls_platform_verifier::Verifier::new(
                provider,
            )?))
            .with_no_client_auth(),
        Some(p) => {
            let data = std::fs::read(p).with_context(|| format!("read {}", p.display()))?;
            let mut roots = rustls::RootCertStore::empty();
            for c in rustls_pemfile::certs(&mut &data[..]) {
                roots.add(c?)?;
            }
            b.with_root_certificates(roots).with_no_client_auth()
        }
    };
    cfg.alpn_protocols = alpn.iter().map(|a| a.to_vec()).collect();
    Ok(Arc::new(cfg))
}

/// `getaddrinfo(host)`: unique addresses and the time it took.
pub fn resolve(host: &str) -> (Result<Vec<IpAddr>>, Duration) {
    let t = Instant::now();
    let r = (host, 0u16)
        .to_socket_addrs()
        .map(|it| {
            let mut v: Vec<IpAddr> = vec![];
            for a in it {
                if !v.contains(&a.ip()) {
                    v.push(a.ip());
                }
            }
            v
        })
        .map_err(|e| anyhow!("{e}"));
    (r, t.elapsed())
}

pub async fn connect_tcp(t: &Target, connect: Option<SocketAddr>) -> Result<TcpStream> {
    let s = match connect {
        Some(a) => TcpStream::connect(a)
            .await
            .with_context(|| format!("connect {a}"))?,
        None => {
            let mut last = anyhow!("{} does not resolve", t.host);
            let mut found = None;
            for a in tokio::net::lookup_host((t.host.as_str(), t.port)).await? {
                match TcpStream::connect(a).await {
                    Ok(s) => {
                        found = Some(s);
                        break;
                    }
                    Err(e) => last = anyhow!("connect {a}: {e}"),
                }
            }
            found.ok_or(last)?
        }
    };
    s.set_nodelay(true)?;
    Ok(s)
}

pub trait Io: AsyncRead + AsyncWrite + Unpin + Send + 'static {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send + 'static> Io for T {}

pub async fn open(
    t: &Target,
    connect: Option<SocketAddr>,
    cfg: &Arc<rustls::ClientConfig>,
) -> Result<(Box<dyn Io>, Option<Vec<u8>>)> {
    let tcp = connect_tcp(t, connect).await?;
    if !t.tls {
        return Ok((Box::new(tcp), None));
    }
    let name = ServerName::try_from(t.host.clone())?;
    let tls = tokio_rustls::TlsConnector::from(cfg.clone())
        .connect(name, tcp)
        .await
        .context("TLS handshake")?;
    let alpn = tls.get_ref().1.alpn_protocol().map(|a| a.to_vec());
    Ok((Box::new(tls), alpn))
}

#[derive(Debug, Clone)]
pub struct Resp {
    pub status: u16,
    pub version: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Resp {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
    pub fn json(&self) -> Result<serde_json::Value> {
        Ok(serde_json::from_slice(&self.body)?)
    }
}

pub fn empty_body() -> ReqBody {
    Empty::<Bytes>::new()
        .map_err(|never| match never {})
        .boxed()
}

/// One request on a fresh connection.
pub async fn request(
    url: &str,
    opts: &Opts,
    method: &str,
    headers: &[(&str, &str)],
    body: ReqBody,
) -> Result<Resp> {
    let t = parse_url(url)?;
    let alpn: &[&[u8]] = if opts.http2 { &[b"h2"] } else { &[b"http/1.1"] };
    let cfg = client_config(opts.cacert.as_deref(), alpn)?;
    let (io, negotiated) = open(&t, opts.connect, &cfg).await?;
    let host_hdr = if (t.tls && t.port == 443) || (!t.tls && t.port == 80) {
        t.host.clone()
    } else {
        format!("{}:{}", t.host, t.port)
    };
    let mut b = Request::builder().method(method);
    for (k, v) in headers {
        b = b.header(*k, *v);
    }
    let resp = if opts.http2 {
        if negotiated.as_deref() != Some(b"h2") {
            bail!(
                "server did not negotiate h2 (got {:?})",
                negotiated.map(|a| String::from_utf8_lossy(&a).into_owned())
            );
        }
        let (mut send, conn) =
            hyper::client::conn::http2::handshake(TokioExecutor::new(), TokioIo::new(io)).await?;
        tokio::spawn(conn);
        let req = b.uri(format!("https://{host_hdr}{}", t.path)).body(body)?;
        send.send_request(req).await?
    } else {
        let (mut send, conn) = hyper::client::conn::http1::handshake(TokioIo::new(io)).await?;
        tokio::spawn(conn.with_upgrades());
        let mut req = b.uri(t.path.as_str()).body(body)?;
        if !req.headers().contains_key(http::header::HOST) {
            req.headers_mut()
                .insert(http::header::HOST, host_hdr.parse()?);
        }
        send.send_request(req).await?
    };
    let status = resp.status().as_u16();
    let version = format!("{:?}", resp.version());
    let hdrs = resp
        .headers()
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
        .collect();
    let body = resp.into_body().collect().await?.to_bytes().to_vec();
    Ok(Resp {
        status,
        version,
        headers: hdrs,
        body,
    })
}

pub async fn get(url: &str, opts: &Opts) -> Result<Resp> {
    request(url, opts, "GET", &[], empty_body()).await
}

/// TLS handshake timings for `n` fresh connections sharing one client
/// config (so sessions resume after the first).
pub async fn tls_handshakes(
    host: &str,
    port: u16,
    n: usize,
    opts: &Opts,
) -> Result<(Vec<Duration>, String)> {
    let cfg = client_config(opts.cacert.as_deref(), &[b"h2", b"http/1.1"])?;
    let t = Target {
        tls: true,
        host: host.into(),
        port,
        path: "/".into(),
    };
    let mut v = vec![];
    let mut alpn = String::new();
    for _ in 0..n.max(1) {
        let start = Instant::now();
        let (_io, a) = open(&t, opts.connect, &cfg).await?;
        v.push(start.elapsed());
        alpn = a
            .map(|a| String::from_utf8_lossy(&a).into_owned())
            .unwrap_or_default();
    }
    Ok((v, alpn))
}

/// Sends a text message over WebSocket and expects it echoed.
pub async fn ws_echo(url: &str, opts: &Opts) -> Result<String> {
    let t = parse_url(url)?;
    let cfg = client_config(opts.cacert.as_deref(), &[b"http/1.1"])?;
    let (io, _) = open(&t, opts.connect, &cfg).await?;
    let (mut ws, resp) = tokio_tungstenite::client_async(url, io)
        .await
        .context("WebSocket handshake")?;
    if resp.status().as_u16() != 101 {
        bail!("expected 101, got {}", resp.status());
    }
    let msg = format!("hello-{}", std::process::id());
    ws.send(tokio_tungstenite::tungstenite::Message::text(msg.clone()))
        .await?;
    let reply = tokio::time::timeout(Duration::from_secs(3), ws.next())
        .await
        .context("no WebSocket reply within 3s")?
        .ok_or_else(|| anyhow!("WebSocket closed"))??;
    let text = reply.into_text()?.to_string();
    if !text.ends_with(&msg) {
        bail!("unexpected echo {text:?}");
    }
    let _ = ws.close(None).await;
    Ok(text)
}

/// Reads an SSE stream; returns `(events, lag of each event in ms)`.
pub async fn sse(url: &str, opts: &Opts, want: usize) -> Result<Vec<u128>> {
    let t = parse_url(url)?;
    let cfg = client_config(opts.cacert.as_deref(), &[b"http/1.1"])?;
    let (io, _) = open(&t, opts.connect, &cfg).await?;
    let (mut send, conn) = hyper::client::conn::http1::handshake(TokioIo::new(io)).await?;
    tokio::spawn(conn);
    let req = Request::builder()
        .uri(t.path.as_str())
        .header("host", t.host.as_str())
        .header("accept", "text/event-stream")
        .body(empty_body())?;
    let resp = send.send_request(req).await?;
    if resp.status() != 200 {
        bail!("status {}", resp.status());
    }
    let mut body = resp.into_body();
    let mut buf = String::new();
    let mut lags = vec![];
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while lags.len() < want {
        let frame = tokio::time::timeout_at(deadline, body.frame())
            .await
            .context("SSE timed out")?
            .ok_or_else(|| anyhow!("stream ended after {} events", lags.len()))??;
        let now = crate::echo::now_ms();
        if let Some(d) = frame.data_ref() {
            buf.push_str(&String::from_utf8_lossy(d));
            while let Some(i) = buf.find("\n\n") {
                let ev: String = buf.drain(..i + 2).collect();
                if let Some(data) = ev.lines().find_map(|l| l.strip_prefix("data: ")) {
                    let sent: u128 = data
                        .split_whitespace()
                        .nth(1)
                        .and_then(|s| s.parse().ok())
                        .unwrap_or(now);
                    lags.push(now.saturating_sub(sent));
                }
            }
        }
    }
    Ok(lags)
}

/// `n` sequential GETs on one keep-alive HTTP/1.1 connection.
pub async fn bench(url: &str, n: usize, opts: &Opts) -> Result<Vec<Duration>> {
    let t = parse_url(url)?;
    let cfg = client_config(opts.cacert.as_deref(), &[b"http/1.1"])?;
    let (io, _) = open(&t, opts.connect, &cfg).await?;
    let (mut send, conn) = hyper::client::conn::http1::handshake(TokioIo::new(io)).await?;
    tokio::spawn(conn);
    let mut v = Vec::with_capacity(n);
    for _ in 0..n {
        let req = Request::builder()
            .uri(t.path.as_str())
            .header("host", t.host.as_str())
            .body(empty_body())?;
        let start = Instant::now();
        let resp = send.send_request(req).await?;
        let status = resp.status();
        resp.into_body().collect().await?;
        v.push(start.elapsed());
        if !status.is_success() {
            bail!("status {status}");
        }
    }
    Ok(v)
}

/// Percentile (0–100) of durations, in ms.
pub fn pct(v: &[Duration], p: f64) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    let mut s: Vec<f64> = v.iter().map(|d| d.as_secs_f64() * 1000.0).collect();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let idx = ((p / 100.0) * (s.len() as f64 - 1.0)).round() as usize;
    s[idx.min(s.len() - 1)]
}

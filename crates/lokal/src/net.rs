//! Client-side network probes shared by setup, doctor and the CLI:
//! `getaddrinfo` lookups, TCP reachability, and HTTPS GETs verified either
//! by the OS trust store (`rustls-platform-verifier`) or by the CA file.

use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use rustls::pki_types::ServerName;

/// `getaddrinfo(host)`: unique addresses in the order returned, and the time taken.
pub fn resolve(host: &str) -> (Result<Vec<IpAddr>>, Duration) {
    let t = Instant::now();
    let res = (host, 0u16)
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
        .map_err(|e| anyhow!("{host} does not resolve: {e}"));
    (res, t.elapsed())
}

/// Both loopback addresses present?
pub fn has_both_loopbacks(addrs: &[IpAddr]) -> bool {
    addrs.contains(&IpAddr::V4(Ipv4Addr::LOCALHOST))
        && addrs.contains(&IpAddr::V6(Ipv6Addr::LOCALHOST))
}

pub fn is_loopback_host(host: &str) -> bool {
    host == "localhost"
        || host
            .parse::<IpAddr>()
            .map(|ip| ip.is_loopback())
            .unwrap_or(false)
}

/// Candidate addresses for an upstream host. `localhost` means both
/// loopbacks (Node binds ::1 only, Python 127.0.0.1 only).
pub fn upstream_addrs(host: &str, port: u16) -> Vec<SocketAddr> {
    if host == "localhost" {
        return vec![
            SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), port),
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port),
        ];
    }
    if let Ok(ip) = host.parse::<IpAddr>() {
        return vec![SocketAddr::new(ip, port)];
    }
    (host, port)
        .to_socket_addrs()
        .map(|it| it.collect())
        .unwrap_or_default()
}

/// Is anything accepting TCP connections on `host:port`? Each candidate
/// address gets `timeout`, tried in parallel.
pub fn probe_tcp(host: &str, port: u16, timeout: Duration) -> bool {
    let addrs = upstream_addrs(host, port);
    std::thread::scope(|s| {
        let handles: Vec<_> = addrs
            .iter()
            .map(|a| s.spawn(move || TcpStream::connect_timeout(a, timeout).is_ok()))
            .collect();
        handles.into_iter().any(|h| h.join().unwrap_or(false))
    })
}

/// How a TLS probe decides whether to trust the server.
#[derive(Debug, Clone)]
pub enum Trust {
    /// The OS trust evaluation (on macOS: the keychain), via
    /// rustls-platform-verifier.
    Platform,
    /// Only this CA file (sandbox tests, and `setup --skip-trust`).
    CaFile(PathBuf),
}

pub fn client_config(trust: &Trust) -> Result<Arc<rustls::ClientConfig>> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let builder = rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()?;
    let cfg = match trust {
        Trust::Platform => {
            let verifier = rustls_platform_verifier::Verifier::new(provider)?;
            builder
                .dangerous()
                .with_custom_certificate_verifier(Arc::new(verifier))
                .with_no_client_auth()
        }
        Trust::CaFile(path) => {
            let mut roots = rustls::RootCertStore::empty();
            for c in read_pem_certs(path)? {
                roots.add(c)?;
            }
            builder.with_root_certificates(roots).with_no_client_auth()
        }
    };
    Ok(Arc::new(cfg))
}

pub fn read_pem_certs(path: &Path) -> Result<Vec<rustls::pki_types::CertificateDer<'static>>> {
    let data = std::fs::read(path).with_context(|| format!("cannot read {}", path.display()))?;
    let certs: Vec<_> = rustls_pemfile::certs(&mut &data[..]).collect::<Result<_, _>>()?;
    if certs.is_empty() {
        bail!("no certificate in {}", path.display());
    }
    Ok(certs)
}

#[derive(Debug)]
pub struct HttpsResult {
    pub status: u16,
    pub body: String,
    pub elapsed: Duration,
}

/// Which step of an HTTPS probe failed, for doctor/selftest messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layer {
    Dns,
    Connect,
    Tls,
    Http,
}

impl Layer {
    pub fn name(self) -> &'static str {
        match self {
            Layer::Dns => "dns",
            Layer::Connect => "connect",
            Layer::Tls => "tls",
            Layer::Http => "http",
        }
    }
}

#[derive(Debug)]
pub struct ProbeError {
    pub layer: Layer,
    pub message: String,
}

impl std::fmt::Display for ProbeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.layer.name(), self.message)
    }
}

impl std::error::Error for ProbeError {}

fn perr(layer: Layer, e: impl std::fmt::Display) -> ProbeError {
    ProbeError {
        layer,
        message: e.to_string(),
    }
}

/// `GET https://<host><path>` over a fresh connection. `connect` overrides
/// the address (otherwise `getaddrinfo(host)`, first address, port 443).
pub fn https_get(
    host: &str,
    path: &str,
    connect: Option<SocketAddr>,
    config: Arc<rustls::ClientConfig>,
    timeout: Duration,
) -> std::result::Result<HttpsResult, ProbeError> {
    let t = Instant::now();
    let addr = match connect {
        Some(a) => a,
        None => {
            let (r, _) = resolve(host);
            let ips = r.map_err(|e| perr(Layer::Dns, e))?;
            SocketAddr::new(
                *ips.first()
                    .ok_or_else(|| perr(Layer::Dns, "no addresses"))?,
                443,
            )
        }
    };
    let tcp = TcpStream::connect_timeout(&addr, timeout)
        .map_err(|e| perr(Layer::Connect, format!("{addr}: {e}")))?;
    tcp.set_read_timeout(Some(timeout)).ok();
    tcp.set_write_timeout(Some(timeout)).ok();
    tcp.set_nodelay(true).ok();
    let name = ServerName::try_from(host.to_string()).map_err(|e| perr(Layer::Tls, e))?;
    let conn = rustls::ClientConnection::new(config, name).map_err(|e| perr(Layer::Tls, e))?;
    let mut tls = rustls::StreamOwned::new(conn, tcp);
    // Drive the handshake first so certificate errors are reported as TLS.
    while tls.conn.is_handshaking() {
        tls.conn
            .complete_io(&mut tls.sock)
            .map_err(|e| perr(Layer::Tls, e))?;
    }
    let req = format!(
        "GET {path} HTTP/1.1\r\nHost: {host}\r\nUser-Agent: lokal/{}\r\nAccept: */*\r\nConnection: close\r\n\r\n",
        crate::version::VERSION
    );
    tls.write_all(req.as_bytes())
        .map_err(|e| perr(Layer::Http, e))?;
    let mut buf = Vec::new();
    match tls.read_to_end(&mut buf) {
        Ok(_) => {}
        // Servers may close without close_notify after a complete response.
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof && !buf.is_empty() => {}
        Err(e) => return Err(perr(Layer::Http, e)),
    }
    let text = String::from_utf8_lossy(&buf);
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    let status = head
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| perr(Layer::Http, format!("bad response: {head:?}")))?;
    Ok(HttpsResult {
        status,
        body: body.to_string(),
        elapsed: t.elapsed(),
    })
}

/// TLS handshake only (no request), connecting to `addr` with SNI `host`.
pub fn tls_handshake(
    host: &str,
    addr: SocketAddr,
    config: Arc<rustls::ClientConfig>,
    timeout: Duration,
) -> std::result::Result<Duration, ProbeError> {
    let t = Instant::now();
    let mut tcp = TcpStream::connect_timeout(&addr, timeout)
        .map_err(|e| perr(Layer::Connect, format!("{addr}: {e}")))?;
    tcp.set_read_timeout(Some(timeout)).ok();
    tcp.set_write_timeout(Some(timeout)).ok();
    let name = ServerName::try_from(host.to_string()).map_err(|e| perr(Layer::Tls, e))?;
    let mut conn = rustls::ClientConnection::new(config, name).map_err(|e| perr(Layer::Tls, e))?;
    while conn.is_handshaking() {
        conn.complete_io(&mut tcp)
            .map_err(|e| perr(Layer::Tls, e))?;
    }
    conn.send_close_notify();
    let _ = conn.complete_io(&mut tcp);
    Ok(t.elapsed())
}

//! Request handling (SPEC §9.2): route by Host (falling back to SNI),
//! 421 on Host/SNI mismatch, stream to the upstream over HTTP/1.1 with
//! pooled keep-alive connections, upgrades (WebSocket), error pages.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use arc_swap::ArcSwap;
use http::header::{self, HeaderName, HeaderValue};
use http::{HeaderMap, Request, Response, StatusCode, Uri, Version};
use http_body_util::BodyExt;
use hyper::body::Incoming;
use hyper_util::client::legacy::Client;
use hyper_util::rt::{TokioExecutor, TokioIo};

use super::Body;
use super::connector::{Connector, UpstreamCapReached};
use super::limits::{Budget, BusyBody, Slot};
use super::pages;
use super::routes::{Route, RouteTable, Target, Upstream, normalize_host};
use crate::state::HostHeader;

/// Per-connection facts.
#[derive(Debug, Clone)]
pub struct ConnInfo {
    pub peer: SocketAddr,
    /// TLS SNI (lowercase), `None` on plain HTTP.
    pub sni: Option<String>,
    /// The connection's place in the connection budget (SPEC §9.3).
    pub slot: Option<Arc<Slot>>,
}

pub struct Proxy {
    pub routes: Arc<ArcSwap<RouteTable>>,
    client: Client<Connector, Incoming>,
}

const HOP_BY_HOP: [&str; 9] = [
    "connection",
    "keep-alive",
    "proxy-connection",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
];

fn strip_hop_by_hop(h: &mut HeaderMap) {
    let listed: Vec<HeaderName> = h
        .get_all(header::CONNECTION)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .filter_map(|n| HeaderName::from_bytes(n.trim().as_bytes()).ok())
        .collect();
    for n in listed {
        h.remove(n);
    }
    for n in HOP_BY_HOP {
        h.remove(n);
    }
}

fn upgrade_token(h: &HeaderMap) -> Option<HeaderValue> {
    let conn_has_upgrade = h
        .get_all(header::CONNECTION)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .any(|v| {
            v.split(',')
                .any(|t| t.trim().eq_ignore_ascii_case("upgrade"))
        });
    if conn_has_upgrade {
        h.get(header::UPGRADE).cloned()
    } else {
        None
    }
}

fn request_host(req: &Request<Incoming>) -> Option<String> {
    req.headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .map(normalize_host)
        .or_else(|| req.uri().host().map(normalize_host))
        .filter(|h| !h.is_empty())
}

/// True when the connector refused because the upstream cap is reached.
fn upstream_cap_reached(e: &(dyn std::error::Error + 'static)) -> bool {
    let mut src = Some(e);
    while let Some(x) = src {
        // io::Error::source skips its own payload, so look inside it.
        let inner = x
            .downcast_ref::<std::io::Error>()
            .and_then(|io| io.get_ref())
            .is_some_and(|i| i.is::<UpstreamCapReached>());
        if inner || x.is::<UpstreamCapReached>() {
            return true;
        }
        src = x.source();
    }
    false
}

impl Proxy {
    /// `budget` caps upstream sockets (SPEC §9.3); `None` leaves them uncapped.
    pub fn new(routes: Arc<ArcSwap<RouteTable>>, budget: Option<Budget>) -> Self {
        let client = Client::builder(TokioExecutor::new())
            .pool_idle_timeout(Duration::from_secs(90))
            .pool_max_idle_per_host(64)
            .set_host(false)
            .build(Connector::new(budget));
        Self { routes, client }
    }

    fn not_found(&self, req_headers: &HeaderMap, host: &str) -> Response<Body> {
        let table = self.routes.load();
        let (msg, links) = match table.project_for_host(host) {
            Some(p) => (
                format!("{host} is not configured in project `{}`.", p.name),
                p.urls.clone(),
            ),
            None => (
                format!("{host} is not configured in lokal."),
                table.all_urls(),
            ),
        };
        pages::error(req_headers, StatusCode::NOT_FOUND, &msg, &links)
    }

    /// HTTPS (port 443).
    pub async fn handle(&self, req: Request<Incoming>, info: &ConnInfo) -> Response<Body> {
        let t = Instant::now();
        let method = req.method().clone();
        let path = req
            .uri()
            .path_and_query()
            .map(|p| p.to_string())
            .unwrap_or_else(|| "/".into());
        let host = request_host(&req)
            .or_else(|| info.sni.clone())
            .unwrap_or_default();
        let (resp, upstream) = self.route(req, &host, info).await;
        tracing::info!(
            target: "access",
            "{method} {host}{path} {} {}ms {}",
            resp.status().as_u16(),
            t.elapsed().as_millis(),
            upstream.as_deref().unwrap_or("-")
        );
        resp
    }

    async fn route(
        &self,
        req: Request<Incoming>,
        host: &str,
        info: &ConnInfo,
    ) -> (Response<Body>, Option<String>) {
        if let Some(sni) = &info.sni
            && sni != host
        {
            // Our certs are single-name; a coalesced h2 connection must not
            // serve another FQDN. 421 makes the browser retry on a new connection.
            return (
                pages::error(
                    req.headers(),
                    StatusCode::MISDIRECTED_REQUEST,
                    &format!("This connection is for {sni}, not {host}."),
                    &[],
                ),
                None,
            );
        }
        let route: Option<Arc<Route>> = self.routes.load().get(host).cloned();
        let Some(route) = route else {
            return (self.not_found(req.headers(), host), None);
        };
        match &route.target {
            Target::Selftest => (pages::text(StatusCode::OK, "lokal ok\n"), None),
            Target::Upstream(up) => {
                let desc = up.authority();
                (self.forward(req, &route, up, host, info).await, Some(desc))
            }
        }
    }

    async fn forward(
        &self,
        mut req: Request<Incoming>,
        route: &Route,
        up: &Upstream,
        host: &str,
        info: &ConnInfo,
    ) -> Response<Body> {
        let upgrade = upgrade_token(req.headers());
        let client_upgrade = upgrade.as_ref().map(|_| hyper::upgrade::on(&mut req));
        let (parts, body) = req.into_parts();
        let original_host = parts
            .headers
            .get(header::HOST)
            .cloned()
            .or_else(|| {
                parts
                    .uri
                    .authority()
                    .and_then(|a| HeaderValue::from_str(a.as_str()).ok())
            })
            .unwrap_or_else(|| HeaderValue::from_str(host).unwrap_or(HeaderValue::from_static("")));
        let path = parts
            .uri
            .path_and_query()
            .map(|p| p.as_str())
            .unwrap_or("/");
        let uri: Uri = match format!("http://{}{}", up.authority(), path).parse() {
            Ok(u) => u,
            Err(e) => {
                return pages::error(
                    &parts.headers,
                    StatusCode::BAD_REQUEST,
                    &format!("bad request path: {e}"),
                    &[],
                );
            }
        };
        let mut headers = parts.headers.clone();
        strip_hop_by_hop(&mut headers);
        if let Some(tok) = &upgrade {
            headers.insert(header::CONNECTION, HeaderValue::from_static("upgrade"));
            headers.insert(header::UPGRADE, tok.clone());
        }
        let host_value = match up.host_header {
            HostHeader::Preserve => original_host.clone(),
            HostHeader::Upstream => {
                HeaderValue::from_str(&up.authority()).unwrap_or_else(|_| original_host.clone())
            }
        };
        headers.insert(header::HOST, host_value);
        let xff = match headers.get("x-forwarded-for").and_then(|v| v.to_str().ok()) {
            Some(prev) if !prev.is_empty() => format!("{prev}, {}", info.peer.ip()),
            _ => info.peer.ip().to_string(),
        };
        if let Ok(v) = HeaderValue::from_str(&xff) {
            headers.insert("x-forwarded-for", v);
        }
        headers.insert("x-forwarded-proto", HeaderValue::from_static("https"));
        headers.insert("x-forwarded-host", original_host);
        headers.insert("x-forwarded-port", HeaderValue::from_static("443"));

        let mut out = Request::new(body);
        *out.method_mut() = parts.method.clone();
        *out.uri_mut() = uri;
        *out.version_mut() = Version::HTTP_11;
        *out.headers_mut() = headers;

        // Only an exchange that reaches the upstream makes the connection busy
        // (SPEC §9.3). The guard lives as long as the upstream response body
        // or the upgraded connection; on an error it drops with this frame,
        // so the error page lokal builds doesn't count.
        let busy = info.slot.as_ref().map(|s| s.proxied());
        let mut resp = match self.client.request(out).await {
            Ok(r) => r,
            Err(e) if upstream_cap_reached(&e) => {
                return pages::error(
                    &parts.headers,
                    StatusCode::SERVICE_UNAVAILABLE,
                    "lokal has too many upstream connections open. Try again in a moment.",
                    &[],
                );
            }
            Err(e) => {
                let detail = {
                    let mut s = e.to_string();
                    let mut src = std::error::Error::source(&e);
                    while let Some(x) = src {
                        s = format!("{s}: {x}");
                        src = x.source();
                    }
                    s
                };
                tracing::warn!(fqdn = %route.fqdn, upstream = %up.authority(), "upstream error: {detail}");
                let msg = if e.is_connect() {
                    format!(
                        "{} → {} is not responding. Is the service running?",
                        route.fqdn,
                        up.authority()
                    )
                } else {
                    format!("{} → {} failed: {detail}", route.fqdn, up.authority())
                };
                return pages::error(&parts.headers, StatusCode::BAD_GATEWAY, &msg, &[]);
            }
        };

        if resp.status() == StatusCode::SWITCHING_PROTOCOLS {
            let Some(client_upgrade) = client_upgrade else {
                return pages::error(
                    &parts.headers,
                    StatusCode::BAD_GATEWAY,
                    "upstream switched protocols unasked",
                    &[],
                );
            };
            let upstream_upgrade = hyper::upgrade::on(&mut resp);
            let fqdn = route.fqdn.clone();
            // An HTTP/1 client socket moves into this task, so its permit does
            // too, and the connection stays busy for as long as it's open. (An
            // h2 connection keeps its socket and its permit.)
            let permit = if parts.version < Version::HTTP_2 {
                info.slot.as_ref().and_then(|s| s.take_permit())
            } else {
                None
            };
            tokio::spawn(async move {
                let _held = (permit, busy);
                match tokio::try_join!(client_upgrade, upstream_upgrade) {
                    Ok((c, u)) => {
                        let mut c = TokioIo::new(c);
                        let mut u = TokioIo::new(u);
                        if let Err(e) = tokio::io::copy_bidirectional(&mut c, &mut u).await {
                            tracing::debug!(fqdn = %fqdn, "upgraded connection ended: {e}");
                        }
                    }
                    Err(e) => tracing::warn!(fqdn = %fqdn, "upgrade failed: {e}"),
                }
            });
            let (rparts, _) = resp.into_parts();
            return Response::from_parts(rparts, pages::empty());
        }

        let (mut rparts, rbody) = resp.into_parts();
        strip_hop_by_hop(&mut rparts.headers);
        rparts.headers.remove(header::STRICT_TRANSPORT_SECURITY);
        let body: Body = BusyBody::new(rbody, busy)
            .map_err(|e| Box::new(e) as super::BoxError)
            .boxed();
        Response::from_parts(rparts, body)
    }

    /// Plain HTTP (port 80): 308 to https for known hosts, else 404.
    pub async fn handle_http(&self, req: Request<Incoming>) -> Response<Body> {
        let host = request_host(&req).unwrap_or_default();
        let known = self.routes.load().get(&host).is_some();
        let path = req
            .uri()
            .path_and_query()
            .map(|p| p.to_string())
            .unwrap_or_else(|| "/".into());
        let resp = if known {
            Response::builder()
                .status(StatusCode::PERMANENT_REDIRECT)
                .header(header::LOCATION, format!("https://{host}{path}"))
                .header(header::CACHE_CONTROL, "no-store")
                .body(pages::empty())
                .expect("valid response")
        } else {
            self.not_found(req.headers(), &host)
        };
        tracing::info!(target: "access", "{} http://{host}{path} {}", req.method(), resp.status().as_u16());
        resp
    }
}

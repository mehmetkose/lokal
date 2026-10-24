//! Echo upstream for tests (SPEC §10.2): every request gets JSON with the
//! marker, method, path, received headers, bound address and body size.
//! `GET /sse` streams 3 events 200ms apart (each carries its send time),
//! `/ws` echoes WebSocket messages, `GET /slow?ms=N` waits first.

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use http::{Request, Response, StatusCode, header};
use http_body_util::{BodyExt, StreamBody, combinators::BoxBody};
use hyper::body::{Frame, Incoming};
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::protocol::Role;

type Body = BoxBody<Bytes, Infallible>;

fn full(s: impl Into<Bytes>) -> Body {
    http_body_util::Full::new(s.into()).boxed()
}

pub fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

async fn handle(req: Request<Incoming>, marker: Arc<String>, bound: SocketAddr) -> Response<Body> {
    let path = req.uri().path().to_string();
    let query = req.uri().query().unwrap_or("").to_string();
    if path == "/ws" && req.headers().contains_key(header::UPGRADE) {
        let Some(key) = req.headers().get("sec-websocket-key").cloned() else {
            return Response::builder()
                .status(400)
                .body(full("no key"))
                .unwrap();
        };
        let accept = tokio_tungstenite::tungstenite::handshake::derive_accept_key(key.as_bytes());
        let mut req = req;
        let on_up = hyper::upgrade::on(&mut req);
        let marker = marker.clone();
        tokio::spawn(async move {
            let Ok(up) = on_up.await else { return };
            let mut ws =
                WebSocketStream::from_raw_socket(TokioIo::new(up), Role::Server, None).await;
            while let Some(Ok(msg)) = ws.next().await {
                if msg.is_text() || msg.is_binary() {
                    let reply = match msg {
                        tokio_tungstenite::tungstenite::Message::Text(t) => {
                            tokio_tungstenite::tungstenite::Message::text(format!(
                                "{}:{}",
                                marker,
                                t.as_str()
                            ))
                        }
                        m => m,
                    };
                    if ws.send(reply).await.is_err() {
                        break;
                    }
                } else if msg.is_close() {
                    break;
                }
            }
        });
        return Response::builder()
            .status(StatusCode::SWITCHING_PROTOCOLS)
            .header(header::CONNECTION, "upgrade")
            .header(header::UPGRADE, "websocket")
            .header("sec-websocket-accept", accept)
            .body(full(""))
            .unwrap();
    }
    if path == "/sse" {
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<Frame<Bytes>, Infallible>>(4);
        tokio::spawn(async move {
            for i in 0..3 {
                if i > 0 {
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
                let ev = format!("id: {i}\ndata: {} {}\n\n", i, now_ms());
                if tx.send(Ok(Frame::data(Bytes::from(ev)))).await.is_err() {
                    return;
                }
            }
        });
        let stream = tokio_stream_from(rx);
        return Response::builder()
            .header(header::CONTENT_TYPE, "text/event-stream")
            .header(header::CACHE_CONTROL, "no-cache")
            .body(BodyExt::boxed(StreamBody::new(stream)))
            .unwrap();
    }
    if path == "/slow" {
        let ms: u64 = query
            .split('&')
            .find_map(|kv| kv.strip_prefix("ms="))
            .and_then(|v| v.parse().ok())
            .unwrap_or(1000);
        tokio::time::sleep(Duration::from_millis(ms)).await;
    }
    let method = req.method().to_string();
    let mut headers = serde_json::Map::new();
    for (k, v) in req.headers() {
        headers.insert(
            k.to_string(),
            serde_json::Value::String(v.to_str().unwrap_or("").to_string()),
        );
    }
    let mut body = req.into_body();
    let mut received: u64 = 0;
    while let Some(Ok(frame)) = body.frame().await {
        if let Some(d) = frame.data_ref() {
            received += d.len() as u64;
        }
    }
    let v = serde_json::json!({
        "marker": marker.as_str(),
        "method": method,
        "path": path,
        "query": query,
        "headers": headers,
        "bound": bound.to_string(),
        "received_bytes": received,
    });
    Response::builder()
        .header(header::CONTENT_TYPE, "application/json")
        .body(full(format!("{v}\n")))
        .unwrap()
}

fn tokio_stream_from<T: Send + 'static>(
    mut rx: tokio::sync::mpsc::Receiver<T>,
) -> impl futures_util::Stream<Item = T> + Send + 'static {
    futures_util::stream::poll_fn(move |cx| rx.poll_recv(cx))
}

/// Serves until the task is dropped.
pub async fn serve(listener: TcpListener, marker: String) {
    let marker = Arc::new(marker);
    let bound = listener.local_addr().expect("bound");
    loop {
        let Ok((s, _)) = listener.accept().await else {
            continue;
        };
        let _ = s.set_nodelay(true);
        let marker = marker.clone();
        tokio::spawn(async move {
            let svc = service_fn(move |req| {
                let marker = marker.clone();
                async move { Ok::<_, Infallible>(handle(req, marker, bound).await) }
            });
            let _ = hyper::server::conn::http1::Builder::new()
                .keep_alive(true)
                .serve_connection(TokioIo::new(s), svc)
                .with_upgrades()
                .await;
        });
    }
}

/// Binds `ip:0` (or `ip:port`) and serves in the background; returns the port.
pub async fn spawn(ip: std::net::IpAddr, port: u16, marker: &str) -> std::io::Result<u16> {
    let l = TcpListener::bind(SocketAddr::new(ip, port)).await?;
    let port = l.local_addr()?.port();
    tokio::spawn(serve(l, marker.to_string()));
    Ok(port)
}

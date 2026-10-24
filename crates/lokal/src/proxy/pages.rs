//! Tiny inline error pages (HTML, or JSON with `Accept: application/json`).

use bytes::Bytes;
use http::{HeaderMap, Response, StatusCode, header};
use http_body_util::{BodyExt, Full};

use super::Body;

pub fn full(data: impl Into<Bytes>) -> Body {
    Full::new(data.into())
        .map_err(|never| match never {})
        .boxed()
}

pub fn empty() -> Body {
    full(Bytes::new())
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn wants_json(headers: &HeaderMap) -> bool {
    headers
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.contains("application/json"))
}

/// `links`: `(url, note)` pairs listed under the message.
pub fn error(
    req_headers: &HeaderMap,
    status: StatusCode,
    message: &str,
    links: &[(String, String)],
) -> Response<Body> {
    let (ctype, body) = if wants_json(req_headers) {
        let urls: Vec<serde_json::Value> = links
            .iter()
            .map(|(u, n)| serde_json::json!({ "url": u, "upstream": n }))
            .collect();
        let v = serde_json::json!({
            "error": status.canonical_reason().unwrap_or("error"),
            "status": status.as_u16(),
            "message": message,
            "urls": urls,
        });
        ("application/json", format!("{v}\n"))
    } else {
        let mut list = String::new();
        if !links.is_empty() {
            list.push_str("<ul>");
            for (u, n) in links {
                list.push_str(&format!(
                    "<li><a href=\"{u}\">{u}</a> → {n}</li>",
                    u = escape(u),
                    n = escape(n)
                ));
            }
            list.push_str("</ul>");
        }
        let title = format!(
            "{} {}",
            status.as_u16(),
            status.canonical_reason().unwrap_or("")
        );
        (
            "text/html; charset=utf-8",
            format!(
                "<!doctype html><html><head><meta charset=\"utf-8\"><title>{t}</title>\
<style>body{{font:15px/1.5 -apple-system,system-ui,sans-serif;max-width:40em;margin:3em auto;padding:0 1em;color:#222}}\
@media(prefers-color-scheme:dark){{body{{background:#111;color:#ddd}}a{{color:#8ab4f8}}}}\
h1{{font-size:1.2em}}small{{color:#888}}</style></head><body><h1>{t}</h1><p>{m}</p>{list}\
<p><small>lokal</small></p></body></html>\n",
                t = escape(&title),
                m = escape(message),
            ),
        )
    };
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, ctype)
        .header(header::CACHE_CONTROL, "no-store")
        .body(full(body))
        .expect("valid response")
}

pub fn text(status: StatusCode, body: &'static str) -> Response<Body> {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .header(header::CACHE_CONTROL, "no-store")
        .body(full(body))
        .expect("valid response")
}

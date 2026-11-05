//! Tiny inline error pages (HTML, or JSON with `Accept: application/json`).

use bytes::Bytes;
use http::{HeaderMap, Response, StatusCode, header};
use http_body_util::{BodyExt, Full};

use super::Body;
use super::routes::Link;

fn full(data: Bytes) -> Body {
    Full::new(data).map_err(|never| match never {}).boxed()
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

/// How [`error`] writes its page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Html,
    Json,
}

impl Format {
    /// JSON for a request whose `Accept` names `application/json`, else HTML.
    pub fn of(req_headers: &HeaderMap) -> Self {
        let wants_json = req_headers
            .get(header::ACCEPT)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.contains("application/json"));
        if wants_json { Self::Json } else { Self::Html }
    }
}

/// `links` are listed under the message.
pub fn error(format: Format, status: StatusCode, message: &str, links: &[Link]) -> Response<Body> {
    let (ctype, body) = if format == Format::Json {
        let urls: Vec<serde_json::Value> = links
            .iter()
            .map(|l| serde_json::json!({ "url": l.url, "upstream": l.upstream }))
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
            for l in links {
                let (u, n) = (escape(&l.url), escape(&l.upstream));
                // A wildcard service's URL names no single page: text.
                let item = if l.opens() {
                    format!("<li><a href=\"{u}\">{u}</a> → {n}</li>")
                } else {
                    format!("<li>{u} → {n}</li>")
                };
                list.push_str(&item);
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
        .body(full(Bytes::from(body)))
        .expect("a status code and constant headers make a valid response")
}

pub fn text(status: StatusCode, body: &'static str) -> Response<Body> {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .header(header::CACHE_CONTROL, "no-store")
        .body(full(Bytes::from(body)))
        .expect("a status code and constant headers make a valid response")
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn body(r: Response<Body>) -> String {
        let b = r.into_body().collect().await.unwrap().to_bytes();
        String::from_utf8(b.to_vec()).unwrap()
    }

    /// The 404 page links each service's URL, but shows a wildcard
    /// service's as text: `https://*.p.lokal` names no single page.
    #[tokio::test]
    async fn a_wildcard_is_listed_as_text() {
        let links = [
            Link {
                url: "https://api.p.lokal".into(),
                upstream: "localhost:4000".into(),
            },
            Link {
                url: "https://*.p.lokal".into(),
                upstream: "localhost:3000".into(),
            },
        ];
        let html = body(error(Format::Html, StatusCode::NOT_FOUND, "m", &links)).await;
        assert!(
            html.contains(
                "<li><a href=\"https://api.p.lokal\">https://api.p.lokal</a> → localhost:4000</li>\
<li>https://*.p.lokal → localhost:3000</li>"
            ),
            "{html}"
        );
        let json: serde_json::Value = serde_json::from_str(
            &body(error(Format::Json, StatusCode::NOT_FOUND, "m", &links)).await,
        )
        .unwrap();
        assert_eq!(json["urls"][1]["url"], "https://*.p.lokal");
    }
}

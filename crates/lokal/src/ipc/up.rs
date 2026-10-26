//! The socket of a running `lokal up` (SPEC §4.11, "Restarting a
//! service"): newline-delimited JSON over `<up dir>/up.sock` (0600), one
//! request per connection, as with the daemon's (§8.4). The answer is a
//! stream of [`Event`]s, one per line, and ends when `up` closes the
//! connection. The only request is a restart of services in the file `up`
//! started with: nothing on the socket can run another command, change a
//! route or stop `up` (§16.12).

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "op", rename_all = "lowercase")]
pub enum Request {
    /// Restart these services (subdomains, as the file normalises them),
    /// or start them if they aren't running.
    Restart { services: Vec<String> },
}

/// One line of `up`'s answer. Each service named gets `restarting` first,
/// then any `hint`s, then one of `ready`, `exited` or `busy`; a request
/// `up` refuses gets one `error` instead.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "event", rename_all = "lowercase")]
pub enum Event {
    /// The restart began. `file_changed`: `lokal.toml`'s bytes no longer
    /// hash to the digest `up` started with; the restart still runs the
    /// command `up` started with.
    Restarting { service: String, file_changed: bool },
    /// A hint `up` printed about the new run: the wrong-port hint, the
    /// port-in-use hint, or that its server detached.
    Hint { service: String, text: String },
    /// Its new run accepts connections on `port`, `ms` after the request.
    /// `url` is its URL, when the routes took when `up` started (§4.11,
    /// "The ready line").
    Ready {
        service: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        url: Option<String>,
        port: u16,
        ms: u64,
    },
    /// Its new run exited with `code` before it was ready.
    Exited { service: String, code: i32 },
    /// Something already listens on its port, so `up` didn't start it:
    /// `message` and `hint` are the busy-port error's (§4.11, "Ports"),
    /// `owner` the listeners, as `node (pid 123)`.
    Busy {
        service: String,
        port: u16,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        owner: Option<String>,
        message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        hint: Option<String>,
    },
    /// The request was refused, and nothing restarted. `usage`: a usage
    /// error, such as a service that isn't in the file (exit 2).
    Error {
        message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        hint: Option<String>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        usage: bool,
    },
}

/// Sends `req` to the `lokal up` listening on `socket` and calls `each` with
/// every event, in order, until `up` closes the connection. There's no
/// timeout: a restart takes as long as the old run takes to stop and the
/// new one to get ready.
pub fn call(socket: &Path, req: &Request, mut each: impl FnMut(Event)) -> std::io::Result<()> {
    let mut s = UnixStream::connect(socket)?;
    let mut line = serde_json::to_string(req)?;
    line.push('\n');
    s.write_all(line.as_bytes())?;
    for line in BufReader::new(&s).lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        each(parse(&line).map_err(std::io::Error::other)?);
    }
    Ok(())
}

fn parse(line: &str) -> Result<Event> {
    serde_json::from_str(line).with_context(|| format!("bad answer from lokal up: {line}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_format() {
        assert_eq!(
            serde_json::to_string(&Request::Restart {
                services: vec!["web".into()]
            })
            .unwrap(),
            r#"{"op":"restart","services":["web"]}"#
        );
        let lines = [
            (
                Event::Restarting {
                    service: "web".into(),
                    file_changed: false,
                },
                r#"{"event":"restarting","service":"web","file_changed":false}"#,
            ),
            (
                Event::Hint {
                    service: "web".into(),
                    text: "…".into(),
                },
                r#"{"event":"hint","service":"web","text":"…"}"#,
            ),
            (
                Event::Ready {
                    service: "web".into(),
                    url: Some("https://web.shop.lokal".into()),
                    port: 51234,
                    ms: 1234,
                },
                r#"{"event":"ready","service":"web","url":"https://web.shop.lokal","port":51234,"ms":1234}"#,
            ),
            (
                Event::Ready {
                    service: "web".into(),
                    url: None,
                    port: 51234,
                    ms: 7,
                },
                r#"{"event":"ready","service":"web","port":51234,"ms":7}"#,
            ),
            (
                Event::Exited {
                    service: "web".into(),
                    code: 1,
                },
                r#"{"event":"exited","service":"web","code":1}"#,
            ),
            (
                Event::Busy {
                    service: "web".into(),
                    port: 9902,
                    owner: Some("node (pid 123)".into()),
                    message: "m".into(),
                    hint: Some("h".into()),
                },
                r#"{"event":"busy","service":"web","port":9902,"owner":"node (pid 123)","message":"m","hint":"h"}"#,
            ),
            (
                Event::Error {
                    message: "lokal up is stopping".into(),
                    hint: None,
                    usage: false,
                },
                r#"{"event":"error","message":"lokal up is stopping"}"#,
            ),
            (
                Event::Error {
                    message: "no service `x`".into(),
                    hint: Some("its services: a, b".into()),
                    usage: true,
                },
                r#"{"event":"error","message":"no service `x`","hint":"its services: a, b","usage":true}"#,
            ),
        ];
        for (ev, json) in lines {
            assert_eq!(serde_json::to_string(&ev).unwrap(), json);
            assert_eq!(parse(json).unwrap(), ev);
        }
        // A field a newer lokal adds doesn't break an older client.
        assert_eq!(
            parse(r#"{"event":"exited","service":"a","code":3,"more":1}"#).unwrap(),
            Event::Exited {
                service: "a".into(),
                code: 3
            }
        );
    }
}

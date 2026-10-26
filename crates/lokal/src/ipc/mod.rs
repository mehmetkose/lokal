//! IPC between the CLI and the daemon (SPEC §8.4): newline-delimited JSON
//! over the Unix socket `<base>/lokal.sock` (0600), one request per
//! connection. [`up`] is the same for a running `lokal up` (§4.11).

pub mod up;

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};

use crate::paths::Paths;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "op", rename_all = "lowercase")]
pub enum Request {
    Ping,
    Status,
    Reload,
    Selftest { action: SelftestAction },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SelftestAction {
    Begin,
    End,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ping {
    pub ok: bool,
    pub version: String,
    pub build_id: String,
    pub pid: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NameStatus {
    pub fqdn: String,
    pub state: String,
}

/// The `status --json` body (SPEC §4.8).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Status {
    pub schema: u32,
    pub setup_done: bool,
    pub running: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uptime_secs: Option<u64>,
    #[serde(default)]
    pub listeners: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listener_source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name_backend: Option<String>,
    #[serde(default)]
    pub names: Vec<NameStatus>,
    #[serde(default)]
    pub certs_cached: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Failed {
    pub fqdn: String,
    pub error: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Reload {
    pub ok: bool,
    pub ms: u64,
    #[serde(default)]
    pub ready: Vec<String>,
    #[serde(default)]
    pub removed: Vec<String>,
    #[serde(default)]
    pub failed: Vec<Failed>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Selftest {
    pub ok: bool,
    /// The first selftest name (kept for older CLIs).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fqdn: Option<String>,
    /// One selftest name per TLD.
    #[serde(default)]
    pub fqdns: Vec<String>,
    pub ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Why a request didn't get an answer.
#[derive(Debug)]
pub enum IpcError {
    /// No daemon listening (no socket, or connection refused).
    NotRunning,
    Other(anyhow::Error),
}

impl std::fmt::Display for IpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IpcError::NotRunning => f.write_str("the background service isn't running"),
            IpcError::Other(e) => write!(f, "{e:#}"),
        }
    }
}

impl std::error::Error for IpcError {}

/// Sends `req` and parses the single-line JSON response.
pub fn call<T: for<'de> Deserialize<'de>>(
    paths: &Paths,
    req: &Request,
    timeout: Duration,
) -> std::result::Result<T, IpcError> {
    let mut s = match UnixStream::connect(&paths.socket) {
        Ok(s) => s,
        Err(e)
            if matches!(
                e.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
            ) =>
        {
            return Err(IpcError::NotRunning);
        }
        Err(e) => {
            return Err(IpcError::Other(anyhow!(
                "cannot connect to {}: {e}",
                paths.socket.display()
            )));
        }
    };
    let mut inner = || -> Result<T> {
        s.set_read_timeout(Some(timeout))?;
        s.set_write_timeout(Some(timeout))?;
        let mut line = serde_json::to_string(req)?;
        line.push('\n');
        s.write_all(line.as_bytes())?;
        let mut resp = String::new();
        BufReader::new(&s)
            .read_line(&mut resp)
            .context("no answer from the background service")?;
        if resp.trim().is_empty() {
            return Err(anyhow!("the background service closed the connection"));
        }
        serde_json::from_str(&resp)
            .with_context(|| format!("bad answer from the background service: {resp}"))
    };
    inner().map_err(IpcError::Other)
}

pub fn ping(paths: &Paths, timeout: Duration) -> std::result::Result<Ping, IpcError> {
    call(paths, &Request::Ping, timeout)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_format() {
        assert_eq!(
            serde_json::to_string(&Request::Ping).unwrap(),
            r#"{"op":"ping"}"#
        );
        assert_eq!(
            serde_json::to_string(&Request::Reload).unwrap(),
            r#"{"op":"reload"}"#
        );
        assert_eq!(
            serde_json::to_string(&Request::Selftest {
                action: SelftestAction::Begin
            })
            .unwrap(),
            r#"{"op":"selftest","action":"begin"}"#
        );
        let r: Request = serde_json::from_str(r#"{"op":"selftest","action":"end"}"#).unwrap();
        assert_eq!(
            r,
            Request::Selftest {
                action: SelftestAction::End
            }
        );
    }
}

//! IPC server: newline-delimited JSON on `<base>/lokal.sock` (0600), one
//! request per connection.

use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use tokio::sync::watch;

use super::Core;
use crate::ipc::Request;

pub fn bind(core: &Core) -> Result<UnixListener> {
    let path = &core.paths.socket;
    // We hold daemon.lock, so any existing socket file is stale.
    let _ = std::fs::remove_file(path);
    let l = UnixListener::bind(path).with_context(|| format!("cannot bind {}", path.display()))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(l)
}

pub async fn serve(listener: UnixListener, core: Arc<Core>, mut shutdown: watch::Receiver<bool>) {
    loop {
        let stream = tokio::select! {
            r = listener.accept() => match r {
                Ok((s, _)) => s,
                Err(e) => {
                    tracing::warn!("IPC accept: {e}");
                    continue;
                }
            },
            _ = shutdown.changed() => break,
        };
        let core = core.clone();
        tokio::spawn(async move {
            let (r, mut w) = stream.into_split();
            let mut line = String::new();
            if BufReader::new(r).read_line(&mut line).await.is_err() {
                return;
            }
            let resp = match serde_json::from_str::<Request>(line.trim()) {
                Ok(req) => core.handle(req).await,
                Err(e) => serde_json::json!({ "ok": false, "error": format!("bad request: {e}") }),
            };
            let mut out = resp.to_string();
            out.push('\n');
            let _ = w.write_all(out.as_bytes()).await;
            let _ = w.shutdown().await;
        });
    }
}

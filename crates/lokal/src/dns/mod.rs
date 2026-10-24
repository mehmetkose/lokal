//! Built-in DNS server for the `.lokal` TLD. The OS routes `*.lokal` lookups
//! to it (on macOS via `/etc/resolver/lokal`, installed by `setup`).
//! mDNSResponder only publishes LocalOnly records under `.local`, so
//! `.lokal` needs its own server.
//!
//! Every answer has TTL 0 and every negative answer carries an SOA with
//! minimum 0: nothing is cached, so names appear and vanish the moment a
//! reload returns. Only A (127.0.0.1) and AAAA (::1) are answered.

pub mod wire;

use std::collections::{BTreeSet, HashSet};
use std::sync::Arc;

use arc_swap::ArcSwap;
use tokio::net::UdpSocket;

use crate::platform::{BoxFuture, NamePublisher, NameResult, NameStates};

/// The TLD this server is authoritative for.
pub const ZONE: &str = "lokal";
/// Fixed loopback port (it is written into the OS resolver config).
pub const DEFAULT_PORT: u16 = 53535;

/// The names the server answers for, swapped atomically on reload.
pub type ZoneNames = Arc<ArcSwap<HashSet<String>>>;

pub fn in_zone(fqdn: &str) -> bool {
    fqdn == ZONE || fqdn.ends_with(&format!(".{ZONE}"))
}

/// Serves DNS on `sock` until the task is dropped.
pub async fn serve(sock: UdpSocket, names: ZoneNames) {
    let mut buf = vec![0u8; 4096];
    loop {
        let (n, peer) = match sock.recv_from(&mut buf).await {
            Ok(x) => x,
            Err(e) => {
                tracing::warn!("dns recv: {e}");
                continue;
            }
        };
        let current = names.load();
        if let Some(resp) = wire::answer(&buf[..n], ZONE, &current)
            && let Err(e) = sock.send_to(&resp, peer).await
        {
            tracing::debug!(%peer, "dns send: {e}");
        }
    }
}

/// Publishes `.lokal` names by putting them in the server's zone. Ready at
/// once: the server answers from this set, with nothing cached anywhere.
pub struct DnsPublisher {
    names: ZoneNames,
    states: NameStates,
}

impl DnsPublisher {
    pub fn new(names: ZoneNames) -> Self {
        Self {
            names,
            states: NameStates::default(),
        }
    }
}

impl NamePublisher for DnsPublisher {
    fn backend_name(&self) -> &'static str {
        "dns"
    }

    fn apply<'a>(&'a mut self, fqdns: &'a BTreeSet<String>) -> BoxFuture<'a, Vec<NameResult>> {
        Box::pin(async move {
            self.names.store(Arc::new(fqdns.iter().cloned().collect()));
            let mut st = self.states.lock().expect("lock");
            st.clear();
            fqdns
                .iter()
                .map(|f| {
                    st.insert(f.clone(), "registered".into());
                    NameResult {
                        fqdn: f.clone(),
                        outcome: Ok(()),
                    }
                })
                .collect()
        })
    }

    fn clear(&mut self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            self.names.store(Arc::new(HashSet::new()));
            self.states.lock().expect("lock").clear();
        })
    }

    fn states(&self) -> NameStates {
        self.states.clone()
    }
}

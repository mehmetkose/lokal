//! Built-in DNS server for the `.lokal` TLD, the only one lokal serves
//! (SPEC §7). The OS routes `*.lokal` lookups to it (on macOS via
//! `/etc/resolver/lokal`, on Linux via a systemd-resolved DNS delegate,
//! both installed by `setup`).
//!
//! Every answer has TTL 0 and every negative answer carries an SOA with
//! minimum 0: nothing is cached, so names appear and vanish the moment a
//! reload returns. Only A (127.0.0.1) and AAAA (::1) are answered.

pub mod wire;

use std::collections::{BTreeSet, HashSet};
use std::sync::Arc;

use arc_swap::ArcSwap;
use tokio::net::UdpSocket;

/// The TLD this server is authoritative for.
pub const ZONE: &str = "lokal";
/// How names are published, for `status` (`name_backend`).
pub const BACKEND: &str = "dns";
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
}

impl DnsPublisher {
    pub fn new(names: ZoneNames) -> Self {
        Self { names }
    }

    /// Answer for exactly `fqdns` from now on.
    pub fn apply(&self, fqdns: &BTreeSet<String>) {
        self.names.store(Arc::new(fqdns.iter().cloned().collect()));
    }

    /// Answer for no name.
    pub fn clear(&self) {
        self.names.store(Arc::new(HashSet::new()));
    }

    /// The names it answers for, in order, for `status`.
    pub fn names(&self) -> BTreeSet<String> {
        self.names.load().iter().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publisher_swaps_the_zone() {
        let zone = ZoneNames::default();
        let p = DnsPublisher::new(zone.clone());
        let want = BTreeSet::from(["a.x.lokal".to_string(), "x.lokal".to_string()]);
        p.apply(&want);
        assert_eq!(p.names(), want);
        assert!(zone.load().contains("a.x.lokal"));
        p.apply(&BTreeSet::from(["x.lokal".to_string()]));
        assert!(!zone.load().contains("a.x.lokal"));
        p.clear();
        assert!(zone.load().is_empty());
    }
}

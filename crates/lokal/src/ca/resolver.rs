//! On-demand leaf certificates for rustls (SPEC §6.2): only configured
//! FQDNs (plus the active selftest name) get a certificate; anything else,
//! or no SNI, fails the handshake. Leaves are cached in memory and
//! pre-warmed on reload.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime};

use arc_swap::ArcSwap;
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;

use super::LeafIssuer;

/// Re-issue when fewer than this remain.
const RENEW_BEFORE: Duration = Duration::from_secs(30 * 86_400);

pub struct CertStore {
    issuer: LeafIssuer,
    allowed: ArcSwap<HashSet<String>>,
    cache: RwLock<HashMap<String, (Arc<CertifiedKey>, SystemTime)>>,
}

impl std::fmt::Debug for CertStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CertStore").finish_non_exhaustive()
    }
}

impl CertStore {
    pub fn new(issuer: LeafIssuer) -> Self {
        Self {
            issuer,
            allowed: ArcSwap::from_pointee(HashSet::new()),
            cache: RwLock::new(HashMap::new()),
        }
    }

    pub fn set_allowed(&self, names: HashSet<String>) {
        self.allowed.store(Arc::new(names));
    }

    pub fn is_allowed(&self, fqdn: &str) -> bool {
        self.allowed.load().contains(fqdn)
    }

    pub fn cached_count(&self) -> usize {
        self.cache.read().map(|c| c.len()).unwrap_or(0)
    }

    /// Drops cached leaves that are no longer allowed.
    pub fn evict_unallowed(&self) {
        let allowed = self.allowed.load();
        if let Ok(mut c) = self.cache.write() {
            c.retain(|k, _| allowed.contains(k));
        }
    }

    fn fresh(&self, fqdn: &str) -> Option<Arc<CertifiedKey>> {
        let c = self.cache.read().ok()?;
        let (ck, exp) = c.get(fqdn)?;
        let ok = exp
            .duration_since(SystemTime::now())
            .map(|left| left > RENEW_BEFORE)
            .unwrap_or(false);
        ok.then(|| ck.clone())
    }

    /// Returns the cached leaf or issues one. Does not check `allowed`.
    pub fn get_or_issue(&self, fqdn: &str) -> anyhow::Result<Arc<CertifiedKey>> {
        if let Some(ck) = self.fresh(fqdn) {
            return Ok(ck);
        }
        let (ck, exp) = self.issuer.certified_key(fqdn)?;
        if let Ok(mut c) = self.cache.write() {
            c.insert(fqdn.to_string(), (ck.clone(), exp));
        }
        Ok(ck)
    }

    /// Issues leaves for `names` in parallel. Returns per-name errors.
    pub fn prewarm(&self, names: &[String]) -> Vec<(String, String)> {
        let todo: Vec<&String> = names.iter().filter(|n| self.fresh(n).is_none()).collect();
        if todo.is_empty() {
            return vec![];
        }
        let threads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
            .min(todo.len());
        let chunk = todo.len().div_ceil(threads);
        std::thread::scope(|s| {
            let handles: Vec<_> = todo
                .chunks(chunk)
                .map(|part| {
                    s.spawn(move || {
                        part.iter()
                            .filter_map(|n| {
                                self.get_or_issue(n)
                                    .err()
                                    .map(|e| ((*n).clone(), format!("certificate: {e:#}")))
                            })
                            .collect::<Vec<_>>()
                    })
                })
                .collect();
            handles
                .into_iter()
                .flat_map(|h| h.join().unwrap_or_default())
                .collect()
        })
    }
}

#[derive(Debug)]
pub struct Resolver(pub Arc<CertStore>);

impl ResolvesServerCert for Resolver {
    fn resolve(&self, hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        let sni = hello
            .server_name()?
            .trim_end_matches('.')
            .to_ascii_lowercase();
        if !self.0.is_allowed(&sni) {
            tracing::debug!(sni = %sni, "TLS: unknown name, refusing handshake");
            return None;
        }
        match self.0.get_or_issue(&sni) {
            Ok(ck) => Some(ck),
            Err(e) => {
                tracing::error!(sni = %sni, "cannot issue certificate: {e:#}");
                None
            }
        }
    }
}

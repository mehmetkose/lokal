//! Leaf certificates for rustls (SPEC §6.2): only configured FQDNs, names
//! under a configured wildcard service, names a configured route covers,
//! and the active selftest name get a certificate; anything else, or no
//! SNI, fails the handshake. Every leaf names exactly one FQDN. Leaves for
//! configured FQDNs are pre-warmed on reload and cached; leaves for names
//! under a wildcard service or a route are issued on the first handshake
//! and kept in a cache of their own, capped at [`ON_DEMAND_CAP`].

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, SystemTime};

use arc_swap::ArcSwap;
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;

use super::LeafIssuer;
use crate::state::validate;

/// Re-issue when fewer than this remain.
const RENEW_BEFORE: Duration = Duration::from_secs(30 * 86_400);

/// How many leaves for names under wildcard services and routes are kept.
/// When it's full, the oldest-issued one goes first: issuing again costs
/// about 1ms, so there's no LRU bookkeeping.
pub const ON_DEMAND_CAP: usize = 1024;

/// A leaf and when it expires.
type Entry = (Arc<CertifiedKey>, SystemTime);

/// The names the resolver serves.
#[derive(Debug, Default)]
pub struct Allowed {
    /// Configured FQDNs and the active selftest name.
    pub exact: HashSet<String>,
    /// Wildcard services' and routes' patterns (`*.shop.lokal`,
    /// `{site}.shop.lokal`): every name one of them covers
    /// ([`validate::covers`]) is served too.
    pub patterns: Vec<String>,
}

impl Allowed {
    pub fn new(
        exact: impl IntoIterator<Item = String>,
        patterns: impl IntoIterator<Item = String>,
    ) -> Self {
        Self {
            exact: exact.into_iter().collect(),
            patterns: patterns.into_iter().collect(),
        }
    }

    /// Is `fqdn` under a wildcard service, or covered by a route?
    fn covered(&self, fqdn: &str) -> bool {
        self.patterns.iter().any(|p| validate::covers(p, fqdn))
    }
}

/// Which cache a served name's leaf lives in.
enum Kind {
    /// A configured FQDN: pre-warmed, kept while it's configured.
    Exact,
    /// A name under a wildcard service or a route: issued on the first
    /// handshake.
    OnDemand,
}

/// Leaves for names under wildcard services and routes, in the order they
/// were issued.
#[derive(Default)]
struct OnDemand {
    leaves: HashMap<String, Entry>,
    /// Oldest first; each name in `leaves` once.
    order: VecDeque<String>,
    /// The daemon has logged that the cache is full: once per daemon run.
    logged_full: bool,
}

impl OnDemand {
    fn fresh(&self, fqdn: &str) -> Option<Arc<CertifiedKey>> {
        let (ck, exp) = self.leaves.get(fqdn)?;
        is_fresh(*exp).then(|| ck.clone())
    }

    /// Adds a leaf, then drops the oldest-issued ones beyond `cap`. Returns
    /// how many it dropped.
    fn insert(&mut self, fqdn: &str, entry: Entry, cap: usize) -> usize {
        if self.leaves.insert(fqdn.to_string(), entry).is_some() {
            // Issued again (it was about to expire): it's the newest now.
            self.order.retain(|n| n != fqdn);
        }
        self.order.push_back(fqdn.to_string());
        let mut dropped = 0;
        while self.leaves.len() > cap {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            self.leaves.remove(&oldest);
            dropped += 1;
        }
        dropped
    }

    fn retain(&mut self, keep: impl Fn(&str) -> bool) {
        self.leaves.retain(|k, _| keep(k));
        let leaves = &self.leaves;
        self.order.retain(|k| leaves.contains_key(k));
    }
}

fn is_fresh(exp: SystemTime) -> bool {
    exp.duration_since(SystemTime::now())
        .map(|left| left > RENEW_BEFORE)
        .unwrap_or(false)
}

pub struct CertStore {
    issuer: LeafIssuer,
    allowed: ArcSwap<Allowed>,
    cache: RwLock<HashMap<String, Entry>>,
    on_demand: Mutex<OnDemand>,
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
            allowed: ArcSwap::from_pointee(Allowed::default()),
            cache: RwLock::new(HashMap::new()),
            on_demand: Mutex::new(OnDemand::default()),
        }
    }

    pub fn set_allowed(&self, allowed: Allowed) {
        self.allowed.store(Arc::new(allowed));
    }

    /// Which cache serves `fqdn`, if it's served at all. A configured FQDN
    /// beats a wildcard that also covers it.
    fn kind(&self, fqdn: &str) -> Option<Kind> {
        let allowed = self.allowed.load();
        if allowed.exact.contains(fqdn) {
            Some(Kind::Exact)
        } else if allowed.covered(fqdn) {
            Some(Kind::OnDemand)
        } else {
            None
        }
    }

    pub fn is_allowed(&self, fqdn: &str) -> bool {
        self.kind(fqdn).is_some()
    }

    /// Leaves in both caches.
    pub fn cached_count(&self) -> usize {
        let exact = self.cache.read().map(|c| c.len()).unwrap_or(0);
        let on_demand = self.on_demand.lock().map(|c| c.leaves.len()).unwrap_or(0);
        exact + on_demand
    }

    /// Drops cached leaves that are no longer allowed: a configured FQDN
    /// that's gone, and a name that no wildcard service or route covers any
    /// more (or that has its own service now, whose leaf is pre-warmed).
    pub fn evict_unallowed(&self) {
        let allowed = self.allowed.load();
        if let Ok(mut c) = self.cache.write() {
            c.retain(|k, _| allowed.exact.contains(k));
        }
        if let Ok(mut c) = self.on_demand.lock() {
            c.retain(|k| allowed.covered(k) && !allowed.exact.contains(k));
        }
    }

    fn fresh(&self, fqdn: &str) -> Option<Arc<CertifiedKey>> {
        let c = self.cache.read().ok()?;
        let (ck, exp) = c.get(fqdn)?;
        is_fresh(*exp).then(|| ck.clone())
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

    /// Returns the cached leaf for a name a pattern covers, or
    /// issues one (about 1ms) into the capped on-demand cache. Does not
    /// check `allowed`. The lock isn't held while signing, so two first
    /// handshakes for one name may both sign; the second leaf wins.
    pub fn get_or_issue_on_demand(&self, fqdn: &str) -> anyhow::Result<Arc<CertifiedKey>> {
        if let Some(ck) = self.on_demand.lock().ok().and_then(|c| c.fresh(fqdn)) {
            return Ok(ck);
        }
        let (ck, exp) = self.issuer.certified_key(fqdn)?;
        let first_full = self.on_demand.lock().is_ok_and(|mut c| {
            let dropped = c.insert(fqdn, (ck.clone(), exp), ON_DEMAND_CAP);
            dropped > 0 && !std::mem::replace(&mut c.logged_full, true)
        });
        if first_full {
            tracing::info!(
                cap = ON_DEMAND_CAP,
                "TLS: the cache for names under wildcard services and routes is full; each new name now drops \
the oldest-issued leaf, which is issued again (about 1ms) if that name comes back. Logged once per \
daemon run"
            );
        }
        Ok(ck)
    }

    /// The leaf for a served name, from the cache it belongs in. `None` for
    /// a name that isn't served.
    fn leaf(&self, fqdn: &str) -> Option<anyhow::Result<Arc<CertifiedKey>>> {
        match self.kind(fqdn)? {
            Kind::Exact => Some(self.get_or_issue(fqdn)),
            Kind::OnDemand => Some(self.get_or_issue_on_demand(fqdn)),
        }
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
        let Some(leaf) = self.0.leaf(&sni) else {
            tracing::debug!(sni = %sni, "TLS: unknown name, refusing handshake");
            return None;
        };
        match leaf {
            Ok(ck) => Some(ck),
            Err(e) => {
                tracing::error!(sni = %sni, "cannot issue certificate: {e:#}");
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use x509_parser::extensions::{GeneralName, ParsedExtension};

    use super::*;
    use crate::ca::CaCert;
    use crate::paths::Paths;

    /// A store with an in-memory CA (nothing is written).
    fn store() -> CertStore {
        let paths = Paths::sandbox(PathBuf::from("/nonexistent/lokal-resolver-test"));
        let ca = CaCert::preview(&paths, "u", "h").unwrap();
        CertStore::new(ca.issuer().unwrap())
    }

    fn sans(ck: &CertifiedKey) -> Vec<String> {
        let (_, c) = x509_parser::parse_x509_certificate(&ck.cert[0]).unwrap();
        let mut v = vec![];
        for ext in c.extensions() {
            if let ParsedExtension::SubjectAlternativeName(san) = ext.parsed_extension() {
                for n in &san.general_names {
                    match n {
                        GeneralName::DNSName(d) => v.push(d.to_string()),
                        other => panic!("unexpected SAN {other:?}"),
                    }
                }
            }
        }
        v
    }

    /// A name under a wildcard service gets a normal leaf naming exactly
    /// that name, never the pattern (SPEC §6.2, §16.3).
    #[test]
    fn an_on_demand_leaf_names_exactly_one_fqdn() {
        let s = store();
        s.set_allowed(Allowed::new([], ["*.p.lokal".to_string()]));
        assert!(s.is_allowed("x.p.lokal"));
        assert!(s.is_allowed("b.a.p.lokal"));
        assert!(!s.is_allowed("p.lokal"));
        assert!(!s.is_allowed("*.p.lokal"));
        assert!(!s.is_allowed("x.q.lokal"));
        assert!(s.leaf("x.q.lokal").is_none());
        let ck = s.leaf("x.p.lokal").unwrap().unwrap();
        assert_eq!(sans(&ck), ["x.p.lokal"]);
        let (_, c) = x509_parser::parse_x509_certificate(&ck.cert[0]).unwrap();
        let cn = c.subject().iter_common_name().next().unwrap();
        assert_eq!(cn.as_str().unwrap(), "x.p.lokal");
        // The second handshake reuses it, from the on-demand cache.
        let again = s.leaf("x.p.lokal").unwrap().unwrap();
        assert!(Arc::ptr_eq(&ck, &again));
        assert_eq!(s.cached_count(), 1);
        assert!(s.cache.read().unwrap().is_empty());
        // The pattern itself is never signed, whoever asks.
        assert!(s.get_or_issue_on_demand("*.p.lokal").is_err());
        assert!(s.get_or_issue("*.p.lokal").is_err());
    }

    /// A name a route covers gets a leaf naming exactly that name; the
    /// route's pattern never does (SPEC §6.2, §16.3).
    #[test]
    fn a_name_a_route_covers_gets_its_own_leaf() {
        let s = store();
        s.set_allowed(Allowed::new(
            [],
            [
                "{site}.p.lokal".to_string(),
                "preview.{slug}.q.lokal".to_string(),
            ],
        ));
        for (name, allowed) in [
            ("acme.com.p.lokal", true),
            ("x.p.lokal", true),
            ("p.lokal", false),
            ("preview.x.q.lokal", true),
            ("preview.a.b.q.lokal", false),
            ("x.q.lokal", false),
            ("{site}.p.lokal", false),
            ("preview.{slug}.q.lokal", false),
        ] {
            assert_eq!(s.is_allowed(name), allowed, "{name}");
        }
        let ck = s.leaf("acme.com.p.lokal").unwrap().unwrap();
        assert_eq!(sans(&ck), ["acme.com.p.lokal"]);
        assert!(s.leaf("x.q.lokal").is_none());
        for pattern in ["{site}.p.lokal", "preview.{slug}.q.lokal"] {
            assert!(s.get_or_issue_on_demand(pattern).is_err(), "{pattern}");
            assert!(s.get_or_issue(pattern).is_err(), "{pattern}");
        }
    }

    /// A configured FQDN beats a wildcard that covers it: its leaf is the
    /// pre-warmed one.
    #[test]
    fn an_exact_name_uses_the_prewarmed_cache() {
        let s = store();
        s.set_allowed(Allowed::new(
            ["api.p.lokal".to_string()],
            ["*.p.lokal".to_string()],
        ));
        assert!(s.prewarm(&["api.p.lokal".to_string()]).is_empty());
        let pre = s.cache.read().unwrap()["api.p.lokal"].0.clone();
        let ck = s.leaf("api.p.lokal").unwrap().unwrap();
        assert!(Arc::ptr_eq(&pre, &ck));
        assert!(s.on_demand.lock().unwrap().leaves.is_empty());
    }

    /// Full at 1024: the oldest-issued leaf goes first.
    #[test]
    fn the_on_demand_cache_stops_at_its_cap() {
        let s = store();
        let (ck, exp) = s.issuer.certified_key("x.p.lokal").unwrap();
        let mut c = OnDemand::default();
        for i in 0..=ON_DEMAND_CAP {
            c.insert(&format!("n{i}.p.lokal"), (ck.clone(), exp), ON_DEMAND_CAP);
        }
        assert_eq!(c.leaves.len(), ON_DEMAND_CAP);
        assert_eq!(c.order.len(), ON_DEMAND_CAP);
        assert!(!c.leaves.contains_key("n0.p.lokal"));
        assert!(c.leaves.contains_key("n1.p.lokal"));
        assert!(c.leaves.contains_key(&format!("n{ON_DEMAND_CAP}.p.lokal")));
        // A leaf issued again is the newest: the next one in line goes.
        c.insert("n1.p.lokal", (ck.clone(), exp), ON_DEMAND_CAP);
        c.insert("new.p.lokal", (ck.clone(), exp), ON_DEMAND_CAP);
        assert_eq!(c.leaves.len(), ON_DEMAND_CAP);
        assert!(c.leaves.contains_key("n1.p.lokal"));
        assert!(!c.leaves.contains_key("n2.p.lokal"));
        assert_eq!(c.order.back().map(String::as_str), Some("new.p.lokal"));
        assert_eq!(c.order.len(), c.leaves.len());

        // The store uses the same cap.
        for i in 0..=ON_DEMAND_CAP {
            s.on_demand.lock().unwrap().insert(
                &format!("n{i}.p.lokal"),
                (ck.clone(), exp),
                ON_DEMAND_CAP,
            );
        }
        s.set_allowed(Allowed::new([], ["*.p.lokal".to_string()]));
        s.get_or_issue_on_demand("last.p.lokal").unwrap();
        assert_eq!(s.cached_count(), ON_DEMAND_CAP);
    }

    /// Writes what a test's subscriber logs into a shared buffer.
    #[derive(Clone, Default)]
    struct Captured(Arc<Mutex<Vec<u8>>>);

    impl std::io::Write for Captured {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl Captured {
        fn text(&self) -> String {
            String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
        }
    }

    /// The first leaf that pushes another out logs one line, at info so
    /// `lokal logs` shows it; later ones in the same run don't.
    #[test]
    fn a_full_on_demand_cache_is_logged_once() {
        let s = store();
        s.set_allowed(Allowed::new([], ["*.p.lokal".to_string()]));
        let (ck, exp) = s.issuer.certified_key("x.p.lokal").unwrap();
        {
            let mut c = s.on_demand.lock().unwrap();
            for i in 0..ON_DEMAND_CAP - 1 {
                c.insert(&format!("n{i}.p.lokal"), (ck.clone(), exp), ON_DEMAND_CAP);
            }
        }
        let log = Captured::default();
        let writer = log.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::INFO)
            .with_ansi(false)
            .with_writer(move || writer.clone())
            .finish();
        tracing::subscriber::with_default(subscriber, || {
            // Full now, but nothing dropped yet.
            s.get_or_issue_on_demand("last.p.lokal").unwrap();
            assert_eq!(log.text(), "");
            s.get_or_issue_on_demand("one.p.lokal").unwrap();
            s.get_or_issue_on_demand("two.p.lokal").unwrap();
        });
        let text = log.text();
        assert_eq!(text.matches("is full").count(), 1, "{text}");
        assert!(text.contains(" INFO "), "{text}");
        assert!(text.contains("cap=1024"), "{text}");
        assert_eq!(s.cached_count(), ON_DEMAND_CAP);
    }

    /// Reload drops on-demand leaves that no wildcard covers any more, and
    /// those whose name now has its own service.
    #[test]
    fn reload_prunes_uncovered_on_demand_leaves() {
        let s = store();
        s.set_allowed(Allowed::new(
            ["api.p.lokal".to_string()],
            ["*.p.lokal".to_string(), "*.q.lokal".to_string()],
        ));
        for n in ["x.p.lokal", "y.p.lokal", "x.q.lokal"] {
            s.leaf(n).unwrap().unwrap();
        }
        s.leaf("api.p.lokal").unwrap().unwrap();
        assert_eq!(s.cached_count(), 4);
        s.set_allowed(Allowed::new(
            ["api.p.lokal".to_string(), "y.p.lokal".to_string()],
            ["*.p.lokal".to_string()],
        ));
        s.evict_unallowed();
        let on_demand = s.on_demand.lock().unwrap();
        let mut left: Vec<&String> = on_demand.leaves.keys().collect();
        left.sort();
        assert_eq!(left, ["x.p.lokal"]);
        assert_eq!(on_demand.order, ["x.p.lokal"]);
        drop(on_demand);
        assert_eq!(s.cached_count(), 2);
        // Without patterns, nothing on demand is left.
        s.set_allowed(Allowed::new(["api.p.lokal".to_string()], []));
        s.evict_unallowed();
        assert_eq!(s.cached_count(), 1);
        assert!(s.leaf("x.p.lokal").is_none());
    }
}

//! The routing table: an immutable map swapped atomically on reload
//! (`ArcSwap`), so lookups are lock-free.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use crate::state::{self, HostHeader, State, validate};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Upstream {
    pub host: String,
    pub port: u16,
    pub host_header: HostHeader,
}

impl Upstream {
    /// `localhost:4000`, `[::1]:4000`
    pub fn authority(&self) -> String {
        if self.host.contains(':') {
            format!("[{}]:{}", self.host, self.port)
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Upstream(Upstream),
    /// The built-in `200 lokal ok` of the selftest name.
    Selftest,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Route {
    /// The service's FQDN; for a wildcard service, its pattern
    /// (`*.shop.lokal`).
    pub fqdn: String,
    pub project: String,
    pub target: Target,
}

#[derive(Debug, Clone, Default)]
pub struct ProjectView {
    pub name: String,
    pub domain: String,
    pub urls: Vec<Link>,
}

/// A service's URL and where it goes, for the 404 page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    /// `https://<fqdn>`; a wildcard service's is `https://*.shop.lokal`.
    pub url: String,
    /// `host:port`.
    pub upstream: String,
}

impl Link {
    /// Can a browser open it? Not a wildcard service's URL, which names no
    /// single page, so the 404 page shows it as text (SPEC §9.2).
    pub fn opens(&self) -> bool {
        !self.url.contains('*')
    }
}

#[derive(Debug, Default)]
pub struct RouteTable {
    routes: HashMap<String, Arc<Route>>,
    /// Wildcard services, longest base first, so the first that covers a
    /// host is the one it gets (SPEC §9.2).
    wildcards: Vec<Arc<Route>>,
    projects: Vec<ProjectView>,
}

impl RouteTable {
    /// Every served project's services, plus the selftest names. A project
    /// that lokal 0.5 or earlier created on `.local` gets no route, so no
    /// certificate and no name either (SPEC §4.3).
    pub fn new(state: &State, selftest: &[String]) -> Self {
        let mut routes = HashMap::new();
        let mut wildcards = vec![];
        let mut projects = vec![];
        let served = state.projects.iter().filter(|(_, p)| p.is_served());
        for (name, p) in served {
            let mut view = ProjectView {
                name: name.clone(),
                domain: p.domain.clone(),
                urls: vec![],
            };
            for (sub, s) in &p.services {
                let fqdn = p.fqdn(sub);
                let up = Upstream {
                    host: s.host.clone(),
                    port: s.port,
                    host_header: s.host_header,
                };
                view.urls.push(Link {
                    url: format!("https://{fqdn}"),
                    upstream: up.authority(),
                });
                let route = Arc::new(Route {
                    fqdn: fqdn.clone(),
                    project: name.clone(),
                    target: Target::Upstream(up),
                });
                if state::is_wildcard(sub) {
                    wildcards.push(route);
                } else {
                    routes.insert(fqdn, route);
                }
            }
            projects.push(view);
        }
        // The longest pattern has the longest base: `*.v1` before `*`.
        wildcards.sort_by(|a: &Arc<Route>, b: &Arc<Route>| {
            b.fqdn
                .len()
                .cmp(&a.fqdn.len())
                .then_with(|| a.fqdn.cmp(&b.fqdn))
        });
        for st in selftest {
            routes.insert(
                st.to_string(),
                Arc::new(Route {
                    fqdn: st.to_string(),
                    project: String::new(),
                    target: Target::Selftest,
                }),
            );
        }
        Self {
            routes,
            wildcards,
            projects,
        }
    }

    /// The route of exactly `host`: a service, or a selftest name.
    pub fn get(&self, host: &str) -> Option<&Arc<Route>> {
        self.routes.get(host)
    }

    /// The route for `host` (SPEC §9.2): its own service first, else the
    /// wildcard service with the longest base that covers it.
    pub fn lookup(&self, host: &str) -> Option<&Arc<Route>> {
        self.routes.get(host).or_else(|| {
            self.wildcards
                .iter()
                .find(|r| validate::covers(&r.fqdn, host))
        })
    }

    /// The exact FQDNs: services and selftest names, not wildcard patterns.
    /// They get a pre-warmed certificate and a name in the DNS zone.
    pub fn fqdns(&self) -> BTreeSet<String> {
        self.routes.keys().cloned().collect()
    }

    /// The wildcard services' patterns (`*.shop.lokal`). Their names get a
    /// certificate on the first handshake (SPEC §6.2).
    pub fn patterns(&self) -> BTreeSet<String> {
        self.wildcards.iter().map(|r| r.fqdn.clone()).collect()
    }

    pub fn len(&self) -> usize {
        self.routes.len() + self.wildcards.len()
    }

    pub fn is_empty(&self) -> bool {
        self.routes.is_empty() && self.wildcards.is_empty()
    }

    /// FQDNs and patterns whose route is new or different compared with
    /// `old`.
    pub fn changed_since(&self, old: &RouteTable) -> Vec<String> {
        let mut v: Vec<String> = self
            .routes
            .iter()
            .filter(|(k, r)| old.routes.get(*k).map(|o| o != *r).unwrap_or(true))
            .map(|(k, _)| k.clone())
            .collect();
        for r in &self.wildcards {
            if !old.wildcards.iter().any(|o| o == r) {
                v.push(r.fqdn.clone());
            }
        }
        v.sort();
        v
    }

    /// The project whose apex is `host` or a parent of it.
    pub fn project_for_host(&self, host: &str) -> Option<&ProjectView> {
        self.projects
            .iter()
            .find(|p| host == p.domain || host.ends_with(&format!(".{}", p.domain)))
    }

    pub fn all_urls(&self) -> Vec<Link> {
        self.projects.iter().flat_map(|p| p.urls.clone()).collect()
    }
}

/// `Host` header value → bare lowercase host (port and trailing dot removed).
pub fn normalize_host(h: &str) -> String {
    let h = h.trim();
    let host = if let Some(rest) = h.strip_prefix('[') {
        rest.split(']').next().unwrap_or("")
    } else {
        h.rsplit_once(':')
            .filter(|(_, p)| p.chars().all(|c| c.is_ascii_digit()))
            .map(|(h, _)| h)
            .unwrap_or(h)
    };
    host.trim_end_matches('.').to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_normalization() {
        assert_eq!(normalize_host("API.p.lokal:443"), "api.p.lokal");
        assert_eq!(normalize_host("api.p.lokal."), "api.p.lokal");
        assert_eq!(normalize_host("[::1]:80"), "::1");
        assert_eq!(normalize_host("p.lokal"), "p.lokal");
    }

    fn port_of(t: &RouteTable, host: &str) -> Option<u16> {
        match &t.lookup(host)?.target {
            Target::Upstream(up) => Some(up.port),
            Target::Selftest => Some(0),
        }
    }

    /// Decision 4 of the wildcard plan (SPEC §9.2): an exact service beats
    /// any wildcard, and among wildcards the longest base wins.
    #[test]
    fn lookup_prefers_exact_then_the_longest_wildcard() {
        let st = State::parse(
            "version = 1\n\
[projects.p]\ndomain = \"p.lokal\"\n\
[projects.p.services.\"*\"]\nhost = \"localhost\"\nport = 1\n\
[projects.p.services.\"*.v1\"]\nhost = \"localhost\"\nport = 2\n\
[projects.p.services.api]\nhost = \"localhost\"\nport = 3\n\
[projects.p.services.\"x.v1\"]\nhost = \"localhost\"\nport = 4\n\
[projects.q]\ndomain = \"q.lokal\"\n[projects.q.services.web]\nhost = \"localhost\"\nport = 5\n",
        )
        .unwrap();
        let t = RouteTable::new(&st, &["lokal-selftest-x.lokal".into()]);
        for (host, port) in [
            ("a.p.lokal", Some(1)),
            ("b.a.p.lokal", Some(1)),
            ("v1.p.lokal", Some(1)),
            ("api.p.lokal", Some(3)),
            ("y.api.p.lokal", Some(1)),
            ("y.v1.p.lokal", Some(2)),
            ("z.y.v1.p.lokal", Some(2)),
            ("x.v1.p.lokal", Some(4)),
            ("p.lokal", None),
            ("web.q.lokal", Some(5)),
            ("nope.q.lokal", None),
            ("a_b.p.lokal", None),
            ("lokal-selftest-x.lokal", Some(0)),
        ] {
            assert_eq!(port_of(&t, host), port, "{host}");
        }
        // Patterns aren't exact names: no pre-warm and no DNS zone entry.
        assert_eq!(
            t.fqdns().into_iter().collect::<Vec<_>>(),
            [
                "api.p.lokal",
                "lokal-selftest-x.lokal",
                "web.q.lokal",
                "x.v1.p.lokal"
            ]
        );
        assert_eq!(
            t.patterns().into_iter().collect::<Vec<_>>(),
            ["*.p.lokal", "*.v1.p.lokal"]
        );
        assert!(t.get("a.p.lokal").is_none());
        assert_eq!(t.len(), 6);
        // A changed or new wildcard counts as changed; an unchanged one doesn't.
        let old = RouteTable::new(
            &State::parse(
                "version = 1\n[projects.p]\ndomain = \"p.lokal\"\n\
[projects.p.services.\"*\"]\nhost = \"localhost\"\nport = 1\n\
[projects.p.services.\"*.v1\"]\nhost = \"localhost\"\nport = 9\n",
            )
            .unwrap(),
            &[],
        );
        assert_eq!(
            t.changed_since(&old),
            [
                "*.v1.p.lokal",
                "api.p.lokal",
                "lokal-selftest-x.lokal",
                "web.q.lokal",
                "x.v1.p.lokal"
            ]
        );
        // The 404 page lists the wildcard, as text.
        let urls = t.all_urls();
        let star = urls.iter().find(|l| l.url == "https://*.p.lokal").unwrap();
        assert!(!star.opens());
        assert!(
            urls.iter()
                .filter(|l| l.url != star.url && !l.url.contains("*.v1"))
                .all(Link::opens)
        );
    }

    /// A project still on `.local` (from lokal 0.5 or earlier) gets no
    /// route; the others do.
    #[test]
    fn local_projects_get_no_routes() {
        let st = State::parse(
            "version = 1\n\
[projects.a]\ndomain = \"a.lokal\"\n[projects.a.services.api]\nhost = \"localhost\"\nport = 4000\n\
[projects.old]\ndomain = \"old.local\"\n[projects.old.services.api]\nhost = \"localhost\"\nport = 4001\n\
[projects.old.services.\"@\"]\nhost = \"localhost\"\nport = 4002\n",
        )
        .unwrap();
        let t = RouteTable::new(&st, &["lokal-selftest-x.lokal".into()]);
        assert_eq!(
            t.fqdns().into_iter().collect::<Vec<_>>(),
            ["api.a.lokal", "lokal-selftest-x.lokal"]
        );
        assert!(t.get("api.old.local").is_none());
        assert!(t.project_for_host("api.old.local").is_none());
        assert_eq!(
            t.all_urls(),
            [Link {
                url: "https://api.a.lokal".into(),
                upstream: "localhost:4000".into(),
            }]
        );
    }
}

//! The routing table: an immutable map swapped atomically on reload
//! (`ArcSwap`), so lookups are lock-free.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use crate::state::{HostHeader, State};

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
    pub url: String,
    /// `host:port`.
    pub upstream: String,
}

#[derive(Debug, Default)]
pub struct RouteTable {
    routes: HashMap<String, Arc<Route>>,
    projects: Vec<ProjectView>,
}

impl RouteTable {
    /// Every served project's services, plus the selftest names. A project
    /// that lokal 0.5 or earlier created on `.local` gets no route, so no
    /// certificate and no name either (SPEC §4.3).
    pub fn new(state: &State, selftest: &[String]) -> Self {
        let mut routes = HashMap::new();
        let mut projects = vec![];
        for (name, p) in state.projects.iter().filter(|(_, p)| p.is_served()) {
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
                routes.insert(
                    fqdn.clone(),
                    Arc::new(Route {
                        fqdn,
                        project: name.clone(),
                        target: Target::Upstream(up),
                    }),
                );
            }
            projects.push(view);
        }
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
        Self { routes, projects }
    }

    pub fn get(&self, host: &str) -> Option<&Arc<Route>> {
        self.routes.get(host)
    }

    pub fn fqdns(&self) -> BTreeSet<String> {
        self.routes.keys().cloned().collect()
    }

    pub fn len(&self) -> usize {
        self.routes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.routes.is_empty()
    }

    /// FQDNs whose route is new or different compared with `old`.
    pub fn changed_since(&self, old: &RouteTable) -> Vec<String> {
        let mut v: Vec<String> = self
            .routes
            .iter()
            .filter(|(k, r)| old.routes.get(*k).map(|o| o != *r).unwrap_or(true))
            .map(|(k, _)| k.clone())
            .collect();
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

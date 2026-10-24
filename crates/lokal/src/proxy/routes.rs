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
    /// `(url, upstream)`
    pub urls: Vec<(String, String)>,
}

#[derive(Debug, Default)]
pub struct RouteTable {
    routes: HashMap<String, Arc<Route>>,
    projects: Vec<ProjectView>,
}

impl RouteTable {
    pub fn build(state: &State, selftest: &[String]) -> Self {
        let mut routes = HashMap::new();
        let mut projects = vec![];
        for (name, p) in &state.projects {
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
                view.urls.push((format!("https://{fqdn}"), up.authority()));
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

    pub fn all_urls(&self) -> Vec<(String, String)> {
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
        assert_eq!(normalize_host("API.p.local:443"), "api.p.local");
        assert_eq!(normalize_host("api.p.local."), "api.p.local");
        assert_eq!(normalize_host("[::1]:80"), "::1");
        assert_eq!(normalize_host("p.local"), "p.local");
    }
}

//! The routing table: an immutable map swapped atomically on reload
//! (`ArcSwap`), so lookups are lock-free.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use crate::state::validate::{self, RouteTarget};
use crate::state::{self, HostHeader, Project, State};

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
    /// A route from `lokal.toml` (SPEC §4.11, §9.2): a host pattern whose
    /// names are forwarded to a target URL, under a path prefix.
    Rewrite(Rewrite),
    /// The built-in `200 lokal ok` of the selftest name.
    Selftest,
}

/// Where a route forwards, resolved when the table is built: a service
/// target's upstream is copied in, so a request does one lookup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rewrite {
    /// The service target's upstream (with its `host_header`), or the
    /// server at `http://localhost:<port>`.
    pub upstream: Upstream,
    /// The target URL's host, the `Host` the target gets:
    /// `render.shop.lokal`, or `localhost:3000`.
    pub target_host: String,
    /// `https://render.shop.lokal` or `http://localhost:3000`: a redirect
    /// to it is mapped back to the host asked for ([`map_location`]).
    pub origin: String,
    /// The target URL's path, its placeholders not filled in yet
    /// (`/demosite/{site}`); maybe empty.
    pub prefix_template: String,
}

impl Rewrite {
    /// The path prefix for a request: the template filled with what the
    /// pattern's captures matched (`/demosite/acme.com`).
    pub fn prefix(&self, found: &[(&str, &str)]) -> String {
        validate::fill_path(&self.prefix_template, found)
    }

    /// The `Host` the target gets (SPEC §9.2): the target URL's host, or,
    /// for a service whose `host_header` is `upstream`, its upstream's.
    pub fn host_header(&self) -> String {
        match self.upstream.host_header {
            HostHeader::Preserve => self.target_host.clone(),
            HostHeader::Upstream => self.upstream.authority(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Route {
    /// The service's FQDN; for a wildcard service or a route, its pattern
    /// (`*.shop.lokal`, `{site}.shop.lokal`).
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
    /// `https://<fqdn>`; a wildcard service's is `https://*.shop.lokal`, a
    /// route's `https://{site}.shop.lokal`.
    pub url: String,
    /// `host:port`, or a route's target URL.
    pub upstream: String,
}

impl Link {
    /// Can a browser open it? Not a wildcard service's URL or a route's,
    /// which name no single page, so the 404 page shows them as text (SPEC
    /// §9.2).
    pub fn opens(&self) -> bool {
        !self.url.contains(['*', '{'])
    }
}

#[derive(Debug, Default)]
pub struct RouteTable {
    routes: HashMap<String, Arc<Route>>,
    /// Wildcard services and routes, in [`validate::pattern_order`], so the
    /// first that covers a host is the one it gets (SPEC §9.2).
    patterns: Vec<Arc<Route>>,
    projects: Vec<ProjectView>,
}

/// The table's form of project `p`'s route `key` (SPEC §4.11), or why it
/// can't forward: a hand-edited key or target that isn't valid, or a
/// service target that isn't a service of the project (any more).
fn rewrite(p: &Project, key: &str, r: &state::Route) -> Result<Rewrite, String> {
    match validate::route_key(key, &p.domain) {
        Ok(k) if k == key => {}
        Ok(_) => return Err(format!("`{key}` isn't written as lokal writes a route")),
        Err(e) => return Err(e.message),
    }
    let target =
        validate::route_target(&r.to, &validate::capture_names(key)).map_err(|e| e.message)?;
    let upstream = match &target {
        RouteTarget::Service { fqdn, .. } => {
            let sub = if *fqdn == p.domain {
                Some("@")
            } else {
                fqdn.strip_suffix(&format!(".{}", p.domain))
            };
            let svc = sub
                .filter(|sub| !state::is_wildcard(sub))
                .and_then(|sub| p.services.get(sub))
                .ok_or_else(|| format!("its target {fqdn} isn't a service of the project"))?;
            Upstream {
                host: svc.host.clone(),
                port: svc.port,
                host_header: svc.host_header,
            }
        }
        RouteTarget::Local { host, port, .. } => Upstream {
            host: host.clone(),
            port: *port,
            host_header: HostHeader::Upstream,
        },
    };
    Ok(Rewrite {
        upstream,
        target_host: target.host(),
        origin: target.origin(),
        prefix_template: target.path().to_string(),
    })
}

impl RouteTable {
    /// Every served project's services, plus the selftest names. A project
    /// that lokal 0.5 or earlier created on `.local` gets no route, so no
    /// certificate and no name either (SPEC §4.3).
    pub fn new(state: &State, selftest: &[String]) -> Self {
        let mut routes = HashMap::new();
        let mut patterns = vec![];
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
                    patterns.push(route);
                } else {
                    routes.insert(fqdn, route);
                }
            }
            for (key, r) in &p.routes {
                let fqdn = p.fqdn(key);
                let rw = match rewrite(p, key, r) {
                    Ok(rw) => rw,
                    Err(why) => {
                        tracing::warn!(route = %fqdn, "not serving the route: {why}; run 'lokal up' in its repo");
                        continue;
                    }
                };
                view.urls.push(Link {
                    url: format!("https://{fqdn}"),
                    upstream: r.to.clone(),
                });
                patterns.push(Arc::new(Route {
                    fqdn,
                    project: name.clone(),
                    target: Target::Rewrite(rw),
                }));
            }
            projects.push(view);
        }
        // A literal beats a capture, from the base outward: `*.v1` before
        // `*`, `preview.{slug}` before `{site}`.
        patterns
            .sort_by(|a: &Arc<Route>, b: &Arc<Route>| validate::pattern_order(&a.fqdn, &b.fqdn));
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
            patterns,
            projects,
        }
    }

    /// The route of exactly `host`: a service, or a selftest name.
    pub fn get(&self, host: &str) -> Option<&Arc<Route>> {
        self.routes.get(host)
    }

    /// The route for `host` (SPEC §9.2), and what its pattern's captures
    /// matched: its own service first (no captures), else the first
    /// wildcard service or route that covers it, in
    /// [`validate::pattern_order`].
    pub fn lookup<'t, 'h>(
        &'t self,
        host: &'h str,
    ) -> Option<(&'t Arc<Route>, validate::Captured<'t, 'h>)> {
        if let Some(r) = self.routes.get(host) {
            return Some((r, vec![]));
        }
        self.patterns
            .iter()
            .find_map(|r| validate::captures(&r.fqdn, host).map(|found| (r, found)))
    }

    /// The exact FQDNs: services and selftest names, not patterns. They get
    /// a pre-warmed certificate and a name in the DNS zone.
    pub fn fqdns(&self) -> BTreeSet<String> {
        self.routes.keys().cloned().collect()
    }

    /// The wildcard services' patterns (`*.shop.lokal`). Their names get a
    /// certificate on the first handshake (SPEC §6.2).
    pub fn patterns(&self) -> BTreeSet<String> {
        self.patterns
            .iter()
            .filter(|r| !matches!(r.target, Target::Rewrite(_)))
            .map(|r| r.fqdn.clone())
            .collect()
    }

    /// The routes' patterns (`{site}.shop.lokal`), ready once their route is
    /// installed. Not handed to the certificate resolver yet.
    pub fn route_patterns(&self) -> BTreeSet<String> {
        self.patterns
            .iter()
            .filter(|r| matches!(r.target, Target::Rewrite(_)))
            .map(|r| r.fqdn.clone())
            .collect()
    }

    pub fn len(&self) -> usize {
        self.routes.len() + self.patterns.len()
    }

    pub fn is_empty(&self) -> bool {
        self.routes.is_empty() && self.patterns.is_empty()
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
        for r in &self.patterns {
            if !old.patterns.iter().any(|o| o == r) {
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

/// The path and query a route's target gets for `path_and_query` (SPEC
/// §9.2), with `prefix` the target's path, its captures filled in: `/`
/// becomes `prefix` exactly as written (no slash is added: a server that
/// redirects `/x/` to `/x` would send the browser to the public `/`, where
/// the prefix is added again), any other path follows `prefix` without its
/// trailing slash, and an empty `prefix` changes nothing. The query stays.
pub fn map_path(prefix: &str, path_and_query: &str) -> String {
    if prefix.is_empty() {
        return path_and_query.to_string();
    }
    let (path, query) =
        path_and_query.split_at(path_and_query.find('?').unwrap_or(path_and_query.len()));
    let base = prefix.strip_suffix('/').unwrap_or(prefix);
    let mapped = match path {
        "" | "/" => prefix.to_string(),
        p if p.starts_with('/') => format!("{base}{p}"),
        p => format!("{base}/{p}"),
    };
    format!("{mapped}{query}")
}

/// A route's redirect, mapped back to the host asked for (SPEC §9.2, as
/// nginx's `proxy_redirect default`), or `None` to pass it on untouched: a
/// path in the target's path space (under `prefix`) loses the prefix
/// (`/demosite/acme.com/new` → `/new`; exactly the prefix → `/`), and an
/// absolute URL on the target's `origin` whose path is under `prefix`
/// becomes `https://<host><the rest>`. Any other `Location` stays.
pub fn map_location(location: &str, prefix: &str, origin: &str, host: &str) -> Option<String> {
    let base = prefix.strip_suffix('/').unwrap_or(prefix);
    if location.starts_with('/') && !location.starts_with("//") {
        return outside(location, base);
    }
    let rest = on_origin(location, origin)?;
    let path = if rest.starts_with('/') {
        rest.to_string()
    } else {
        format!("/{rest}")
    };
    outside(&path, base).map(|p| format!("https://{host}{p}"))
}

/// `path` (with its query or fragment) without `base`, if it's under it.
fn outside(path: &str, base: &str) -> Option<String> {
    let rest = path.strip_prefix(base)?;
    if rest.is_empty() || rest.starts_with(['?', '#']) {
        Some(format!("/{rest}"))
    } else {
        rest.starts_with('/').then(|| rest.to_string())
    }
}

/// What follows `origin` in the absolute URL `url` (its path, query and
/// fragment), if `url` is on it: the same scheme and host, ignoring case,
/// and the same port, the scheme's default included.
fn on_origin<'u>(url: &'u str, origin: &str) -> Option<&'u str> {
    let (scheme, rest) = url.split_once("://")?;
    let (want_scheme, want) = origin.split_once("://")?;
    if !scheme.eq_ignore_ascii_case(want_scheme) {
        return None;
    }
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(end);
    let authority = authority.to_ascii_lowercase();
    let default = if want_scheme == "https" {
        ":443"
    } else {
        ":80"
    };
    let authority = authority.strip_suffix(default).unwrap_or(&authority);
    (authority == want).then_some(tail)
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
        match &t.lookup(host)?.0.target {
            Target::Upstream(up) => Some(up.port),
            Target::Rewrite(rw) => Some(rw.upstream.port),
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

    const ROUTED: &str = r#"version = 1
[projects.p]
domain = "p.lokal"
[projects.p.services."@"]
host = "localhost"
port = 4000
[projects.p.services.render]
host = "localhost"
port = 51234
[projects.p.services.api]
host = "127.0.0.1"
port = 4001
host_header = "upstream"
[projects.p.services."*.v1"]
host = "localhost"
port = 4002
[projects.p.routes."{site}"]
to = "https://render.p.lokal/demosite/{site}"
managed = "lokal.toml"
[projects.p.routes."preview.{slug}"]
to = "http://localhost:3000/preview/{slug}"
[projects.p.routes."{a}.x"]
to = "https://p.lokal"
[projects.p.routes."{b}.api"]
to = "https://api.p.lokal/b/{b}/"
[projects.p.routes."{z}.gone"]
to = "https://gone.p.lokal/{z}"
[projects.p.routes."{z}.wild"]
to = "https://*.v1.p.lokal/{z}"
[projects.p.routes."{z}.bad"]
to = "ftp://render.p.lokal/{z}"
[projects.p.routes."Shout.{z}"]
to = "http://localhost:3000"
"#;

    /// What a lookup reaches: a port, the `Host` the target gets and the
    /// path prefix.
    fn reach(t: &RouteTable, host: &str) -> Option<(u16, String, String)> {
        let (r, found) = t.lookup(host)?;
        Some(match &r.target {
            Target::Rewrite(rw) => (rw.upstream.port, rw.host_header(), rw.prefix(&found)),
            Target::Upstream(up) => (up.port, String::new(), String::new()),
            Target::Selftest => (0, String::new(), String::new()),
        })
    }

    /// Decision 4 of the routes plan: an exact service first, then patterns
    /// with a literal before a capture from the base outward, and a longer
    /// before a shorter. A route forwards to a service's own upstream, with
    /// the target's host as `Host` (or the service's upstream, with
    /// `host_header = "upstream"`).
    #[test]
    fn routes_forward_under_their_patterns() {
        let t = RouteTable::new(&State::parse(ROUTED).unwrap(), &[]);
        let r = |port: u16, host: &str, prefix: &str| {
            Some((port, host.to_string(), prefix.to_string()))
        };
        for (host, want) in [
            (
                "acme.com.p.lokal",
                r(51234, "render.p.lokal", "/demosite/acme.com"),
            ),
            ("a.p.lokal", r(51234, "render.p.lokal", "/demosite/a")),
            (
                "preview.acme.p.lokal",
                r(3000, "localhost:3000", "/preview/acme"),
            ),
            // The depth is pinned: this one is `{site}`'s.
            (
                "a.preview.acme.p.lokal",
                r(51234, "render.p.lokal", "/demosite/a.preview.acme"),
            ),
            ("q.x.p.lokal", r(4000, "p.lokal", "")),
            ("q.api.p.lokal", r(4001, "127.0.0.1:4001", "/b/q/")),
            // A literal beats a capture: `*.v1` over `{site}`.
            ("y.v1.p.lokal", Some((4002, String::new(), String::new()))),
            // Exact services win.
            (
                "render.p.lokal",
                Some((51234, String::new(), String::new())),
            ),
            ("api.p.lokal", Some((4001, String::new(), String::new()))),
            ("p.lokal", Some((4000, String::new(), String::new()))),
            // Routes that can't forward aren't there: `{site}` takes them.
            (
                "q.gone.p.lokal",
                r(51234, "render.p.lokal", "/demosite/q.gone"),
            ),
            (
                "q.wild.p.lokal",
                r(51234, "render.p.lokal", "/demosite/q.wild"),
            ),
            (
                "q.bad.p.lokal",
                r(51234, "render.p.lokal", "/demosite/q.bad"),
            ),
            (
                "shout.q.p.lokal",
                r(51234, "render.p.lokal", "/demosite/shout.q"),
            ),
            // Hostile hosts reach nothing.
            ("a_b.p.lokal", None),
            ("..p.lokal", None),
            ("%2e.p.lokal", None),
            ("other.lokal", None),
        ] {
            assert_eq!(reach(&t, host), want, "{host}");
        }
        assert_eq!(
            t.route_patterns().into_iter().collect::<Vec<_>>(),
            [
                "preview.{slug}.p.lokal",
                "{a}.x.p.lokal",
                "{b}.api.p.lokal",
                "{site}.p.lokal"
            ]
        );
        // Only the wildcards go to the certificate resolver, for now.
        assert_eq!(
            t.patterns().into_iter().collect::<Vec<_>>(),
            ["*.v1.p.lokal"]
        );
        assert_eq!(t.len(), 3 + 1 + 4);
        let (site, _) = t.lookup("a.p.lokal").unwrap();
        let Target::Rewrite(rw) = &site.target else {
            panic!("{site:?}")
        };
        assert_eq!(rw.origin, "https://render.p.lokal");
        let (preview, _) = t.lookup("preview.a.p.lokal").unwrap();
        let Target::Rewrite(rw) = &preview.target else {
            panic!("{preview:?}")
        };
        assert_eq!(rw.origin, "http://localhost:3000");

        // The 404 page lists a route as text, with its target URL.
        let urls = t.all_urls();
        let site = urls
            .iter()
            .find(|l| l.url == "https://{site}.p.lokal")
            .unwrap();
        assert_eq!(site.upstream, "https://render.p.lokal/demosite/{site}");
        assert!(!site.opens());
        assert!(!urls.iter().any(|l| l.url.contains("gone")));

        // A route changes with the service it forwards to.
        let moved = State::parse(&ROUTED.replace("port = 51234", "port = 51235")).unwrap();
        let changed = RouteTable::new(&moved, &[]).changed_since(&t);
        assert_eq!(changed, ["render.p.lokal", "{site}.p.lokal"]);
        let gone = State::parse(&ROUTED.replace(
            "[projects.p.routes.\"{site}\"]",
            "[projects.p.routes.\"{site}.zz\"]",
        ))
        .unwrap();
        let t2 = RouteTable::new(&gone, &[]);
        assert!(!t2.route_patterns().contains("{site}.p.lokal"));
        assert!(t2.route_patterns().contains("{site}.zz.p.lokal"));
    }

    /// Decision 7: `/` is the prefix exactly as written; other paths follow
    /// it without its trailing slash; an empty prefix changes nothing.
    #[test]
    fn paths_map_under_the_prefix() {
        for (prefix, asked, want) in [
            ("/demosite/acme.com", "/", "/demosite/acme.com"),
            ("/demosite/acme.com", "/?x=1", "/demosite/acme.com?x=1"),
            (
                "/demosite/acme.com",
                "/about?x=1",
                "/demosite/acme.com/about?x=1",
            ),
            ("/demosite/acme.com", "/a/b/", "/demosite/acme.com/a/b/"),
            ("/demosite/acme.com/", "/", "/demosite/acme.com/"),
            ("/demosite/acme.com/", "/about", "/demosite/acme.com/about"),
            ("/", "/", "/"),
            ("/", "/about", "/about"),
            ("", "/", "/"),
            ("", "/about?x=1", "/about?x=1"),
            ("/p", "*", "/p/*"),
        ] {
            assert_eq!(map_path(prefix, asked), want, "{prefix} {asked}");
        }
    }

    /// Decision 9: a redirect into the target's path space comes back
    /// without the prefix, on the host asked for; anything else stays.
    #[test]
    fn redirects_map_back_to_the_host_asked_for() {
        let p = "/demosite/acme.com";
        let o = "https://render.p.lokal";
        let h = "acme.com.p.lokal";
        let m = |loc: &str| map_location(loc, p, o, h);
        assert_eq!(m("/demosite/acme.com/new").as_deref(), Some("/new"));
        assert_eq!(m("/demosite/acme.com/a/b?x=1").as_deref(), Some("/a/b?x=1"));
        assert_eq!(m("/demosite/acme.com").as_deref(), Some("/"));
        assert_eq!(m("/demosite/acme.com/").as_deref(), Some("/"));
        assert_eq!(m("/demosite/acme.com?x=1").as_deref(), Some("/?x=1"));
        assert_eq!(m("/demosite/acme.com#top").as_deref(), Some("/#top"));
        assert_eq!(
            m("https://render.p.lokal/demosite/acme.com/new").as_deref(),
            Some("https://acme.com.p.lokal/new")
        );
        assert_eq!(
            m("HTTPS://Render.P.lokal:443/demosite/acme.com").as_deref(),
            Some("https://acme.com.p.lokal/")
        );
        for untouched in [
            "/elsewhere",
            "/demosite/acme.comx",
            "/demosite/other.com/new",
            "//render.p.lokal/demosite/acme.com/new",
            "new",
            "https://render.p.lokal/elsewhere",
            "http://render.p.lokal/demosite/acme.com/new",
            "https://render.p.lokal:8443/demosite/acme.com/new",
            "https://example.com/demosite/acme.com/new",
            "https://render.p.lokal.example.com/demosite/acme.com",
        ] {
            assert_eq!(m(untouched), None, "{untouched}");
        }
        // A server lokal doesn't start: its own origin, port included.
        let local = |loc: &str| {
            map_location(
                loc,
                "/preview/acme",
                "http://localhost:3000",
                "preview.acme.p.lokal",
            )
        };
        assert_eq!(
            local("http://localhost:3000/preview/acme/x").as_deref(),
            Some("https://preview.acme.p.lokal/x")
        );
        assert_eq!(local("http://localhost:3001/preview/acme/x"), None);
        assert_eq!(local("http://127.0.0.1:3000/preview/acme/x"), None);
        // An empty prefix: every path is the target's, and stays as it is.
        let all = |loc: &str| map_location(loc, "", "https://render.p.lokal", h);
        assert_eq!(all("/new").as_deref(), Some("/new"));
        assert_eq!(
            all("https://render.p.lokal").as_deref(),
            Some("https://acme.com.p.lokal/")
        );
    }
}

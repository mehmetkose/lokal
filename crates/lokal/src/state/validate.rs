//! Validation rules of SPEC §4.3. Every input is trimmed and lowercased
//! first. Errors are usage errors (exit 2) with a hint where useful.

use std::net::IpAddr;

use anyhow::Result;

use super::State;
use crate::error::UserError;

/// TLD of new projects: `.lokal`, served by lokal's own DNS server.
pub const DEFAULT_TLD: &str = "lokal";
/// The TLDs lokal serves: only `.lokal` since 0.6.0. The CA's name
/// constraints (`ca::PERMITTED_TLDS`) also permit `.local`, lokal's TLD
/// before 0.6.0, on purpose: narrowing them would replace the CA on every
/// machine.
pub const SUPPORTED_TLDS: &[&str] = &["lokal"];
/// lokal's TLD before 0.6.0 (mDNS on macOS). Projects that `state.toml`
/// still has on it load, so they can be removed, but aren't served.
pub const DROPPED_TLD: &str = "local";

pub fn normalize(s: &str) -> String {
    s.trim().to_ascii_lowercase()
}

/// 1–63 chars of `[a-z0-9-]`, not starting or ending with `-`.
pub fn is_label(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 63
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !s.starts_with('-')
        && !s.ends_with('-')
}

fn label_error(what: &str, s: &str) -> UserError {
    UserError::usage(format!(
        "invalid {what} `{s}`: use 1–63 letters, digits or '-', not starting or ending with '-'"
    ))
}

pub fn project_name(input: &str) -> Result<String, UserError> {
    let s = normalize(input);
    if s.contains('.') {
        return Err(UserError::usage(format!("project name `{s}` can't contain dots"))
            .hint(format!("use a plain name and pass the full domain: lokal project create <NAME> --domain {s}")));
    }
    if !is_label(&s) {
        return Err(label_error("project name", &s));
    }
    Ok(s)
}

/// The apex for a new project: `<name>.lokal`, or a validated `--domain`.
pub fn apex(name: &str, domain: Option<&str>) -> Result<String, UserError> {
    let Some(d) = domain else {
        return Ok(format!("{name}.{DEFAULT_TLD}"));
    };
    let d = normalize(d);
    let d = d.trim_end_matches('.').to_string();
    let labels: Vec<&str> = d.split('.').collect();
    let tld = *labels.last().unwrap_or(&"");
    if let Some(to) = lokal_for_local(name, &d) {
        return Err(
            UserError::usage(".local names aren't supported since 0.6.0")
                .hint(format!("use --domain {to}")),
        );
    }
    if !SUPPORTED_TLDS.contains(&tld) {
        return Err(UserError::usage(
            "only .lokal is supported (other TLDs could shadow real websites)",
        ));
    }
    if labels.len() < 2 {
        return Err(UserError::usage(format!(
            "invalid domain `{d}`: it needs a name before .{tld}"
        )));
    }
    if d.len() > 253 {
        return Err(UserError::usage(format!(
            "domain `{d}` is longer than 253 characters"
        )));
    }
    for l in &labels {
        if !is_label(l) {
            return Err(label_error("domain label", l));
        }
    }
    Ok(d)
}

/// Where project `name`'s domain moves if it's on `.local`: the same name
/// under `.lokal` (`shop.local` → `shop.lokal`), or `<name>.lokal` when
/// nothing comes before `.local`. `None` for a domain on another TLD.
pub fn lokal_for_local(name: &str, domain: &str) -> Option<String> {
    let d = normalize(domain);
    let d = d.trim_end_matches('.');
    let (rest, tld) = d.rsplit_once('.').unwrap_or(("", d));
    (tld == DROPPED_TLD).then(|| match rest {
        "" => format!("{name}.{DEFAULT_TLD}"),
        _ => format!("{rest}.{DEFAULT_TLD}"),
    })
}

/// Does lokal serve `domain`? Only `.lokal` names: a project that an older
/// lokal created on `.local` still loads from `state.toml`, but gets no
/// route, certificate or name (SPEC §4.3).
pub fn is_served(domain: &str) -> bool {
    let tld = domain.rsplit('.').next().unwrap_or_default();
    SUPPORTED_TLDS.contains(&tld)
}

/// The error for adding to or changing project `name`, whose `domain`
/// lokal no longer serves.
pub fn unserved(name: &str, domain: &str) -> UserError {
    UserError::runtime(format!(
        "project `{name}` is on {domain}, and lokal serves only .lokal names since 0.6.0"
    ))
    .hint(move_hint(name))
}

/// How to move project `name` to `.lokal`. lokal never moves it by
/// itself: a new URL breaks OAuth callbacks, cookies and bookmarks.
pub fn move_hint(name: &str) -> String {
    format!(
        "move it to .lokal: lokal project remove {name} --force, then lokal project create {name} \
and add its services again (or run lokal up, if its repo has a lokal.toml)"
    )
}

/// `a` equals, contains or sits under `b` (label-wise).
pub fn apex_overlaps(a: &str, b: &str) -> bool {
    a == b || a.ends_with(&format!(".{b}")) || b.ends_with(&format!(".{a}"))
}

pub fn check_overlap(state: &State, apex: &str, except: Option<&str>) -> Result<(), UserError> {
    for (name, p) in &state.projects {
        if Some(name.as_str()) == except {
            continue;
        }
        if apex_overlaps(apex, &p.domain) {
            return Err(UserError::usage(format!(
                "{apex} overlaps project `{name}` ({})",
                p.domain
            ))
            .hint("pick a domain that neither equals, contains nor sits under another project's domain"));
        }
    }
    Ok(())
}

/// `@`, or dot-joined labels, the first of which may be `*` (a wildcard
/// service: `*` or `*.v1`). A full FQDN under the apex is accepted and
/// stripped (`api.myproject.lokal` → `api`, `*.myproject.lokal` → `*`).
pub fn subdomain(input: &str, apex: &str) -> Result<String, UserError> {
    let mut s = normalize(input);
    s = s.trim_end_matches('.').to_string();
    if s == "@" || s == apex {
        return Ok("@".into());
    }
    if let Some(stripped) = s.strip_suffix(&format!(".{apex}")) {
        s = stripped.to_string();
    }
    if s.is_empty() {
        return Err(UserError::usage("subdomain can't be empty").hint("use @ for the apex"));
    }
    for (i, l) in s.split('.').enumerate() {
        if i == 0 && l == "*" {
            continue;
        }
        if l.contains('*') {
            return Err(UserError::usage(format!(
                "invalid subdomain `{s}`: `*` can only be the whole first label"
            ))
            .hint("use '*' for every name under the project, or '*.<name>', such as '*.v1'"));
        }
        if !is_label(l) {
            return Err(label_error("subdomain label", l));
        }
    }
    let fqdn = format!("{s}.{apex}");
    if fqdn.len() > 253 {
        return Err(UserError::usage(format!(
            "{fqdn} is longer than 253 characters"
        )));
    }
    Ok(s)
}

/// The base of a wildcard service's FQDN: `shop.lokal` for `*.shop.lokal`,
/// `v1.shop.lokal` for `*.v1.shop.lokal`. `None` for any other name.
pub fn wildcard_base(fqdn: &str) -> Option<&str> {
    fqdn.strip_prefix("*.")
        .filter(|base| !base.is_empty() && !base.contains('*'))
}

/// Does the pattern `pattern_fqdn` cover `host`? A pattern is a wildcard
/// service's FQDN (`*.shop.lokal`, SPEC §4.3) or a route's
/// (`{site}.shop.lokal`, `preview.{slug}.shop.lokal`, §4.11); any other
/// name covers nothing, not even itself. A wildcard covers every name under
/// its base, at any depth (`a.shop.lokal`, `b.a.shop.lokal`), but never the
/// base itself, and only when every label in front of the base is a label:
/// not `a_b.shop.lokal`, and not `*.shop.lokal`. [`captures`] has the rules
/// for routes. `host` is lowercase, without a trailing dot. The one matcher
/// for routes, certificates, `open` and doctor.
pub fn covers(pattern_fqdn: &str, host: &str) -> bool {
    captures(pattern_fqdn, host).is_some()
}

/// One label of a pattern.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PatternLabel<'a> {
    /// Matches itself.
    Literal(&'a str),
    /// `{name}`, or `*` (a wildcard service's, named `*`).
    Capture(&'a str),
}

/// `name` of a `{name}` label: `[a-z][a-z0-9_]*`. `None` for any other label.
fn capture_name(label: &str) -> Option<&str> {
    let name = label.strip_prefix('{')?.strip_suffix('}')?;
    let mut bytes = name.bytes();
    let first = bytes.next()?;
    (first.is_ascii_lowercase()
        && bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_'))
    .then_some(name)
}

/// The labels of a pattern, or `None` if it isn't one: a pattern has at
/// least one capture, `*` only as the whole first label, and `{name}` only
/// as a whole label, and its last label is literal (a pattern sits under a
/// base). Its other labels are literal.
fn pattern_labels(pattern_fqdn: &str) -> Option<Vec<PatternLabel<'_>>> {
    let mut labels = vec![];
    let mut captured = false;
    for (i, l) in pattern_fqdn.split('.').enumerate() {
        let label = if i == 0 && l == "*" {
            PatternLabel::Capture("*")
        } else if let Some(name) = capture_name(l) {
            PatternLabel::Capture(name)
        } else if l.is_empty() || l.contains(['*', '{', '}']) {
            return None;
        } else {
            PatternLabel::Literal(l)
        };
        captured |= matches!(label, PatternLabel::Capture(_));
        labels.push(label);
    }
    // A pattern sits under a base: `*` alone isn't one.
    let based = matches!(labels.last(), Some(PatternLabel::Literal(_)));
    (captured && based).then_some(labels)
}

/// What a pattern's captures matched in a host: (name, value) pairs, in
/// the pattern's order ([`captures`]).
pub type Captured<'p, 'h> = Vec<(&'p str, &'h str)>;

/// What each capture of `pattern_fqdn` matched in `host`, in the pattern's
/// order (`*` is named `*`), or `None` if the pattern doesn't cover `host`
/// ([`covers`]). Every label of `host` must be a label ([`is_label`]), and
/// `host` at most 253 characters. The leftmost label, when it's a capture,
/// matches one or more labels (`{site}` covers `acme.com.shop.lokal`, with
/// `site` = `acme.com`, as `*` covers any depth); every other capture
/// matches exactly one label, and a literal leftmost label pins the depth:
/// `preview.{slug}.shop.lokal` covers `preview.acme.shop.lokal`, but not
/// `a.preview.acme.shop.lokal`. So a captured value is `[a-z0-9-]` labels
/// joined by dots: it can't hold `/`, `%`, `?` or `..`.
pub fn captures<'p, 'h>(pattern_fqdn: &'p str, host: &'h str) -> Option<Captured<'p, 'h>> {
    let pattern = pattern_labels(pattern_fqdn)?;
    if host.len() > 253 {
        return None;
    }
    let labels: Vec<&str> = host.split('.').collect();
    if !labels.iter().all(|l| is_label(l)) {
        return None;
    }
    let deep = matches!(pattern[0], PatternLabel::Capture(_));
    let fixed = if deep {
        pattern.len() - 1
    } else {
        pattern.len()
    };
    let enough = if deep {
        labels.len() > fixed
    } else {
        labels.len() == fixed
    };
    if !enough {
        return None;
    }
    let mut found = vec![];
    // The labels after the leftmost, matched from the right.
    for (p, h) in pattern.iter().rev().zip(labels.iter().rev()).take(fixed) {
        match p {
            PatternLabel::Literal(l) if l == h => {}
            PatternLabel::Literal(_) => return None,
            PatternLabel::Capture(name) => found.push((*name, *h)),
        }
    }
    if let (true, PatternLabel::Capture(name)) = (deep, pattern[0]) {
        let front = labels.len() - fixed;
        // The front labels and the dots between them.
        let len = labels[..front].iter().map(|l| l.len()).sum::<usize>() + front - 1;
        found.push((name, &host[..len]));
    }
    found.reverse();
    Some(found)
}

/// How two patterns that may cover the same name are ordered (SPEC §4.3,
/// §4.11): comparing labels from the base outward, at the first position
/// where one has a literal and the other a capture (or `*`), the literal
/// comes first; if one runs out of labels first, the longer comes first.
/// So `*.v1` comes before `*`, and `preview.{slug}` before `{site}`. Two
/// patterns of the same shape ([`pattern_shape`]) are ordered by name, only
/// so that the order is stable: `lokal up` refuses them.
pub fn pattern_order(a: &str, b: &str) -> std::cmp::Ordering {
    // Literal < capture < no label left.
    let rank = |p: &str| -> Vec<u8> {
        let labels: Vec<&str> = p.split('.').collect();
        labels
            .iter()
            .enumerate()
            .rev()
            .map(|(i, l)| u8::from(capture_name(l).is_some() || (i == 0 && *l == "*")))
            .collect()
    };
    let (ra, rb) = (rank(a), rank(b));
    for i in 0..ra.len().max(rb.len()) {
        let x = ra.get(i).copied().unwrap_or(2);
        let y = rb.get(i).copied().unwrap_or(2);
        if x != y {
            return x.cmp(&y);
        }
    }
    a.cmp(b)
}

/// A pattern's shape: its labels, with every capture and `*` as `{}`
/// (`{}.shop.lokal` for both `*.shop.lokal` and `{site}.shop.lokal`). Two
/// patterns with the same shape cover the same names, so lokal couldn't
/// tell which of them a request is for. `None` for a name that isn't a
/// pattern.
pub fn pattern_shape(pattern_fqdn: &str) -> Option<String> {
    let labels = pattern_labels(pattern_fqdn)?;
    Some(
        labels
            .iter()
            .map(|l| match l {
                PatternLabel::Literal(l) => *l,
                PatternLabel::Capture(_) => "{}",
            })
            .collect::<Vec<_>>()
            .join("."),
    )
}

/// The names of a route key's captures (`site` for `{site}`), in order.
pub fn capture_names(pattern: &str) -> Vec<&str> {
    pattern.split('.').filter_map(capture_name).collect()
}

/// A pattern with every capture replaced by `with`
/// (`preview.lokal-doctor.shop.lokal` for `preview.{slug}.shop.lokal`): a
/// name it covers, for doctor to probe (SPEC §11).
pub fn fill_captures(pattern_fqdn: &str, with: &str) -> String {
    pattern_fqdn
        .split('.')
        .enumerate()
        .map(|(i, l)| {
            if capture_name(l).is_some() || (i == 0 && l == "*") {
                with
            } else {
                l
            }
        })
        .collect::<Vec<_>>()
        .join(".")
}

fn route_error(key: &str, why: &str) -> UserError {
    UserError::usage(format!("invalid route `{key}`: {why}"))
}

/// A key of `lokal.toml`'s `[routes]` (SPEC §4.11): labels and captures
/// (`{name}`, a whole label, `name` from `[a-z][a-z0-9_]*`), relative to
/// `apex`. A full name under the apex is accepted and stripped
/// (`{site}.shop.lokal` → `{site}`). Literal labels are lowercased;
/// capture names are taken as written. A route needs a capture (an exact
/// name belongs in `[services]`), never covers the apex (`@`), and takes no
/// `*`, which is a wildcard service's.
pub fn route_key(input: &str, apex: &str) -> Result<String, UserError> {
    let trimmed = input.trim().trim_end_matches('.');
    let s = trimmed
        .split('.')
        .map(|l| {
            if l.contains(['{', '}']) {
                l.to_string()
            } else {
                l.to_ascii_lowercase()
            }
        })
        .collect::<Vec<_>>()
        .join(".");
    if s == "@" || s == apex {
        return Err(UserError::usage(format!(
            "`{trimmed}` can't be a route: a route never covers the apex"
        ))
        .hint("an exact name belongs in [services]"));
    }
    let s = s
        .strip_suffix(&format!(".{apex}"))
        .unwrap_or(&s)
        .to_string();
    if s.is_empty() {
        return Err(UserError::usage("a route can't be empty")
            .hint("name it with a capture, e.g. \"{tenant}\""));
    }
    if s.contains('/') {
        return Err(route_error(&s, "a route is a host pattern, without a path"));
    }
    let mut names: Vec<&str> = vec![];
    for l in s.split('.') {
        if l.contains('*') {
            return Err(
                route_error(&s, "a route names what it captures, e.g. {tenant}, not `*`")
                    .hint("a wildcard service goes under [services]: \"*\" = \"…\""),
            );
        }
        if !l.contains(['{', '}']) {
            if !is_label(l) {
                return Err(label_error("route label", l));
            }
            continue;
        }
        let Some(name) = capture_name(l) else {
            let why = match l.strip_prefix('{').and_then(|x| x.strip_suffix('}')) {
                Some("") => "a capture needs a name, e.g. {tenant}".to_string(),
                Some(n) if !n.contains(['{', '}']) => format!(
                    "the capture name `{n}` must be lowercase letters, digits and '_', starting with a letter"
                ),
                _ => format!("a capture is a whole label, like {{x}}, so `{l}` can't be one"),
            };
            return Err(route_error(&s, &why));
        };
        if names.contains(&name) {
            return Err(route_error(
                &s,
                &format!("the capture {{{name}}} is used twice"),
            ));
        }
        names.push(name);
    }
    if names.is_empty() {
        return Err(UserError::usage(format!(
            "`{s}` has no capture; a route needs one, e.g. \"{{tenant}}.{s}\""
        ))
        .hint("an exact name belongs in [services]"));
    }
    // The shortest name it covers: each capture one character long.
    let shortest = s
        .split('.')
        .map(|l| {
            if capture_name(l).is_some() {
                1
            } else {
                l.len()
            }
        })
        .sum::<usize>()
        + s.split('.').count()
        + apex.len();
    if shortest > 253 {
        return Err(route_error(
            &s,
            "every name it covers is longer than 253 characters",
        ));
    }
    Ok(s)
}

/// Where a route forwards (SPEC §4.11).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteTarget {
    /// `https://<fqdn><path>`: a service of the same project, by its full
    /// name. lokal forwards to its upstream directly.
    Service { fqdn: String, path: String },
    /// `http://<host>:<port><path>`, with `localhost`, `127.0.0.1` or `::1`:
    /// a server lokal doesn't start.
    Local {
        host: String,
        port: u16,
        path: String,
    },
}

impl RouteTarget {
    /// The path as written, with its placeholders (`/demosite/{site}`),
    /// maybe empty.
    pub fn path(&self) -> &str {
        match self {
            RouteTarget::Service { path, .. } | RouteTarget::Local { path, .. } => path,
        }
    }

    /// `https://render.shop.lokal` or `http://localhost:3000`
    /// (`http://[::1]:3000`): where the target's own redirects point.
    pub fn origin(&self) -> String {
        match self {
            RouteTarget::Service { fqdn, .. } => format!("https://{fqdn}"),
            RouteTarget::Local { .. } => format!("http://{}", self.host()),
        }
    }

    /// The host part of the URL: `render.shop.lokal`, or `localhost:3000`
    /// (`[::1]:3000`) with its port.
    pub fn host(&self) -> String {
        match self {
            RouteTarget::Service { fqdn, .. } => fqdn.clone(),
            RouteTarget::Local { host, port, .. } if host.contains(':') => {
                format!("[{host}]:{port}")
            }
            RouteTarget::Local { host, port, .. } => format!("{host}:{port}"),
        }
    }

    /// The URL with the scheme and host lowercased, the path as written.
    pub fn url(&self) -> String {
        format!("{}{}", self.origin(), self.path())
    }
}

/// The hint for every target error.
const TARGET_HINT: &str = "a route forwards to a service of this lokal.toml by its full name, e.g. \
https://web.shop.lokal/{tenant}, or to http://localhost:<port>/…";

/// A route's target URL (SPEC §4.11): `https://<a lokal name><path>`, or
/// `http://localhost:<port><path>` (also `127.0.0.1` and `[::1]`).
/// Placeholders are allowed only in the path, and each must be one of
/// `captures`, the names in the route's key. No query, fragment, user or
/// other host. Whether a `https://` name is a service of the file is the
/// caller's to check.
pub fn route_target(input: &str, captures: &[&str]) -> Result<RouteTarget, UserError> {
    let s = input.trim();
    let err = |why: String| {
        UserError::usage(format!("invalid route target `{s}`: {why}")).hint(TARGET_HINT)
    };
    let lower = s.to_ascii_lowercase();
    let (https, rest) = if lower.starts_with("https://") {
        (true, &s["https://".len()..])
    } else if lower.starts_with("http://") {
        (false, &s["http://".len()..])
    } else {
        return Err(err("it must start with https:// or http://".into()));
    };
    if rest.contains(['?', '#']) {
        return Err(err("a route target has no query or fragment".into()));
    }
    let (authority, path) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
    if authority.contains(['{', '}']) {
        return Err(err("a placeholder can only be in the path".into()));
    }
    if authority.contains('@') {
        return Err(err("a route target has no user name".into()));
    }
    check_target_path(path, captures).map_err(err)?;
    let path = path.to_string();
    let authority = authority.to_ascii_lowercase();
    if https {
        if authority.contains(':') {
            return Err(err(
                "a service has no port here: lokal forwards to the port the service has".into(),
            ));
        }
        if authority.contains('*') {
            return Err(err(
                "a route forwards to one service, not to a wildcard service".into(),
            ));
        }
        let tld = authority.rsplit('.').next().unwrap_or_default();
        let valid = authority.len() <= 253 && authority.split('.').all(is_label);
        if !valid || !SUPPORTED_TLDS.contains(&tld) {
            return Err(err(format!(
                "https:// is only for a service of this lokal.toml, a .{DEFAULT_TLD} name"
            )));
        }
        return Ok(RouteTarget::Service {
            fqdn: authority,
            path,
        });
    }
    let (host, port) = match authority.strip_prefix('[') {
        Some(v6) => match v6.split_once("]:") {
            Some((h, p)) => (h, Some(p)),
            None => (v6.trim_end_matches(']'), None),
        },
        None => match authority.rsplit_once(':') {
            Some((h, p)) => (h, Some(p)),
            None => (authority.as_str(), None),
        },
    };
    if !matches!(host, "localhost" | "127.0.0.1" | "::1") {
        return Err(err(
            "http:// is only for a server on this machine: localhost, 127.0.0.1 or [::1]".into(),
        ));
    }
    let Some(port) = port else {
        return Err(err(format!(
            "it needs a port, e.g. http://{authority}:3000"
        )));
    };
    let port = match port.parse::<u32>() {
        Ok(n) => self::port(n, host).map_err(|e| err(e.message))?,
        Err(_) => return Err(err(format!("`{port}` is not a port"))),
    };
    Ok(RouteTarget::Local {
        host: host.to_string(),
        port,
        path,
    })
}

/// A target's path: empty or from `/`, each `{name}` one of `captures`,
/// and a valid URL path once they're filled in.
fn check_target_path(path: &str, captures: &[&str]) -> Result<(), String> {
    let mut filled = String::new();
    let mut rest = path;
    while let Some(open) = rest.find(['{', '}']) {
        filled.push_str(&rest[..open]);
        let tail = &rest[open..];
        let Some(close) = tail.strip_prefix('{').and_then(|t| t.find('}')) else {
            return Err("a `{` or `}` that isn't part of a {placeholder}".into());
        };
        let name = &tail[1..=close];
        if !captures.contains(&name) {
            return Err(format!(
                "{{{name}}} isn't a capture of the route; its captures: {}",
                captures
                    .iter()
                    .map(|c| format!("{{{c}}}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        filled.push('x');
        rest = &tail[close + 2..];
    }
    filled.push_str(rest);
    if filled.is_empty() {
        return Ok(());
    }
    match filled.parse::<http::uri::PathAndQuery>() {
        Ok(_) if filled.starts_with('/') => Ok(()),
        _ => Err(format!("`{path}` is not a URL path")),
    }
}

/// Fills a route's path template with what its captures matched
/// ([`captures`]): `/demosite/{site}` → `/demosite/acme.com`.
pub fn fill_path(template: &str, found: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(template.len() + 16);
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let tail = &rest[open + 1..];
        let close = tail.find('}').unwrap_or(tail.len());
        let name = &tail[..close];
        if let Some((_, v)) = found.iter().find(|(n, _)| *n == name) {
            out.push_str(v);
        }
        rest = tail.get(close + 1..).unwrap_or("");
    }
    out.push_str(rest);
    out
}

/// Host: hostname or IP, brackets stripped. Returns the host and an
/// optional warning (non-loopback).
pub fn host(input: &str) -> Result<(String, Option<String>), UserError> {
    let s = normalize(input);
    if s.contains("://") {
        return Err(
            UserError::usage(format!("host `{s}` must not include a scheme"))
                .hint("pass just the host name, e.g. --host localhost"),
        );
    }
    let unbracketed = s
        .strip_prefix('[')
        .and_then(|x| x.strip_suffix(']'))
        .map(str::to_string);
    let h = match unbracketed {
        Some(inner) => {
            if inner.parse::<std::net::Ipv6Addr>().is_err() {
                return Err(UserError::usage(format!("`{s}` is not an IPv6 address")));
            }
            inner
        }
        None => {
            if s.starts_with('[') {
                return Err(UserError::usage(format!("host `{s}` must not include a port"))
                    .hint("pass the port separately: lokal service add <SUBDOMAIN> <PORT> --host <HOST>"));
            }
            s
        }
    };
    if h.parse::<IpAddr>().is_err() {
        if h.contains(':') {
            return Err(
                UserError::usage(format!("host `{h}` must not include a port")).hint(
                    "pass the port separately: lokal service add <SUBDOMAIN> <PORT> --host <HOST>",
                ),
            );
        }
        let name = h.trim_end_matches('.');
        if name.is_empty() || name.len() > 253 || !name.split('.').all(is_label) {
            return Err(UserError::usage(format!(
                "`{h}` is not a valid host name or IP address"
            )));
        }
    }
    let warn = (!crate::net::is_loopback_host(&h))
        .then(|| format!("{h} is not a loopback address; lokal will proxy to it over the network"));
    Ok((h, warn))
}

pub fn port(port: u32, host: &str) -> Result<u16, UserError> {
    if port == 0 || port > 65535 {
        return Err(UserError::usage(format!(
            "port {port} is out of range (1–65535)"
        )));
    }
    if (port == 443 || port == 80) && crate::net::is_loopback_host(host) {
        return Err(UserError::usage(format!(
            "port {port} on {host} is lokal itself; requests would loop back into lokal"
        ))
        .hint("point the service at the port your dev server listens on"));
    }
    Ok(port as u16)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels() {
        assert!(is_label("api"));
        assert!(is_label("a-1"));
        assert!(!is_label("-a"));
        assert!(!is_label("a-"));
        assert!(!is_label(""));
        assert!(!is_label(&"a".repeat(64)));
        assert!(is_label(&"a".repeat(63)));
        assert!(!is_label("a_b"));
    }

    #[test]
    fn project_names() {
        assert_eq!(project_name("  MyProj ").unwrap(), "myproj");
        let e = project_name("my.proj").unwrap_err();
        let hint = e.hint.unwrap();
        assert!(hint.contains("--domain"), "{hint}");
    }

    #[test]
    fn domains() {
        assert_eq!(apex("x", None).unwrap(), "x.lokal");
        assert_eq!(apex("x", Some("x.LOKAL")).unwrap(), "x.lokal");
        assert_eq!(apex("x", Some("App.Lokal")).unwrap(), "app.lokal");
        assert_eq!(apex("x", Some("a.b.lokal")).unwrap(), "a.b.lokal");
        // The CA can't sign anything else, so nothing else is accepted.
        for d in ["x.com", "x.test", "x.internal"] {
            assert_eq!(
                apex("x", Some(d)).unwrap_err().message,
                "only .lokal is supported (other TLDs could shadow real websites)",
                "{d}"
            );
        }
        // Every TLD lokal serves is one the CA permits. The CA also permits
        // `.local` on purpose: narrowing it would replace every user's CA.
        for t in SUPPORTED_TLDS {
            assert!(crate::ca::PERMITTED_TLDS.contains(t), "{t}");
        }
        assert!(apex("x", Some("local")).is_err());
        assert!(apex("x", Some("lokal")).is_err());
        assert!(
            apex(
                "x",
                Some(&format!("{}.lokal", vec!["a".repeat(63); 4].join(".")))
            )
            .is_err()
        );
    }

    /// `.local` was lokal's TLD before 0.6.0: its own error, and the same
    /// name under `.lokal` as the hint.
    #[test]
    fn local_domains_point_at_lokal() {
        for (d, to) in [
            ("x.local", "x.lokal"),
            ("Shop.Acme.LOCAL.", "shop.acme.lokal"),
            ("local", "x.lokal"),
        ] {
            let e = apex("x", Some(d)).unwrap_err();
            assert_eq!(e.code, crate::error::EXIT_USAGE, "{d}");
            assert_eq!(e.message, ".local names aren't supported since 0.6.0");
            assert_eq!(e.hint.unwrap(), format!("use --domain {to}"));
        }
        let to = |d| lokal_for_local("x", d);
        assert_eq!(to("shop.local").as_deref(), Some("shop.lokal"));
        assert_eq!(to("local").as_deref(), Some("x.lokal"));
        assert_eq!(to("shop.lokal"), None);
        assert_eq!(to("mylocal"), None);
        assert_eq!(to("shop.mylocal"), None);
    }

    #[test]
    fn served_domains() {
        assert!(is_served("shop.lokal"));
        assert!(is_served("api.shop.lokal"));
        assert!(!is_served("shop.local"));
        assert!(!is_served("shop.lokal.local"));
        let e = unserved("shop", "shop.local");
        assert_eq!(
            e.message,
            "project `shop` is on shop.local, and lokal serves only .lokal names since 0.6.0"
        );
        assert_eq!(
            e.hint.unwrap(),
            "move it to .lokal: lokal project remove shop --force, then lokal project create shop \
and add its services again (or run lokal up, if its repo has a lokal.toml)"
        );
    }

    #[test]
    fn overlap() {
        assert!(apex_overlaps("app.lokal", "api.app.lokal"));
        assert!(apex_overlaps("api.app.lokal", "app.lokal"));
        assert!(apex_overlaps("app.lokal", "app.lokal"));
        assert!(!apex_overlaps("myapp.lokal", "app.lokal"));
    }

    #[test]
    fn subdomains() {
        assert_eq!(subdomain("api", "p.lokal").unwrap(), "api");
        assert_eq!(subdomain("API.p.lokal", "p.lokal").unwrap(), "api");
        assert_eq!(subdomain("v1.api", "p.lokal").unwrap(), "v1.api");
        assert_eq!(subdomain("@", "p.lokal").unwrap(), "@");
        assert_eq!(subdomain("p.lokal", "p.lokal").unwrap(), "@");
        assert!(subdomain("a..b", "p.lokal").is_err());
        assert!(subdomain("-a", "p.lokal").is_err());
    }

    /// `*` is only ever the whole first label (SPEC §4.3).
    #[test]
    fn wildcard_subdomains() {
        for (input, want) in [
            ("*", "*"),
            ("*.v1", "*.v1"),
            (" *.V1.Api ", "*.v1.api"),
            ("*.p.lokal", "*"),
            ("*.v1.p.lokal.", "*.v1"),
        ] {
            assert_eq!(subdomain(input, "p.lokal").unwrap(), want, "{input}");
        }
        for input in ["a*", "*a", "a.*", "*.*", "**", "v1.*.api", "*-a.b"] {
            let e = subdomain(input, "p.lokal").unwrap_err();
            assert_eq!(e.code, crate::error::EXIT_USAGE, "{input}");
            assert!(
                e.message.ends_with("`*` can only be the whole first label"),
                "{input}: {}",
                e.message
            );
        }
        // The labels after it follow the usual rules.
        let e = subdomain("*.-a", "p.lokal").unwrap_err();
        assert_eq!(e.message, label_error("subdomain label", "-a").message);
        assert!(subdomain("*.", "p.lokal").is_ok_and(|s| s == "*"));
        assert!(subdomain(&format!("*.{}", "a".repeat(250)), "p.lokal").is_err());
    }

    /// Decisions 2, 3 and 5: any depth, never the base, labels only.
    #[test]
    fn what_a_wildcard_covers() {
        assert_eq!(wildcard_base("*.shop.lokal"), Some("shop.lokal"));
        assert_eq!(wildcard_base("*.v1.shop.lokal"), Some("v1.shop.lokal"));
        for not_a_pattern in [
            "shop.lokal",
            "a.shop.lokal",
            "*",
            "*.",
            "a*.shop.lokal",
            "*.*.p.lokal",
        ] {
            assert_eq!(wildcard_base(not_a_pattern), None, "{not_a_pattern}");
        }

        let p = "*.shop.lokal";
        for host in [
            "a.shop.lokal",
            "b.a.shop.lokal",
            "v1.shop.lokal",
            "x-1.y.z.shop.lokal",
        ] {
            assert!(covers(p, host), "{host}");
        }
        for host in [
            "shop.lokal",
            "xshop.lokal",
            ".shop.lokal",
            "a..shop.lokal",
            "a_b.shop.lokal",
            "*.shop.lokal",
            "a.*.shop.lokal",
            "-a.shop.lokal",
            "A.shop.lokal",
            "a.shop.lokal.other.lokal",
            "a.other.lokal",
        ] {
            assert!(!covers(p, host), "{host}");
        }
        assert!(!covers(
            p,
            &format!(
                "{}.{}.shop.lokal",
                "a".repeat(63),
                vec!["b".repeat(63); 3].join(".")
            )
        ));

        let v1 = "*.v1.shop.lokal";
        assert!(covers(v1, "x.v1.shop.lokal"));
        assert!(covers(v1, "y.x.v1.shop.lokal"));
        assert!(!covers(v1, "v1.shop.lokal"));
        assert!(!covers(v1, "x.shop.lokal"));
        // A name that isn't a pattern covers nothing, not even itself.
        assert!(!covers("a.shop.lokal", "a.shop.lokal"));
        assert!(!covers("*.*.shop.lokal", "x.*.shop.lokal"));
    }

    /// Decision 2 of the routes plan (SPEC §4.11): labels and whole-label
    /// captures, relative to the apex, at least one capture, never the apex.
    #[test]
    fn route_keys() {
        for (input, want) in [
            ("{site}", "{site}"),
            ("preview.{slug}", "preview.{slug}"),
            (" Preview.{slug} ", "preview.{slug}"),
            ("{site}.shop.lokal", "{site}"),
            ("{site}.SHOP.lokal.", "{site}"),
            ("{a}.x.{b}", "{a}.x.{b}"),
            ("{tenant_1}.v1", "{tenant_1}.v1"),
        ] {
            assert_eq!(route_key(input, "shop.lokal").unwrap(), want, "{input}");
        }
        let why = |input: &str| {
            let e = route_key(input, "shop.lokal").unwrap_err();
            assert_eq!(e.code, crate::error::EXIT_USAGE, "{input}");
            e.message
        };
        for input in ["pre-{x}", "{x}y", "{{x}}", "a.{x}y.b"] {
            assert!(why(input).contains("a capture is a whole label"), "{input}");
        }
        assert_eq!(
            why("{}"),
            "invalid route `{}`: a capture needs a name, e.g. {tenant}"
        );
        for input in ["{X}", "{1a}", "{a-b}", "{_a}"] {
            assert!(why(input).contains("must be lowercase letters"), "{input}");
        }
        for input in ["*", "*.v1", "{a}.*"] {
            assert!(why(input).contains("not `*`"), "{input}");
        }
        assert_eq!(
            why("{a}.x.{a}"),
            "invalid route `{a}.x.{a}`: the capture {a} is used twice"
        );
        for input in ["@", "shop.lokal", "SHOP.lokal."] {
            let e = route_key(input, "shop.lokal").unwrap_err();
            assert!(e.message.contains("never covers the apex"), "{input}");
            assert_eq!(e.hint.unwrap(), "an exact name belongs in [services]");
        }
        let e = route_key("old", "shop.lokal").unwrap_err();
        assert_eq!(
            e.message,
            "`old` has no capture; a route needs one, e.g. \"{tenant}.old\""
        );
        assert_eq!(
            why("{site}/x"),
            "invalid route `{site}/x`: a route is a host pattern, without a path"
        );
        assert!(why("a_b.{x}").contains("invalid route label `a_b`"));
        assert!(why(" ").contains("can't be empty"));
        let long = format!("{{x}}.{}", vec!["a".repeat(63); 4].join("."));
        assert!(why(&long).contains("longer than 253"));
    }

    /// Decision 3: a capture on the left takes one or more labels, any
    /// other exactly one, and a literal on the left pins the depth.
    #[test]
    fn what_a_route_covers_and_captures() {
        let site = "{site}.shop.lokal";
        assert_eq!(
            captures(site, "acme.com.shop.lokal"),
            Some(vec![("site", "acme.com")])
        );
        assert_eq!(captures(site, "a.shop.lokal"), Some(vec![("site", "a")]));
        assert_eq!(
            captures(site, "x.y.z.shop.lokal"),
            Some(vec![("site", "x.y.z")])
        );
        assert_eq!(captures(site, "shop.lokal"), None);
        assert_eq!(captures(site, "a.other.lokal"), None);

        let preview = "preview.{slug}.shop.lokal";
        assert_eq!(
            captures(preview, "preview.acme.shop.lokal"),
            Some(vec![("slug", "acme")])
        );
        for host in [
            "a.preview.acme.shop.lokal",
            "preview.a.b.shop.lokal",
            "preview.shop.lokal",
            "x.acme.shop.lokal",
        ] {
            assert_eq!(captures(preview, host), None, "{host}");
        }

        let two = "{a}.x.{b}.shop.lokal";
        assert_eq!(
            captures(two, "p.q.x.r.shop.lokal"),
            Some(vec![("a", "p.q"), ("b", "r")])
        );
        assert_eq!(captures(two, "x.r.shop.lokal"), None);

        // A wildcard is an anonymous capture on the left.
        assert_eq!(
            captures("*.v1.shop.lokal", "a.b.v1.shop.lokal"),
            Some(vec![("*", "a.b")])
        );

        // A forged Host can't put anything but labels into a capture.
        for host in [
            "a_b.shop.lokal",
            "..shop.lokal",
            "a..b.shop.lokal",
            "%2e.shop.lokal",
            "%2e%2e.shop.lokal",
            ".shop.lokal",
            "a/b.shop.lokal",
            "a?b.shop.lokal",
            "{site}.shop.lokal",
            "A.shop.lokal",
            "preview..shop.lokal",
        ] {
            assert_eq!(captures(site, host), None, "{host}");
            assert_eq!(captures(preview, host), None, "{host}");
        }
        let at_limit = format!("{}.shop.lokal", vec!["a".repeat(60); 4].join("."));
        assert_eq!(at_limit.len(), 254);
        assert_eq!(captures(site, &at_limit), None);
        assert!(covers(site, &at_limit[1..]));

        // Not patterns: no capture, a bare `*`, a capture that isn't a label.
        for p in ["a.shop.lokal", "*", "{x}", "{x}y.shop.lokal", "a.*.lokal"] {
            assert!(!covers(p, "a.shop.lokal"), "{p}");
        }
    }

    /// Decision 4: an exact name is the route table's business; among
    /// patterns, a literal beats a capture, from the base outward, and a
    /// longer pattern beats a shorter one.
    #[test]
    fn pattern_precedence_and_shape() {
        let mut v = vec![
            "*.p.lokal",
            "{site}.q.lokal",
            "preview.{slug}.q.lokal",
            "*.v1.p.lokal",
            "{a}.{b}.q.lokal",
            "{a}.x.q.lokal",
        ];
        v.sort_by(|a, b| pattern_order(a, b));
        let pos = |p: &str| v.iter().position(|x| *x == p).unwrap();
        assert!(pos("*.v1.p.lokal") < pos("*.p.lokal"), "{v:?}");
        assert!(
            pos("preview.{slug}.q.lokal") < pos("{site}.q.lokal"),
            "{v:?}"
        );
        assert!(pos("{a}.x.q.lokal") < pos("{a}.{b}.q.lokal"), "{v:?}");
        assert!(pos("{a}.{b}.q.lokal") < pos("{site}.q.lokal"), "{v:?}");

        assert_eq!(pattern_shape("*.p.lokal").unwrap(), "{}.p.lokal");
        assert_eq!(pattern_shape("{site}.p.lokal"), pattern_shape("*.p.lokal"));
        assert_eq!(
            pattern_shape("{a}.x.p.lokal"),
            pattern_shape("{b}.x.p.lokal")
        );
        assert_ne!(
            pattern_shape("{a}.x.p.lokal"),
            pattern_shape("{a}.y.p.lokal")
        );
        assert_ne!(pattern_shape("*.v1.p.lokal"), pattern_shape("*.p.lokal"));
        assert_eq!(pattern_shape("api.p.lokal"), None);

        assert_eq!(capture_names("{a}.x.{b}"), ["a", "b"]);
        assert_eq!(
            fill_captures("preview.{slug}.shop.lokal", "lokal-doctor"),
            "preview.lokal-doctor.shop.lokal"
        );
        assert_eq!(
            fill_captures("*.v1.shop.lokal", "lokal-doctor"),
            "lokal-doctor.v1.shop.lokal"
        );
    }

    /// Decision 5: a service of the file by its full name, or a server on
    /// this machine; placeholders only in the path, and only captures.
    #[test]
    fn route_targets() {
        let caps = ["site"];
        let t = route_target("https://render.shop.lokal/demosite/{site}", &caps).unwrap();
        assert_eq!(
            t,
            RouteTarget::Service {
                fqdn: "render.shop.lokal".into(),
                path: "/demosite/{site}".into()
            }
        );
        assert_eq!(t.origin(), "https://render.shop.lokal");
        assert_eq!(t.host(), "render.shop.lokal");
        assert_eq!(t.url(), "https://render.shop.lokal/demosite/{site}");
        let t = route_target(" HTTPS://Render.Shop.LOKAL ", &caps).unwrap();
        assert_eq!(t.url(), "https://render.shop.lokal");
        assert_eq!(t.path(), "");

        for (input, host, url) in [
            (
                "http://localhost:3000/preview/{site}",
                "localhost:3000",
                "http://localhost:3000/preview/{site}",
            ),
            (
                "http://127.0.0.1:3000/",
                "127.0.0.1:3000",
                "http://127.0.0.1:3000/",
            ),
            (
                "http://[::1]:3000/a/{site}/b",
                "[::1]:3000",
                "http://[::1]:3000/a/{site}/b",
            ),
            (
                "HTTP://LOCALHOST:8080",
                "localhost:8080",
                "http://localhost:8080",
            ),
        ] {
            let t = route_target(input, &caps).unwrap();
            assert_eq!(t.host(), host, "{input}");
            assert_eq!(t.url(), url, "{input}");
            assert_eq!(t.origin(), format!("http://{host}"), "{input}");
        }
        assert!(matches!(
            route_target("http://[::1]:3000", &caps).unwrap(),
            RouteTarget::Local { ref host, port: 3000, .. } if host == "::1"
        ));

        let why = |input: &str| {
            let e = route_target(input, &caps).unwrap_err();
            assert_eq!(e.code, crate::error::EXIT_USAGE, "{input}");
            assert_eq!(e.hint.as_deref(), Some(TARGET_HINT), "{input}");
            e.message
        };
        for (input, says) in [
            ("ftp://x.shop.lokal/", "must start with https:// or http://"),
            ("render.shop.lokal/x", "must start with https:// or http://"),
            (
                "https://example.com/x",
                "only for a service of this lokal.toml",
            ),
            (
                "https://localhost/x",
                "only for a service of this lokal.toml",
            ),
            (
                "https://render.shop.lokal:8443/x",
                "a service has no port here",
            ),
            ("https://*.shop.lokal/x", "not to a wildcard service"),
            (
                "https://{site}.shop.lokal/x",
                "a placeholder can only be in the path",
            ),
            ("https://render.shop.lokal/x?y=1", "no query or fragment"),
            ("https://render.shop.lokal/x#top", "no query or fragment"),
            ("https://u@render.shop.lokal/x", "no user name"),
            (
                "http://example.com:3000/x",
                "only for a server on this machine",
            ),
            (
                "http://192.168.1.5:3000/x",
                "only for a server on this machine",
            ),
            (
                "http://localhost/x",
                "it needs a port, e.g. http://localhost:3000",
            ),
            ("http://localhost:443/x", "lokal itself"),
            ("http://localhost:0/x", "out of range"),
            ("http://localhost:x/x", "`x` is not a port"),
            (
                "https://render.shop.lokal/{slug}",
                "{slug} isn't a capture of the route; its captures: {site}",
            ),
            ("https://render.shop.lokal/{Site}", "isn't a capture"),
            (
                "https://render.shop.lokal/a}b",
                "isn't part of a {placeholder}",
            ),
            (
                "https://render.shop.lokal/{site",
                "isn't part of a {placeholder}",
            ),
            ("https://render.shop.lokal/a b", "is not a URL path"),
        ] {
            let msg = why(input);
            assert!(
                msg.starts_with(&format!("invalid route target `{}`: ", input.trim())),
                "{msg}"
            );
            assert!(msg.contains(says), "{input}: {msg}");
        }
    }

    #[test]
    fn paths_are_filled_with_what_was_captured() {
        let found = [("site", "acme.com"), ("slug", "x")];
        assert_eq!(fill_path("/demosite/{site}", &found), "/demosite/acme.com");
        assert_eq!(fill_path("/{slug}/{site}/", &found), "/x/acme.com/");
        assert_eq!(fill_path("", &found), "");
        assert_eq!(fill_path("/plain", &found), "/plain");
    }

    #[test]
    fn hosts() {
        assert_eq!(host("localhost").unwrap(), ("localhost".into(), None));
        assert_eq!(host("[::1]").unwrap().0, "::1");
        assert_eq!(host("::1").unwrap().0, "::1");
        let e = host("http://localhost").unwrap_err();
        assert!(e.message.contains("scheme"), "{e}");
        for with_port in ["localhost:3000", "[::1]:3000"] {
            let e = host(with_port).unwrap_err();
            assert!(e.message.contains("port"), "{with_port}: {e}");
        }
        assert!(host("192.168.1.5").unwrap().1.is_some());
        assert!(host("bad host").is_err());
    }

    #[test]
    fn ports() {
        assert!(port(0, "localhost").is_err());
        assert!(port(70000, "localhost").is_err());
        assert!(port(443, "localhost").is_err());
        assert!(port(80, "127.0.0.1").is_err());
        assert!(port(443, "10.0.0.2").is_ok());
        assert_eq!(port(4000, "localhost").unwrap(), 4000);
    }
}

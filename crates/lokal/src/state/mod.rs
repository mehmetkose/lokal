//! `state.toml` (SPEC §5.2): schema, atomic save, and locked
//! read-modify-write. Only the CLI writes it; the daemon only reads it.

pub mod validate;

use std::collections::BTreeMap;
use std::fs;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::paths::Paths;

pub const STATE_VERSION: u32 = 1;

/// The `managed` value of a service that `lokal up` added from a
/// `lokal.toml`, or adopted because its route was the file's (SPEC §5.2).
/// `up` updates and removes only these, and `down` removes them.
pub const MANAGED_BY_FILE: &str = "lokal.toml";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HostHeader {
    #[default]
    Preserve,
    Upstream,
}

impl HostHeader {
    pub fn as_str(self) -> &'static str {
        match self {
            HostHeader::Preserve => "preserve",
            HostHeader::Upstream => "upstream",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Service {
    pub host: String,
    pub port: u16,
    #[serde(default)]
    pub host_header: HostHeader,
    /// `Some("lokal.toml")` ([`MANAGED_BY_FILE`]) for a service that `lokal
    /// up` added or adopted. Absent means added by hand or by `lokal run`;
    /// `down` never touches those, and `up` only adopts one whose route is
    /// the one the file gives it (SPEC §4.11). Not written when absent, so
    /// state for hand-added services is the same as before the field
    /// existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub managed: Option<String>,
    /// Fields this lokal doesn't know, kept so that rewriting state doesn't
    /// erase a newer lokal's fields (SPEC §5.2).
    #[serde(flatten)]
    pub extra: toml::Table,
}

impl Service {
    /// Did `lokal up` add or adopt it (from a `lokal.toml`)?
    pub fn is_managed(&self) -> bool {
        self.managed.as_deref() == Some(MANAGED_BY_FILE)
    }

    /// Same route: host, port and host-header mode. Who added the service
    /// doesn't matter, so re-adding a managed service's route by hand is
    /// "already configured".
    pub fn same_route(&self, other: &Service) -> bool {
        self.host == other.host && self.port == other.port && self.host_header == other.host_header
    }

    /// `localhost:4000`, `[::1]:4000`
    pub fn upstream(&self) -> String {
        if self.host.contains(':') {
            format!("[{}]:{}", self.host, self.port)
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Project {
    pub domain: String,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub services: BTreeMap<String, Service>,
    /// Fields this lokal doesn't know, kept so that rewriting state doesn't
    /// erase a newer lokal's fields (SPEC §5.2).
    #[serde(flatten)]
    pub extra: toml::Table,
}

impl Project {
    /// Full hostname for a subdomain (`@` is the apex).
    pub fn fqdn(&self, sub: &str) -> String {
        fqdn(sub, &self.domain)
    }
}

pub fn fqdn(sub: &str, domain: &str) -> String {
    if sub == "@" {
        domain.to_string()
    } else {
        format!("{sub}.{domain}")
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct State {
    pub version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_project: Option<String>,
    #[serde(default)]
    pub projects: BTreeMap<String, Project>,
    /// Fields this lokal doesn't know, kept so that rewriting state doesn't
    /// erase a newer lokal's fields (SPEC §5.2).
    #[serde(flatten)]
    pub extra: toml::Table,
}

impl Default for State {
    fn default() -> Self {
        Self {
            version: STATE_VERSION,
            current_project: None,
            projects: BTreeMap::new(),
            extra: toml::Table::new(),
        }
    }
}

/// One configured service, flattened.
#[derive(Debug, Clone, PartialEq)]
pub struct ServiceEntry {
    pub project: String,
    pub domain: String,
    pub subdomain: String,
    pub fqdn: String,
    pub service: Service,
}

impl State {
    pub fn parse(text: &str) -> Result<Self> {
        let st: State = toml::from_str(text).context("state.toml is not valid")?;
        if st.version > STATE_VERSION {
            bail!(crate::error::UserError::runtime(
                "state file was written by a newer lokal; run a newer lokal"
            ));
        }
        Ok(st)
    }

    /// Reads the state file without locking (writes are atomic renames).
    /// A missing file is an empty state.
    pub fn load(paths: &Paths) -> Result<Self> {
        match fs::read_to_string(&paths.state) {
            Ok(text) => Self::parse(&text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).with_context(|| format!("cannot read {}", paths.state.display())),
        }
    }

    /// Atomic write: `state.toml.tmp`, fsync, rename. Caller holds the lock.
    fn save(&self, paths: &Paths) -> Result<()> {
        paths.ensure_base()?;
        let text = toml::to_string_pretty(self)?;
        let tmp = &paths.state_tmp;
        {
            use std::io::Write;
            let mut f =
                fs::File::create(tmp).with_context(|| format!("cannot write {}", tmp.display()))?;
            f.write_all(text.as_bytes())?;
            f.sync_all()?;
        }
        fs::rename(tmp, &paths.state)
            .with_context(|| format!("cannot replace {}", paths.state.display()))?;
        Ok(())
    }

    /// Locked read-modify-write. `f` may fail, in which case nothing is
    /// written.
    pub fn update<T>(paths: &Paths, f: impl FnOnce(&mut State) -> Result<T>) -> Result<T> {
        paths.ensure_base()?;
        let _lock = crate::util::lock_exclusive(&paths.state_lock)?;
        let mut st = Self::load(paths)?;
        let before = st.clone();
        let out = f(&mut st)?;
        if st != before {
            st.save(paths)?;
        }
        Ok(out)
    }

    pub fn services(&self) -> Vec<ServiceEntry> {
        let mut v = vec![];
        for (pname, p) in &self.projects {
            for (sub, s) in &p.services {
                v.push(ServiceEntry {
                    project: pname.clone(),
                    domain: p.domain.clone(),
                    subdomain: sub.clone(),
                    fqdn: p.fqdn(sub),
                    service: s.clone(),
                });
            }
        }
        v
    }

    /// Project selection order of SPEC §4.4.
    pub fn resolve_project(&self, explicit: Option<&str>) -> Result<String> {
        if let Some(p) = explicit {
            let p = validate::normalize(p);
            if !self.projects.contains_key(&p) {
                bail!(no_such_project(&p));
            }
            return Ok(p);
        }
        if let Some(cur) = &self.current_project
            && self.projects.contains_key(cur)
        {
            return Ok(cur.clone());
        }
        if self.projects.len() == 1 {
            return Ok(self.projects.keys().next().cloned().unwrap_or_default());
        }
        bail!(crate::error::UserError::runtime(
            "no project selected: run 'lokal project use <NAME>' or pass -p <NAME>"
        ))
    }
}

pub fn no_such_project(name: &str) -> crate::error::UserError {
    crate::error::UserError::runtime(format!("no project named `{name}`"))
        .hint("run 'lokal project list' to see your projects")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_spec_example_and_ignores_unknown_fields() {
        let text = r#"
version = 1
current_project = "myproject"
future_thing = true

[projects.myproject]
domain = "myproject.local"
created_at = "2026-10-10T12:00:00Z"
color = "blue"

[projects.myproject.services.api]
host = "localhost"
port = 4000
host_header = "preserve"
weight = 3

[projects.myproject.services."@"]
host = "localhost"
port = 3000
host_header = "upstream"
"#;
        let st = State::parse(text).unwrap();
        let p = &st.projects["myproject"];
        assert_eq!(p.services["api"].port, 4000);
        assert_eq!(p.services["@"].host_header, HostHeader::Upstream);
        assert_eq!(p.fqdn("@"), "myproject.local");
        assert_eq!(st.services().len(), 2);
        // Each unknown field is kept at its own level, and only there.
        assert_eq!(st.extra, toml::toml! { future_thing = true });
        assert_eq!(p.extra, toml::toml! { color = "blue" });
        assert_eq!(p.services["api"].extra, toml::toml! { weight = 3 });
        assert!(p.services["@"].extra.is_empty());
    }

    /// A newer lokal's fields, at every level, survive an older lokal
    /// rewriting the file. Every default is spelled out, or the raw tables
    /// would differ by the defaults lokal fills in.
    #[test]
    fn unknown_fields_survive_a_rewrite() {
        let text = r#"version = 1
current_project = "a"
future = 1
extra = "a key, not the Rust field"
saved_at = 2026-10-26T10:00:00Z

[future_table]
list = [1, 2]

[projects.a]
domain = "a.lokal"
created_at = "2026-10-10T12:00:00Z"
color = "blue"
pinned_at = 2026-10-26T10:00:00

[projects.a.services.api]
host = "localhost"
port = 4000
host_header = "preserve"
managed = "lokal.toml"
weight = 0.5
tags = ["x", "y"]
limits = { rps = 10 }

[projects.a.services.api.health]
path = "/healthz"
interval_ms = 500

[[projects.a.services.api.hooks]]
on = "start"
run = "echo hi"

[[projects.a.services.api.hooks]]
on = "stop"
run = "echo bye"

[projects.a.services."@"]
host = "::1"
port = 3000
host_header = "upstream"
extra = { nested = true }

[projects.b]
domain = "b.local"
created_at = "2026-10-10T12:00:00Z"

[projects.b.services]
"#;
        let first = State::parse(text).unwrap();
        let keys = |t: &toml::Table| t.keys().cloned().collect::<Vec<_>>();
        assert_eq!(
            keys(&first.extra),
            ["extra", "future", "future_table", "saved_at"]
        );
        let a = &first.projects["a"];
        assert_eq!(keys(&a.extra), ["color", "pinned_at"]);
        assert_eq!(
            keys(&a.services["api"].extra),
            ["health", "hooks", "limits", "tags", "weight"]
        );
        assert_eq!(
            a.services["@"].extra,
            toml::toml! { extra = { nested = true } }
        );
        assert!(first.projects["b"].extra.is_empty());

        let out = toml::to_string_pretty(&first).unwrap();
        // The same data as the input: nothing lost, nothing moved to another
        // level, nothing added.
        assert_eq!(
            out.parse::<toml::Table>().unwrap(),
            text.parse::<toml::Table>().unwrap(),
            "{out}"
        );
        assert_eq!(State::parse(&out).unwrap(), first);
    }

    /// The SPEC §5.2 example, comments and all.
    const SPEC_EXAMPLE: &str = r#"version = 1
current_project = "myproject"

[projects.myproject]
domain = "myproject.local"
created_at = "2026-10-10T12:00:00Z"

[projects.myproject.services.api]
host = "localhost"
port = 4000
host_header = "preserve"   # or "upstream"

[projects.myproject.services."@"]
host = "localhost"
port = 3000
host_header = "preserve"

[projects.myproject.services.web]
host = "localhost"
port = 51234
host_header = "preserve"
managed = "lokal.toml"     # only on services that lokal up added (§4.11)
"#;

    /// What lokal writes for state without unknown fields, pinned so that a
    /// change to the structs can't change the file people already have.
    #[test]
    fn state_without_unknown_fields_is_written_as_before() {
        let st = State::parse(SPEC_EXAMPLE).unwrap();
        assert_eq!(
            toml::to_string_pretty(&st).unwrap(),
            r#"version = 1
current_project = "myproject"

[projects.myproject]
domain = "myproject.local"
created_at = "2026-10-10T12:00:00Z"

[projects.myproject.services."@"]
host = "localhost"
port = 3000
host_header = "preserve"

[projects.myproject.services.api]
host = "localhost"
port = 4000
host_header = "preserve"

[projects.myproject.services.web]
host = "localhost"
port = 51234
host_header = "preserve"
managed = "lokal.toml"
"#
        );

        // Shapes the example lacks: a project without services (what
        // `project create` writes), an IPv6 host, `upstream`, no current
        // project.
        let text = "version = 1\n\
[projects.a]\ndomain = \"a.lokal\"\ncreated_at = \"2026-10-10T12:00:00Z\"\n\
[projects.b]\ndomain = \"b.local\"\ncreated_at = \"\"\n\
[projects.b.services.x]\nhost = \"::1\"\nport = 1\nhost_header = \"upstream\"\n";
        assert_eq!(
            toml::to_string_pretty(&State::parse(text).unwrap()).unwrap(),
            r#"version = 1

[projects.a]
domain = "a.lokal"
created_at = "2026-10-10T12:00:00Z"

[projects.a.services]

[projects.b]
domain = "b.local"
created_at = ""

[projects.b.services.x]
host = "::1"
port = 1
host_header = "upstream"
"#
        );

        // An empty state.
        assert_eq!(
            toml::to_string_pretty(&State::default()).unwrap(),
            "version = 1\n\n[projects]\n"
        );
    }

    #[test]
    fn newer_version_is_rejected() {
        let err = State::parse("version = 2").unwrap_err();
        assert!(err.to_string().contains("newer lokal"), "{err}");
    }

    #[test]
    fn managed_marker_round_trips_and_is_absent_for_hand_added_services() {
        let text = "version = 1\n[projects.a]\ndomain = \"a.lokal\"\n\
[projects.a.services.web]\nhost = \"localhost\"\nport = 51234\nhost_header = \"preserve\"\nmanaged = \"lokal.toml\"\n\
[projects.a.services.api]\nhost = \"localhost\"\nport = 4000\n";
        let st = State::parse(text).unwrap();
        let p = &st.projects["a"];
        assert!(p.services["web"].is_managed());
        assert_eq!(p.services["web"].managed.as_deref(), Some(MANAGED_BY_FILE));
        assert!(!p.services["api"].is_managed());
        assert_eq!(p.services["api"].managed, None);
        let out = toml::to_string_pretty(&st).unwrap();
        // `None` isn't written: state for hand-added services is unchanged.
        assert_eq!(out.matches("managed").count(), 1, "{out}");
        assert_eq!(State::parse(&out).unwrap(), st);
        // Another value isn't lokal.toml's.
        let other = State::parse(&text.replace("\"lokal.toml\"", "\"other\"")).unwrap();
        assert!(!other.projects["a"].services["web"].is_managed());
    }

    #[test]
    fn same_route_ignores_who_added_the_service() {
        let hand = Service {
            host: "localhost".into(),
            port: 4000,
            host_header: HostHeader::Preserve,
            managed: None,
            extra: toml::Table::new(),
        };
        let file = Service {
            managed: Some(MANAGED_BY_FILE.into()),
            ..hand.clone()
        };
        assert!(hand.same_route(&file));
        assert_ne!(hand, file);
        for other in [
            Service {
                port: 4001,
                ..hand.clone()
            },
            Service {
                host: "::1".into(),
                ..hand.clone()
            },
            Service {
                host_header: HostHeader::Upstream,
                ..hand.clone()
            },
        ] {
            assert!(!hand.same_route(&other), "{other:?}");
        }
    }

    #[test]
    fn missing_host_header_defaults_to_preserve() {
        let st = State::parse(
            "version = 1\n[projects.a]\ndomain = \"a.local\"\n[projects.a.services.x]\nhost = \"localhost\"\nport = 1\n",
        )
        .unwrap();
        assert_eq!(
            st.projects["a"].services["x"].host_header,
            HostHeader::Preserve
        );
    }
}

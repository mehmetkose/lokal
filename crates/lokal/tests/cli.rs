//! CLI and state rules (SPEC §4.3–§4.8, M2) in a `LOKAL_HOME` sandbox.
//! No daemon runs here, so mutating commands save state and print the
//! "background service isn't running" note.

use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::Value;

struct Sandbox {
    dir: tempfile::TempDir,
}

impl Sandbox {
    fn new() -> Self {
        Self {
            dir: tempfile::Builder::new().prefix("lk").tempdir().unwrap(),
        }
    }

    fn cmd(&self) -> Command {
        let mut c = Command::cargo_bin("lokal").unwrap();
        c.env("LOKAL_HOME", self.dir.path())
            .env("NO_COLOR", "1")
            .env_remove("LOKAL_FORBID_SUDO");
        c
    }

    fn run(&self, args: &[&str]) -> assert_cmd::assert::Assert {
        self.cmd().args(args).assert()
    }

    fn json(&self, args: &[&str]) -> Value {
        let out = self.cmd().args(args).output().unwrap();
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).unwrap()
    }

    fn state(&self) -> String {
        std::fs::read_to_string(self.dir.path().join("state.toml")).unwrap_or_default()
    }
}

#[test]
fn help_lists_every_command_and_has_examples() {
    let s = Sandbox::new();
    let out = s.run(&["--help"]).success().get_output().stdout.clone();
    let text = String::from_utf8(out).unwrap();
    for c in [
        "setup",
        "project create <NAME>",
        "service add <SUBDOMAIN> <PORT>",
        "start | stop | restart",
        "ca path",
    ] {
        assert!(text.contains(c), "{c}");
    }
    // `lokal help` prints the same root help.
    let help = s.run(&["help"]).success().get_output().stdout.clone();
    assert_eq!(String::from_utf8(help).unwrap().trim(), text.trim());
    // Every subcommand has an Examples section.
    for args in [
        &["setup", "--help"][..],
        &["project", "create", "--help"],
        &["service", "add", "--help"],
        &["service", "update", "--help"],
        &["run", "--help"],
        &["list", "--help"],
        &["env", "--help"],
        &["ca", "path", "--help"],
        &["open", "--help"],
        &["logs", "--help"],
    ] {
        s.run(args)
            .success()
            .stdout(predicate::str::contains("Examples:"));
    }
    s.run(&["service", "add", "--help"])
        .success()
        .stdout(predicate::str::contains("allowedHosts"))
        .stdout(predicate::str::contains("--host-header upstream"));
}

#[test]
fn version_format() {
    let s = Sandbox::new();
    let out = s.run(&["--version"]).success().get_output().stdout.clone();
    let v = String::from_utf8(out).unwrap();
    let re = regex_lite(&v);
    assert!(re, "unexpected version line {v:?}");
}

/// `lokal 0.2.0 (abc1234 2026-10-10T12:00:00Z)`, with this package's version.
fn regex_lite(v: &str) -> bool {
    let v = v.trim();
    let Some(rest) = v.strip_prefix(concat!("lokal ", env!("CARGO_PKG_VERSION"), " (")) else {
        return false;
    };
    let Some(inner) = rest.strip_suffix(')') else {
        return false;
    };
    let mut it = inner.split(' ');
    let (Some(hash), Some(date), None) = (it.next(), it.next(), it.next()) else {
        return false;
    };
    hash.len() == 7 && date.len() == 20 && date.ends_with('Z')
}

#[test]
fn no_args_prints_help_and_exits_zero() {
    let s = Sandbox::new();
    s.run(&[])
        .success()
        .stdout(predicate::str::contains("Usage: lokal <COMMAND>"));
}

#[test]
fn project_create_and_current_project() {
    let s = Sandbox::new();
    s.run(&["project", "create", "  MyProj "])
        .success()
        .stdout(predicate::str::contains(
            "Created myproj.lokal (current project)",
        ))
        .stdout(predicate::str::contains("background service isn't running"));
    let v = s.json(&["list", "--json"]);
    assert_eq!(v["schema"], 1);
    assert_eq!(v["current_project"], "myproj");
    assert_eq!(v["projects"][0]["domain"], "myproj.lokal");
}

#[test]
fn project_create_is_safe_to_rerun() {
    let s = Sandbox::new();
    s.run(&["project", "create", "one"]).success();
    s.run(&["project", "create", "two"]).success();
    assert_eq!(s.json(&["list", "--json"])["current_project"], "two");
    let before = s.state();
    // Same arguments again: exit 0, nothing changes except the current project.
    s.run(&["project", "create", "one"])
        .success()
        .stdout(predicate::str::contains(
            "one.lokal already exists (current project)",
        ));
    assert_eq!(s.json(&["list", "--json"])["current_project"], "one");
    assert_eq!(
        s.json(&["project", "list", "--json"])["projects"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        before.replace("current_project = \"two\"", "current_project = \"one\""),
        s.state(),
        "only current_project changed"
    );
    // Custom domain: re-run with or without the same --domain is fine; a different one is not.
    s.run(&["project", "create", "shop", "--domain", "shop.acme.local"])
        .success();
    s.run(&["project", "create", "shop"])
        .success()
        .stdout(predicate::str::contains("shop.acme.local already exists"));
    s.run(&["project", "create", "shop", "--domain", "SHOP.acme.local"])
        .success();
    s.run(&["project", "create", "shop", "--domain", "shop.other.local"])
        .failure()
        .code(2)
        .stderr(predicate::str::contains(
            "already exists with domain shop.acme.local, not shop.other.local",
        ));
}

#[test]
fn project_name_rules() {
    let s = Sandbox::new();
    s.run(&["project", "create", "my.proj"])
        .failure()
        .code(2)
        .stderr(predicate::str::contains("can't contain dots"))
        .stderr(predicate::str::contains("hint:").and(predicate::str::contains("--domain")));
    s.run(&["project", "create", "-bad"]).failure();
    s.run(&["project", "create", "bad-"]).failure().code(2);
    s.run(&["project", "create", &"a".repeat(64)])
        .failure()
        .code(2);
    s.run(&["project", "create", &"a".repeat(63)]).success();
    s.run(&["project", "create", "under_score"])
        .failure()
        .code(2);
}

#[test]
fn domain_rules() {
    let s = Sandbox::new();
    for d in ["a.test", "a.internal"] {
        s.run(&["project", "create", "a", "--domain", d])
            .failure()
            .code(2)
            .stderr(predicate::str::contains(
                "only .lokal and .local are supported",
            ));
    }
    s.run(&["project", "create", "a", "--domain", "a.com"])
        .failure()
        .stderr(predicate::str::contains(
            "only .lokal and .local are supported (other TLDs could shadow real websites)",
        ));
    s.run(&["project", "create", "a", "--domain", "local"])
        .failure()
        .code(2);
    s.run(&["project", "create", "a", "--domain", "Shop.Acme.LOCAL"])
        .success();
    let v = s.json(&["project", "show", "a", "--json"]);
    assert_eq!(v["project"]["domain"], "shop.acme.local");
}

#[test]
fn apex_overlap_rules() {
    let s = Sandbox::new();
    s.run(&["project", "create", "app"]).success();
    s.run(&["project", "create", "api", "--domain", "api.app.lokal"])
        .failure()
        .code(2)
        .stderr(predicate::str::contains("overlaps project `app`"));
    s.run(&["project", "create", "x", "--domain", "app.lokal"])
        .failure()
        .code(2);
    s.run(&["project", "create", "myapp"]).success();
    // parent of an existing apex
    s.run(&["project", "create", "deep", "--domain", "a.b.lokal"])
        .success();
    s.run(&["project", "create", "b"])
        .failure()
        .stderr(predicate::str::contains("overlaps"));
}

#[test]
fn default_tld_is_lokal_and_local_still_works() {
    let s = Sandbox::new();
    s.run(&["project", "create", "shop"])
        .success()
        .stdout(predicate::str::contains("Created shop.lokal"));
    // Same name under .local is a different apex: no overlap.
    s.run(&["project", "create", "legacy", "--domain", "shop.local"])
        .success();
    s.run(&["service", "add", "api", "4000", "-p", "legacy"])
        .success()
        .stdout(predicate::str::contains("api.shop.local → localhost:4000"));
    s.run(&["service", "add", "api", "4000", "-p", "shop"])
        .success()
        .stdout(predicate::str::contains("api.shop.lokal → localhost:4000"));
    s.run(&["project", "create", "x", "--domain", "x.dev"])
        .failure()
        .stderr(predicate::str::contains(
            "only .lokal and .local are supported",
        ));
}

#[test]
fn subdomain_rules() {
    let s = Sandbox::new();
    s.run(&["project", "create", "p"]).success();
    s.run(&["service", "add", "api", "4000"])
        .success()
        .stdout(predicate::str::contains("api.p.lokal → localhost:4000"))
        .stdout(predicate::str::contains("https://api.p.lokal"));
    s.run(&["service", "add", "v1.api", "4001"]).success();
    s.run(&["service", "add", "WEB.p.lokal", "4002"]).success();
    s.run(&["service", "add", "@", "4003"]).success();
    s.run(&["service", "add", "a..b", "4004"]).failure().code(2);
    s.run(&["service", "add", "-x", "4004"]).failure().code(2);
    s.run(&[
        "service",
        "add",
        &format!(
            "{}.{}.{}.{}",
            "a".repeat(63),
            "b".repeat(63),
            "c".repeat(63),
            "d".repeat(60)
        ),
        "4005",
    ])
    .failure()
    .stderr(predicate::str::contains("longer than 253"));
    let v = s.json(&["service", "list", "--json"]);
    let subs: Vec<&str> = v["services"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["subdomain"].as_str().unwrap())
        .collect();
    assert_eq!(subs, ["@", "api", "v1.api", "web"]);
    let apex = &v["services"][0];
    assert_eq!(apex["fqdn"], "p.lokal");
    assert_eq!(apex["url"], "https://p.lokal");
}

#[test]
fn uniqueness_rule_message() {
    let s = Sandbox::new();
    s.run(&["project", "create", "p"]).success();
    s.run(&["service", "add", "api", "4000"]).success();
    s.run(&["service", "add", "api", "5000"])
        .failure()
        .code(2)
        .stderr(predicate::str::contains(
            "service `api` already exists (localhost:4000); use `lokal service update api --port 5000`",
        ));
    s.run(&["service", "add", "api", "4000", "--host-header", "upstream"])
        .failure()
        .stderr(predicate::str::contains(
            "use `lokal service update api --host-header upstream`",
        ));
    s.run(&["service", "add", "api", "4001", "--host", "127.0.0.1"])
        .failure()
        .stderr(predicate::str::contains(
            "use `lokal service update api --port 4001 --host 127.0.0.1`",
        ));
}

#[test]
fn service_add_is_safe_to_rerun() {
    let s = Sandbox::new();
    s.run(&["project", "create", "p"]).success();
    s.run(&["service", "add", "api", "4000"]).success();
    s.run(&["service", "add", "web", "5173", "--host-header", "upstream"])
        .success();
    s.run(&["service", "add", "@", "3000"]).success();
    let before = s.state();
    for args in [
        &["service", "add", "api", "4000"][..],
        &[
            "service",
            "add",
            "API.p.lokal",
            "4000",
            "--host",
            "localhost",
        ],
        &["service", "add", "web", "5173", "--host-header", "upstream"],
        &["service", "add", "@", "3000"],
    ] {
        s.run(args)
            .success()
            .stdout(predicate::str::contains("already configured"));
    }
    assert_eq!(
        before,
        s.state(),
        "re-adding identical services must not touch state"
    );
    // The existing upstream is described with its host-header mode.
    s.run(&["service", "add", "web", "5173"])
        .failure()
        .stderr(predicate::str::contains("(localhost:5173, host header upstream); use `lokal service update web --host-header preserve`"));
}

#[test]
fn port_rules() {
    let s = Sandbox::new();
    s.run(&["project", "create", "p"]).success();
    s.run(&["service", "add", "a", "0"])
        .failure()
        .code(2)
        .stderr(predicate::str::contains("out of range"));
    s.run(&["service", "add", "a", "65536"]).failure().code(2);
    s.run(&["service", "add", "a", "443"])
        .failure()
        .code(2)
        .stderr(predicate::str::contains("loop back"));
    s.run(&["service", "add", "a", "80", "--host", "127.0.0.1"])
        .failure()
        .code(2);
    s.run(&["service", "add", "a", "443", "--host", "10.0.0.5"])
        .success()
        .stdout(predicate::str::contains("not a loopback address"));
    s.run(&["service", "add", "b", "65535"]).success();
}

#[test]
fn host_rules() {
    let s = Sandbox::new();
    s.run(&["project", "create", "p"]).success();
    s.run(&["service", "add", "a", "3000", "--host", "http://localhost"])
        .failure()
        .code(2)
        .stderr(predicate::str::contains("scheme"))
        .stderr(predicate::str::contains("hint:"));
    s.run(&["service", "add", "a", "3000", "--host", "localhost:3000"])
        .failure()
        .stderr(predicate::str::contains("must not include a port"));
    s.run(&["service", "add", "a", "3000", "--host", "[::1]"])
        .success();
    let v = s.json(&["service", "list", "--json"]);
    assert_eq!(v["services"][0]["host"], "::1");
    s.run(&["service", "add", "b", "3000", "--host", "[::1]:3000"])
        .failure()
        .code(2);
    assert!(s.state().contains("host = \"::1\""));
}

#[test]
fn update_and_remove() {
    let s = Sandbox::new();
    s.run(&["project", "create", "p"]).success();
    s.run(&["service", "add", "api", "4000"]).success();
    s.run(&["service", "update", "api"])
        .failure()
        .code(2)
        .stderr(predicate::str::contains("nothing to change"));
    s.run(&[
        "service",
        "update",
        "api",
        "--port",
        "4001",
        "--host-header",
        "upstream",
    ])
    .success();
    let v = s.json(&["service", "list", "--json"]);
    assert_eq!(v["services"][0]["port"], 4001);
    assert_eq!(v["services"][0]["host_header"], "upstream");
    s.run(&["service", "set", "api", "--port", "443"])
        .failure()
        .code(2);
    s.run(&["service", "update", "nope", "--port", "1"])
        .failure();
    s.run(&["service", "rm", "api"])
        .success()
        .stdout(predicate::str::contains("Removed api.p.lokal"));
    s.run(&["service", "remove", "api"]).failure();
}

#[test]
fn current_project_resolution() {
    let s = Sandbox::new();
    s.run(&["service", "list"])
        .failure()
        .stderr(predicate::str::contains(
            "no project selected: run 'lokal project use <NAME>' or pass -p <NAME>",
        ));
    s.run(&["project", "create", "one"]).success();
    s.run(&["project", "create", "two"]).success();
    // create sets current
    assert_eq!(s.json(&["list", "--json"])["current_project"], "two");
    s.run(&["service", "add", "api", "4000"]).success();
    assert_eq!(s.json(&["service", "list", "--json"])["project"], "two");
    // -p wins
    s.run(&["service", "add", "api", "4000", "-p", "one"])
        .success();
    s.run(&["p", "use", "one"]).success();
    assert_eq!(s.json(&["s", "ls", "--json"])["project"], "one");
    // remove clears current; with two projects left none is selected
    s.run(&["project", "create", "three"]).success();
    s.run(&["project", "use", "one"]).success();
    s.run(&["project", "remove", "one", "--force"]).success();
    assert_eq!(s.json(&["list", "--json"])["current_project"], Value::Null);
    s.run(&["service", "list"])
        .failure()
        .stderr(predicate::str::contains("no project selected"));
    // exactly one project → that one
    s.run(&["project", "rm", "three"]).success();
    assert_eq!(s.json(&["service", "list", "--json"])["project"], "two");
}

#[test]
fn project_remove_needs_force_with_services() {
    let s = Sandbox::new();
    s.run(&["project", "create", "p"]).success();
    s.run(&["service", "add", "api", "4000"]).success();
    s.run(&["project", "remove", "p"])
        .failure()
        .stderr(predicate::str::contains("still has 1 service"))
        .stderr(predicate::str::contains("--force"));
    s.run(&["project", "remove", "p", "--force"]).success();
    assert_eq!(
        s.json(&["project", "list", "--json"])["projects"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
}

#[test]
fn json_shapes_match_spec() {
    let s = Sandbox::new();
    s.run(&["project", "create", "myproject"]).success();
    s.run(&["service", "add", "api", "4000"]).success();
    let v = s.json(&["list", "--json"]);
    let expected: Value = serde_json::from_str(
        r#"{ "schema": 1, "current_project": "myproject", "projects": [
  { "name": "myproject", "domain": "myproject.lokal", "services": [
    { "subdomain": "api", "fqdn": "api.myproject.lokal", "url": "https://api.myproject.lokal",
      "host": "localhost", "port": 4000, "host_header": "preserve",
      "managed": null } ] } ] }"#,
    )
    .unwrap();
    assert_eq!(v, expected);
    assert_eq!(s.json(&["project", "list", "--json"]), expected);
    let show = s.json(&["project", "show", "--json"]);
    assert_eq!(show["schema"], 1);
    assert_eq!(show["project"], expected["projects"][0]);
    let sl = s.json(&["service", "list", "--json"]);
    assert_eq!(sl["schema"], 1);
    assert_eq!(sl["services"], expected["projects"][0]["services"]);
    // pretty-printed, no colour codes
    let raw = s.cmd().args(["list", "--json"]).output().unwrap().stdout;
    let text = String::from_utf8(raw).unwrap();
    assert!(text.contains("\n  \"schema\": 1"));
    assert!(!text.contains('\x1b'));
}

/// A service `lokal up` added (`managed = "lokal.toml"`, SPEC §5.2) in the
/// other commands (§4.11): marked in `list`, `managed` in `--json`, an
/// identical `service add` keeps the marker, and `update` and `remove` say
/// that the next `up` undoes them. `tests/up.rs` covers the same through
/// `lokal up`.
#[test]
fn managed_services_in_other_commands() {
    let s = Sandbox::new();
    std::fs::create_dir_all(s.dir.path()).unwrap();
    std::fs::write(
        s.dir.path().join("state.toml"),
        "version = 1\ncurrent_project = \"p\"\n[projects.p]\ndomain = \"p.lokal\"\n\
[projects.p.services.web]\nhost = \"localhost\"\nport = 51234\nhost_header = \"preserve\"\nmanaged = \"lokal.toml\"\n\
[projects.p.services.api]\nhost = \"localhost\"\nport = 4000\nhost_header = \"preserve\"\n",
    )
    .unwrap();
    let v = s.json(&["list", "--json"]);
    let svcs = v["projects"][0]["services"].as_array().unwrap();
    let managed =
        |sub: &str| svcs.iter().find(|x| x["subdomain"] == sub).unwrap()["managed"].clone();
    assert_eq!(managed("web"), "lokal.toml");
    assert_eq!(managed("api"), Value::Null);
    let out = s.run(&["list"]).success().get_output().stdout.clone();
    let out = String::from_utf8(out).unwrap();
    let line = |sub: &str| {
        out.lines()
            .find(|l| l.trim_start().starts_with(&format!("{sub} ")))
            .unwrap_or_default()
            .to_string()
    };
    assert!(
        line("web").ends_with("→ localhost:51234  (lokal.toml)"),
        "{out}"
    );
    assert!(line("api").ends_with("→ localhost:4000"), "{out}");

    // The same route again: already configured, and still managed.
    s.run(&["service", "add", "web", "51234"])
        .success()
        .stdout(predicate::str::contains("(already configured"));
    assert!(
        s.state().contains("managed = \"lokal.toml\""),
        "{}",
        s.state()
    );
    // A different route still needs `update`.
    s.run(&["service", "add", "web", "5000"])
        .failure()
        .code(2)
        .stderr(predicate::str::contains(
            "use `lokal service update web --port 5000`",
        ));
    s.run(&["service", "update", "web", "--port", "5001"])
        .success()
        .stdout(predicate::str::contains(
            "note: lokal.toml manages web; the next lokal up sets it back",
        ));
    assert!(
        s.state().contains("managed = \"lokal.toml\""),
        "{}",
        s.state()
    );
    s.run(&["service", "update", "api", "--port", "4001"])
        .success()
        .stdout(predicate::str::contains("lokal.toml").not());
    s.run(&["service", "remove", "web"])
        .success()
        .stdout(predicate::str::contains(
            "note: lokal.toml manages web; the next lokal up adds it back",
        ));
    s.run(&["service", "remove", "api"])
        .success()
        .stdout(predicate::str::contains("lokal.toml").not());
}

#[test]
fn status_without_daemon() {
    let s = Sandbox::new();
    let v = s.json(&["status", "--json"]);
    assert_eq!(v["schema"], 1);
    assert_eq!(v["running"], false);
}

#[test]
fn unknown_state_fields_and_newer_version() {
    let s = Sandbox::new();
    std::fs::write(
        s.dir.path().join("state.toml"),
        "version = 1\nfuture = 1\n[projects.a]\ndomain = \"a.local\"\nextra = true\n",
    )
    .unwrap();
    s.run(&["list"])
        .success()
        .stdout(predicate::str::contains("a.local"));
    std::fs::write(s.dir.path().join("state.toml"), "version = 9\n").unwrap();
    s.run(&["list"]).failure().stderr(predicate::str::contains(
        "state file was written by a newer lokal; run a newer lokal",
    ));
}

#[test]
fn twenty_parallel_adds_lose_no_writes() {
    let s = Sandbox::new();
    s.run(&["project", "create", "p"]).success();
    let bin = assert_cmd::cargo::cargo_bin("lokal");
    let handles: Vec<_> = (0..20)
        .map(|i| {
            std::process::Command::new(&bin)
                .env("LOKAL_HOME", s.dir.path())
                .args(["service", "add", &format!("s{i}"), &format!("{}", 5000 + i)])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .unwrap()
        })
        .collect();
    for h in handles {
        let out = h.wait_with_output().unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let v = s.json(&["service", "list", "--json"]);
    assert_eq!(v["services"].as_array().unwrap().len(), 20);
}

#[test]
fn not_set_up_nudges() {
    let s = Sandbox::new();
    s.run(&["ca", "path"])
        .failure()
        .code(3)
        .stderr(predicate::str::contains("lokal setup"));
    s.run(&["env"]).failure().code(3);
}

/// A plain `lokal uninstall` keeps the CA files but removes install.json and
/// the LaunchAgent. `env` and `ca path` must then print nothing: the
/// `eval "$(lokal env)"` profile line would otherwise keep exporting the CA as
/// a trust anchor. Real layout (no LOKAL_HOME) under a temporary HOME.
#[test]
fn env_and_ca_path_need_setup_not_just_the_ca_files() {
    let home = tempfile::Builder::new().prefix("lkh").tempdir().unwrap();
    let real = |args: &[&str]| {
        let mut c = Command::cargo_bin("lokal").unwrap();
        c.env("HOME", home.path())
            .env_remove("LOKAL_HOME")
            .env("NO_COLOR", "1")
            .args(args);
        c.assert()
    };
    let out = real(&["internal", "paths"])
        .success()
        .get_output()
        .stdout
        .clone();
    let p: Value = serde_json::from_slice(&out).unwrap();
    let path = |k: &str| std::path::PathBuf::from(p[k].as_str().unwrap());
    assert!(path("base").starts_with(home.path()), "{p}");
    // The CA files where setup puts them, and a fresh bundle so `env` doesn't
    // rebuild it from the system roots.
    Command::cargo_bin("lokal")
        .unwrap()
        .env("LOKAL_HOME", path("base"))
        .args(["internal", "create-ca"])
        .assert()
        .success();
    std::fs::write(path("ca_bundle"), "").unwrap();

    // Uninstalled: CA files kept, install.json and the plist gone.
    for args in [&["env"][..], &["env", "--shell", "fish"], &["ca", "path"]] {
        real(args)
            .failure()
            .code(3)
            .stdout("")
            .stderr(predicate::str::contains("lokal setup"));
    }

    // Set up: the same CA files are exported.
    std::fs::write(path("install_record"), "{}").unwrap();
    std::fs::create_dir_all(path("plist").parent().unwrap()).unwrap();
    std::fs::write(path("plist"), "").unwrap();
    real(&["env"]).success().stdout(
        predicate::str::contains("export NODE_EXTRA_CA_CERTS=")
            .and(predicate::str::contains("bundle.pem")),
    );
    real(&["ca", "path"])
        .success()
        .stdout(predicate::str::contains("root.pem"));
}

#[test]
fn setup_refuses_sandbox() {
    let s = Sandbox::new();
    s.run(&["setup", "--dry-run"])
        .failure()
        .stderr(predicate::str::contains("LOKAL_HOME"));
}

#[test]
fn closed_stdout_pipe_does_not_panic() {
    // `lokal --help | head -1`: the reader goes away before lokal writes.
    let s = Sandbox::new();
    for args in [&["--help"][..], &["list"], &["status"]] {
        let mut child = std::process::Command::new(assert_cmd::cargo::cargo_bin("lokal"))
            .env("LOKAL_HOME", s.dir.path())
            .args(args)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        drop(child.stdout.take());
        let out = child.wait_with_output().unwrap();
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(!err.contains("panicked"), "{args:?}: {err}");
    }
}

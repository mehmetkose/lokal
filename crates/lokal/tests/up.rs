//! `lokal.toml`, `lokal up` and `lokal down` (SPEC §4.11, M5 steps 1 and
//! 2): the file's errors and warnings, finding the file, the derived project
//! name, idempotence, managed versus hand-added services, clashes, domains,
//! busy ports, the other commands on managed services, and the trust check
//! (§16.11): no terminal, `--yes`, a person's answer on a pty, a changed
//! file, and symlinks. Each test makes a temp repo and runs lokal in it
//! with a `LOKAL_HOME` sandbox; the ones that need live routes use the
//! sandbox daemon.
//!
//! The tests that aren't about the trust check pass `--yes`, the agent and
//! CI path, unless they fail before it (an error in the file, an unknown
//! SUB).

mod common;

use std::io::Write;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output};
use std::time::{Duration, Instant};

use common::{Daemon, PtyOutput, spawn_on_pty};
use serde_json::Value;
use testkit::probe;

/// `lokal args…` in `cwd` under the `home` sandbox. Run with `.output()`,
/// its stdin is closed: no terminal.
fn lokal_cmd(home: &Path, cwd: &Path, args: &[&str]) -> Command {
    let mut c = Command::new(assert_cmd::cargo::cargo_bin("lokal"));
    c.env("LOKAL_HOME", home)
        .env("NO_COLOR", "1")
        .env_remove("LOKAL_FORBID_SUDO")
        .current_dir(cwd)
        .args(args);
    c
}

fn lokal(home: &Path, cwd: &Path, args: &[&str]) -> Output {
    lokal_cmd(home, cwd, args).output().unwrap()
}

fn code(o: &Output) -> i32 {
    o.status.code().unwrap_or(-1)
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

/// A `LOKAL_HOME` sandbox and a folder for repos.
struct Sandbox {
    home: tempfile::TempDir,
    repos: tempfile::TempDir,
}

impl Sandbox {
    fn new() -> Self {
        Self {
            // Short: the IPC socket path must fit in sun_path.
            home: tempfile::Builder::new().prefix("lk").tempdir().unwrap(),
            repos: tempfile::Builder::new().prefix("lkr").tempdir().unwrap(),
        }
    }

    /// A repo directory `name` (nested paths allowed) with this `lokal.toml`.
    /// Canonical, as the current directory lokal sees (`/private/var/…` on
    /// macOS, not `/var/…`).
    fn repo(&self, name: &str, toml: &str) -> PathBuf {
        let dir = self.repos.path().join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("lokal.toml"), toml).unwrap();
        dir.canonicalize().unwrap()
    }

    fn run(&self, cwd: &Path, args: &[&str]) -> Output {
        lokal(self.home.path(), cwd, args)
    }

    /// Runs lokal in `cwd` and expects exit 0.
    fn ok(&self, cwd: &Path, args: &[&str]) -> Output {
        let o = self.run(cwd, args);
        assert_eq!(code(&o), 0, "lokal {args:?}:\n{}{}", stdout(&o), stderr(&o));
        o
    }

    fn state(&self) -> Option<Vec<u8>> {
        std::fs::read(self.home.path().join("state.toml")).ok()
    }

    fn list(&self) -> Value {
        list(self.home.path())
    }

    fn trusted_dir(&self) -> PathBuf {
        self.home.path().join("trusted")
    }

    /// The approval records in `<LOKAL_HOME>/trusted/` (none if it doesn't
    /// exist).
    fn records(&self) -> Vec<PathBuf> {
        let Ok(rd) = std::fs::read_dir(self.trusted_dir()) else {
            return vec![];
        };
        let mut v: Vec<PathBuf> = rd.map(|e| e.unwrap().path()).collect();
        v.sort();
        v
    }

    /// Where SPEC §16.11 puts the approval of the file at the canonical
    /// path `file`: `trusted/<SHA-256 of the path>`.
    fn record_of(&self, file: &Path) -> PathBuf {
        self.trusted_dir()
            .join(sha256_hex(file.as_os_str().as_bytes()))
    }

    /// Runs `lokal up` in `cwd` on a pty, waits for the question, types
    /// `answer`, and returns the exit code and everything lokal printed up
    /// to `last`, a line it prints before it exits.
    fn up_on_pty(&self, cwd: &Path, answer: &str, last: &str) -> (i32, String) {
        let (child, master) = spawn_on_pty(lokal_cmd(self.home.path(), cwd, &["up"]));
        let mut run = OnPty(child);
        let out = PtyOutput::collect(&master);
        out.wait_for("run these commands? [y/N] ", Duration::from_secs(10));
        std::fs::File::from(master.try_clone().unwrap())
            .write_all(answer.as_bytes())
            .unwrap();
        let code = run.wait_exit(Duration::from_secs(10), &out);
        out.wait_for(last, Duration::from_secs(5));
        (code, out.text())
    }
}

/// A lokal on a pty; stopped if the test fails while it waits for an answer.
struct OnPty(Child);

impl OnPty {
    fn wait_exit(&mut self, limit: Duration, out: &PtyOutput) -> i32 {
        let deadline = Instant::now() + limit;
        loop {
            if let Some(st) = self.0.try_wait().unwrap() {
                return st.code().unwrap_or(-1);
            }
            assert!(
                Instant::now() < deadline,
                "lokal didn't exit within {limit:?}; output so far:\n{}",
                out.text()
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for OnPty {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

fn sha256_hex(data: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, data)
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn mode(path: &Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

fn list(home: &Path) -> Value {
    let o = lokal(home, home, &["list", "--json"]);
    assert_eq!(code(&o), 0, "{}", stderr(&o));
    serde_json::from_slice(&o.stdout).unwrap()
}

/// `subdomain → (port, managed)` for every service in `project`.
fn services(v: &Value, project: &str) -> Vec<(String, u64, Value)> {
    v["projects"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|p| p["name"] == project)
        .flat_map(|p| p["services"].as_array().unwrap().clone())
        .map(|s| {
            (
                s["subdomain"].as_str().unwrap().to_string(),
                s["port"].as_u64().unwrap(),
                s["managed"].clone(),
            )
        })
        .collect()
}

fn project_names(v: &Value) -> Vec<String> {
    v["projects"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap().to_string())
        .collect()
}

fn managed() -> Value {
    Value::from("lokal.toml")
}

/// A port nothing listens on.
fn free_port() -> u16 {
    common::free_port()
}

#[test]
fn file_errors_exit_2_with_their_location_and_change_nothing() {
    let s = Sandbox::new();
    let somewhere = s.repos.path();
    s.ok(somewhere, &["project", "create", "other"]);
    let before = s.state();
    let cases: &[(&str, &str, &str)] = &[
        (
            "api = { port = 4000 }\n",
            "3:1",
            "service `api` has no `run`",
        ),
        (
            "api = 4000\n",
            "3:7",
            "`api = 4000` has no command: every service needs `run`, e.g. api = { run = \"npm start\", port = 4000 }",
        ),
        (
            "\"bad_name\" = \"x\"\n",
            "3:1",
            "invalid subdomain label `bad_name`",
        ),
        (
            "api = { run = \"x\", env = { PORT = \"1\" } }\n",
            "3:28",
            "env `PORT` is set by lokal",
        ),
        (
            "api = { run = \"x\", cwd = \"nope\" }\n",
            "3:26",
            "cwd `nope` is not a directory",
        ),
        (
            "api = \"a\"\nAPI = \"b\"\n",
            "4:1",
            "`API` and `api` (line 3) are the same service, `api`",
        ),
        (
            "a = { run = \"x\", port = 3000 }\nb = { run = \"y\", port = 3000 }\n",
            "4:25",
            "port 3000 is already the port of `a` (line 3)",
        ),
    ];
    for (body, at, msg) in cases {
        let dir = s.repo("p", &format!("project = \"p\"\n[services]\n{body}"));
        let o = s.run(&dir, &["up"]);
        let err = stderr(&o);
        assert_eq!(code(&o), 2, "{body}: {err}");
        assert!(
            err.contains(&format!("lokal.toml:{at}: {msg}")),
            "{body}: want {at}: {msg}\n{err}"
        );
        assert_eq!(s.state(), before, "{body}");
    }
    let dir = s.repo("p", "project = \"p\"\n");
    let o = s.run(&dir, &["up"]);
    assert_eq!(code(&o), 2);
    assert!(
        stderr(&o).contains("lokal.toml:1:1: lokal.toml has no services"),
        "{}",
        stderr(&o)
    );
    // A TOML syntax error has the parser's location.
    let dir = s.repo("p", "project = \"p\"\n[services]\nweb = { run = \"x\"\n");
    let o = s.run(&dir, &["up"]);
    assert_eq!(code(&o), 2);
    assert!(stderr(&o).contains("lokal.toml:3:"), "{}", stderr(&o));
    assert_eq!(s.state(), before);
}

#[test]
fn the_short_form_works_and_unknown_keys_only_warn() {
    let s = Sandbox::new();
    let dir = s.repo(
        "shop",
        "project = \"p\"\nflavour = 1\n[services]\nweb = \"npm run dev -- --port {port}\"\napi = { run = \"cargo run\", restart = true }\n",
    );
    let o = s.ok(&dir, &["up", "--yes"]);
    let (out, err) = (stdout(&o), stderr(&o));
    let file = dir.join("lokal.toml");
    assert!(
        err.contains(&format!(
            "warning: {}:2:1: unknown key `flavour` (ignored)",
            file.display()
        )),
        "{err}"
    );
    assert!(
        err.contains(&format!(
            "warning: {}:5:28: unknown key `restart` (ignored)",
            file.display()
        )),
        "{err}"
    );
    let svcs = services(&s.list(), "p");
    let web = svcs.iter().find(|x| x.0 == "web").unwrap();
    assert_eq!(web.2, managed());
    // One line per service, with {port} replaced.
    assert!(
        out.contains(&format!(
            "✓ https://web.p.lokal → localhost:{0}  npm run dev -- --port {0}\n",
            web.1
        )),
        "{out}"
    );
    assert!(out.contains("✓ https://api.p.lokal → localhost:"), "{out}");
    assert!(out.contains("  routes saved\n"), "{out}");
    assert!(out.contains("background service isn't running"), "{out}");
    // `up` makes the project current.
    assert_eq!(s.list()["current_project"], "p");
}

#[test]
fn the_file_is_found_from_a_subdirectory_and_a_nested_one_is_its_own_project() {
    let s = Sandbox::new();
    let root = s.repo("mono", "project = \"outer\"\n[services]\nweb = \"x\"\n");
    let deep = root.join("services/api/src");
    std::fs::create_dir_all(&deep).unwrap();
    s.ok(&deep, &["up", "--yes"]);
    assert_eq!(project_names(&s.list()), ["outer"]);
    // A nested lokal.toml without `project`: named after its directory.
    let inner = s.repo("mono/Inner App", "[services]\napi = \"y\"\n");
    let below = inner.join("x");
    std::fs::create_dir_all(&below).unwrap();
    let o = s.ok(&below, &["up", "--yes"]);
    assert!(
        stdout(&o).contains(
            "note: project inner-app (from the directory name; set project = \"…\" in lokal.toml to choose another)"
        ),
        "{}",
        stdout(&o)
    );
    assert!(
        stdout(&o).contains("✓ https://api.inner-app.lokal → "),
        "{}",
        stdout(&o)
    );
    let v = s.list();
    assert_eq!(project_names(&v), ["inner-app", "outer"]);
    assert_eq!(services(&v, "outer").len(), 1, "nothing merges");
    // No file at all: exit 1.
    let empty = s.repos.path().join("empty");
    std::fs::create_dir_all(&empty).unwrap();
    let o = s.run(&empty, &["up"]);
    assert_eq!(code(&o), 1);
    assert!(
        stderr(&o).contains("error: no lokal.toml in "),
        "{}",
        stderr(&o)
    );
    assert!(
        stderr(&o).contains(" or any parent directory"),
        "{}",
        stderr(&o)
    );
}

#[test]
fn a_second_up_changes_nothing_and_keeps_picked_ports() {
    let s = Sandbox::new();
    let dir = s.repo(
        "shop",
        "project = \"shop\"\n[services]\n\"@\" = { run = \"a\", port = 3000 }\napi = \"b\"\nweb = { run = \"c\", host_header = \"upstream\" }\n",
    );
    let first = s.ok(&dir, &["up", "--yes"]);
    let state = s.state().unwrap();
    let ports = services(&s.list(), "shop");
    let second = s.ok(&dir, &["up", "--yes"]);
    assert_eq!(s.state().unwrap(), state, "byte-identical");
    assert_eq!(services(&s.list(), "shop"), ports);
    assert_eq!(stdout(&first), stdout(&second));
    // A picked port stays even after other services change.
    std::fs::write(
        dir.join("lokal.toml"),
        "project = \"shop\"\n[services]\napi = \"b\"\nweb = \"c\"\ndocs = \"d\"\n",
    )
    .unwrap();
    s.ok(&dir, &["up", "--yes"]);
    let after = services(&s.list(), "shop");
    let port = |v: &[(String, u64, Value)], sub: &str| v.iter().find(|x| x.0 == sub).map(|x| x.1);
    assert_eq!(port(&after, "api"), port(&ports, "api"));
    assert_eq!(port(&after, "web"), port(&ports, "web"));
    assert_eq!(port(&after, "@"), None, "left the file");
    assert!(port(&after, "docs").is_some());
}

#[test]
fn hand_added_services_survive_up_and_down() {
    let s = Sandbox::new();
    let dir = s.repo(
        "shop",
        "project = \"shop\"\n[services]\nweb = \"a\"\napi = \"b\"\n",
    );
    s.ok(&dir, &["project", "create", "shop"]);
    s.ok(&dir, &["service", "add", "db", "5432"]);
    s.ok(&dir, &["up", "--yes"]);
    let svcs = services(&s.list(), "shop");
    let by: Vec<(&str, &Value)> = svcs.iter().map(|x| (x.0.as_str(), &x.2)).collect();
    assert_eq!(
        by,
        [
            ("api", &managed()),
            ("db", &Value::Null),
            ("web", &managed())
        ]
    );
    // A service removed from the file is gone after the next up.
    std::fs::write(
        dir.join("lokal.toml"),
        "project = \"shop\"\n[services]\nweb = \"a\"\n",
    )
    .unwrap();
    s.ok(&dir, &["up", "--yes"]);
    let subs: Vec<String> = services(&s.list(), "shop")
        .into_iter()
        .map(|x| x.0)
        .collect();
    assert_eq!(subs, ["db", "web"]);
    let o = s.ok(&dir, &["down"]);
    assert!(
        stdout(&o).contains("✓ Removed 1 service from lokal.toml  (saved)"),
        "{}",
        stdout(&o)
    );
    assert_eq!(
        services(&s.list(), "shop"),
        [("db".to_string(), 5432, Value::Null)]
    );
}

#[test]
fn a_clash_fails_before_any_change() {
    let s = Sandbox::new();
    let dir = s.repo(
        "shop",
        "project = \"shop\"\n[services]\nweb = \"a\"\napi = \"b\"\n",
    );
    s.ok(&dir, &["project", "create", "shop"]);
    s.ok(&dir, &["service", "add", "web", "3000"]);
    let before = s.state();
    let o = s.run(&dir, &["up", "--yes"]);
    let err = stderr(&o);
    assert_eq!(code(&o), 2, "{err}");
    assert!(
        err.contains(
            "error: service web already exists and lokal.toml doesn't manage it (added by hand, or left by lokal run)"
        ),
        "{err}"
    );
    assert!(
        err.contains("hint: lokal service remove web -p shop"),
        "{err}"
    );
    assert_eq!(s.state(), before);
    // A route that `lokal run` left is a clash too.
    s.ok(&dir, &["service", "remove", "web"]);
    s.ok(&dir, &["run", "api", "--", "/usr/bin/true"]);
    std::fs::write(
        dir.join("lokal.toml"),
        "project = \"shop\"\n[services]\napi = \"b\"\n",
    )
    .unwrap();
    let before = s.state();
    let o = s.run(&dir, &["up", "--yes"]);
    assert_eq!(code(&o), 2);
    assert!(
        stderr(&o).contains("service api already exists"),
        "{}",
        stderr(&o)
    );
    assert!(stderr(&o).contains("left by lokal run"), "{}", stderr(&o));
    assert_eq!(s.state(), before);
}

#[test]
fn a_different_domain_fails_and_no_domain_key_takes_the_projects() {
    let s = Sandbox::new();
    let dir = s.repo(
        "shop",
        "project = \"shop\"\ndomain = \"shop.lokal\"\n[services]\nweb = \"a\"\n",
    );
    s.ok(
        &dir,
        &["project", "create", "shop", "--domain", "shop.local"],
    );
    let before = s.state();
    let o = s.run(&dir, &["up", "--yes"]);
    assert_eq!(code(&o), 2);
    assert!(
        stderr(&o).contains("project `shop` already exists with domain shop.local, not shop.lokal"),
        "{}",
        stderr(&o)
    );
    assert_eq!(s.state(), before);
    std::fs::write(
        dir.join("lokal.toml"),
        "project = \"shop\"\n[services]\nweb = \"a\"\n",
    )
    .unwrap();
    let o = s.ok(&dir, &["up", "--yes"]);
    assert!(
        stdout(&o).contains("✓ https://web.shop.local → localhost:"),
        "{}",
        stdout(&o)
    );
    let v = s.list();
    assert_eq!(v["projects"][0]["domain"], "shop.local");
    assert_eq!(v["projects"][0]["services"][0]["fqdn"], "web.shop.local");
}

#[test]
fn a_busy_port_stops_only_the_services_up_starts() {
    let s = Sandbox::new();
    let held = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let p = held.local_addr().unwrap().port();
    let dir = s.repo(
        "shop",
        &format!("project = \"shop\"\n[services]\na = {{ run = \"x\", port = {p} }}\nb = \"y\"\n"),
    );
    // A fixed port of a service this up starts: exit 1, nothing written.
    let o = s.run(&dir, &["up", "--yes"]);
    let err = stderr(&o);
    assert_eq!(code(&o), 1, "{err}");
    assert!(
        err.contains(&format!("error: localhost:{p} is already in use by")),
        "{err}"
    );
    assert!(
        err.contains(&format!("(pid {}); not starting a", std::process::id())),
        "{err}"
    );
    assert!(
        err.contains("hint: stop that server, or change port for a in lokal.toml"),
        "{err}"
    );
    assert_eq!(s.state(), None);
    // Starting only b: a is only routed, so its busy port is fine.
    let o = s.ok(&dir, &["up", "--yes", "b"]);
    assert!(
        stdout(&o).contains(&format!(
            "✓ https://a.shop.lokal → localhost:{p}  x  (not started)"
        )),
        "{}",
        stdout(&o)
    );
    let svcs = services(&s.list(), "shop");
    assert_eq!(svcs[0], ("a".to_string(), p as u64, managed()));
    let kept = svcs[1].1 as u16;
    // A busy kept port fails the same way, and isn't moved.
    let held2 = std::net::TcpListener::bind(("127.0.0.1", kept)).unwrap();
    let before = s.state();
    let o = s.run(&dir, &["up", "--yes", "b"]);
    let err = stderr(&o);
    assert_eq!(code(&o), 1, "{err}");
    assert!(
        err.contains(&format!("localhost:{kept} is already in use by")),
        "{err}"
    );
    assert!(
        err.contains(
            "hint: stop that server (is lokal run b still running?), or let lokal pick a new port: lokal service remove b -p shop, then lokal up"
        ),
        "{err}"
    );
    assert_eq!(s.state(), before);
    drop((held, held2));
}

#[test]
fn an_unknown_sub_is_a_usage_error_listing_the_services() {
    let s = Sandbox::new();
    let dir = s.repo(
        "shop",
        "project = \"shop\"\n[services]\n\"@\" = \"a\"\napi = \"b\"\n",
    );
    let o = s.run(&dir, &["up", "docs"]);
    assert_eq!(code(&o), 2);
    assert!(
        stderr(&o).contains("error: no service `docs` in "),
        "{}",
        stderr(&o)
    );
    assert!(
        stderr(&o).contains("hint: its services: @, api"),
        "{}",
        stderr(&o)
    );
    assert_eq!(s.state(), None);
    // The full name works too.
    let o = s.ok(&dir, &["up", "--yes", "api.shop.lokal"]);
    assert!(
        stdout(&o).contains("✓ https://shop.lokal → localhost:"),
        "{}",
        stdout(&o)
    );
    assert!(
        stdout(&o).contains("  a  (not started)\n"),
        "{}",
        stdout(&o)
    );
    let api = stdout(&o)
        .lines()
        .find(|l| l.starts_with("✓ https://api.shop.lokal → localhost:"))
        .map(str::to_string);
    assert!(
        api.as_deref().is_some_and(|l| l.ends_with("  b")),
        "{}",
        stdout(&o)
    );
}

#[test]
fn colliding_url_variables_warn_once() {
    let s = Sandbox::new();
    let dir = s.repo(
        "shop",
        "project = \"shop\"\n[services]\n\"v1.api\" = \"a\"\nv1-api = \"b\"\nweb = \"c\"\n",
    );
    let o = s.ok(&dir, &["up", "--yes"]);
    let err = stderr(&o);
    assert_eq!(
        err.matches("warning: LOKAL_URL_V1_API isn't set").count(),
        1,
        "{err}"
    );
}

#[test]
fn other_commands_on_managed_services() {
    let s = Sandbox::new();
    let dir = s.repo("shop", "project = \"shop\"\n[services]\nweb = \"a\"\n");
    s.ok(&dir, &["up", "--yes"]);
    s.ok(&dir, &["service", "add", "db", "5432"]);
    let port = services(&s.list(), "shop")[1].1.to_string();
    // list: the marker in --json and in the human output.
    let v = s.list();
    assert_eq!(
        services(&v, "shop"),
        [
            ("db".to_string(), 5432, Value::Null),
            ("web".to_string(), port.parse().unwrap(), managed())
        ]
    );
    let out = stdout(&s.ok(&dir, &["list"]));
    assert!(
        out.contains(&format!("→ localhost:{port}  (lokal.toml)\n")),
        "{out}"
    );
    assert!(out.contains("→ localhost:5432\n"), "{out}");
    let shown = stdout(&s.ok(&dir, &["project", "show"]));
    assert!(shown.contains("(lokal.toml)"), "{shown}");
    let json = stdout(&s.ok(&dir, &["service", "list", "--json"]));
    assert!(json.contains("\"managed\": \"lokal.toml\""), "{json}");
    assert!(json.contains("\"managed\": null"), "{json}");
    // The same route by hand: already configured, and still managed.
    let o = s.ok(&dir, &["service", "add", "web", &port]);
    assert!(stdout(&o).contains("(already configured"), "{}", stdout(&o));
    assert_eq!(services(&s.list(), "shop")[1].2, managed());
    let o = s.ok(&dir, &["service", "update", "web", "--port", "4999"]);
    assert!(
        stdout(&o).contains("note: lokal.toml manages web; the next lokal up sets it back"),
        "{}",
        stdout(&o)
    );
    // ... and it does.
    s.ok(&dir, &["up", "--yes"]);
    assert_eq!(services(&s.list(), "shop")[1].1.to_string(), "4999");
    let o = s.ok(&dir, &["service", "remove", "web"]);
    assert!(
        stdout(&o).contains("note: lokal.toml manages web; the next lokal up adds it back"),
        "{}",
        stdout(&o)
    );
    s.ok(&dir, &["up", "--yes"]);
    assert_eq!(services(&s.list(), "shop")[1].2, managed());
}

#[test]
fn down_removes_managed_services_and_maybe_the_project() {
    let s = Sandbox::new();
    let dir = s.repo(
        "shop",
        "project = \"shop\"\n[services]\nweb = \"a\"\napi = \"b\"\n",
    );
    s.ok(&dir, &["up", "--yes"]);
    s.ok(&dir, &["service", "add", "db", "5432"]);
    // --remove-project keeps a project that still has hand-added services.
    let o = s.ok(&dir, &["down", "--remove-project"]);
    let out = stdout(&o);
    assert!(
        out.contains("✓ Removed 2 services from lokal.toml  (saved)"),
        "{out}"
    );
    assert!(
        out.contains(
            "note: kept project shop: it still has services that lokal.toml doesn't manage (db)"
        ),
        "{out}"
    );
    assert_eq!(
        services(&s.list(), "shop"),
        [("db".to_string(), 5432, Value::Null)]
    );
    // Nothing left to do: exit 0.
    let o = s.ok(&dir, &["down"]);
    assert!(
        stdout(&o).contains("Nothing to remove: lokal.toml manages no services in shop"),
        "{}",
        stdout(&o)
    );
    // Without hand-added services, the project goes too, and stops being current.
    s.ok(&dir, &["service", "remove", "db"]);
    s.ok(&dir, &["up", "--yes"]);
    let o = s.ok(&dir, &["down", "--remove-project"]);
    assert!(
        stdout(&o).contains("✓ Removed 2 services from lokal.toml and the project shop.lokal"),
        "{}",
        stdout(&o)
    );
    let v = s.list();
    assert!(project_names(&v).is_empty());
    assert_eq!(v["current_project"], Value::Null);
    let o = s.ok(&dir, &["down"]);
    assert!(
        stdout(&o).contains("Nothing to remove: there is no project shop"),
        "{}",
        stdout(&o)
    );
}

#[test]
fn down_finds_the_project_without_a_valid_file_or_with_p() {
    let s = Sandbox::new();
    let dir = s.repo("Shop", "[services]\nweb = \"a\"\n");
    s.ok(&dir, &["up", "--yes"]);
    // A file with other errors still names its project (here derived).
    std::fs::write(dir.join("lokal.toml"), "[services]\nweb = 4000\n").unwrap();
    assert_eq!(code(&s.run(&dir, &["up"])), 2);
    let o = s.ok(&dir, &["down"]);
    assert!(
        stdout(&o).contains("note: project shop (from the directory name"),
        "{}",
        stdout(&o)
    );
    assert!(
        stdout(&o).contains("✓ Removed 1 service from lokal.toml"),
        "{}",
        stdout(&o)
    );
    // -p works anywhere; with neither -p nor a file, exit 1 and a hint.
    std::fs::write(dir.join("lokal.toml"), "[services]\nweb = \"a\"\n").unwrap();
    s.ok(&dir, &["up", "--yes"]);
    let elsewhere = s.repos.path().join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    let o = s.run(&elsewhere, &["down"]);
    assert_eq!(code(&o), 1);
    assert!(
        stderr(&o).contains("error: no lokal.toml in "),
        "{}",
        stderr(&o)
    );
    assert!(
        stderr(&o).contains("hint: run it in the repo, or name the project: lokal down -p <NAME>"),
        "{}",
        stderr(&o)
    );
    let o = s.ok(&elsewhere, &["down", "-p", "shop"]);
    assert!(
        stdout(&o).contains("✓ Removed 1 service from lokal.toml"),
        "{}",
        stdout(&o)
    );
    let o = s.ok(&elsewhere, &["down", "-p", "nope"]);
    assert!(
        stdout(&o).contains("Nothing to remove: there is no project nope"),
        "{}",
        stdout(&o)
    );
}

fn https_get(d: &Daemon, url: &str) -> anyhow::Result<probe::Resp> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(probe::get(url, &d.opts()))
}

/// With the sandbox daemon: every URL answers as soon as `up` returns (the
/// 502 page: the route is installed, nothing listens), and is gone as soon
/// as `down` returns (unknown SNI, SPEC §15 M3).
#[test]
fn routes_are_live_when_up_returns_and_gone_when_down_returns() {
    let d = Daemon::start("none");
    let repos = tempfile::Builder::new().prefix("lkr").tempdir().unwrap();
    let dir = repos.path().join("shop");
    std::fs::create_dir_all(&dir).unwrap();
    let (a, b) = (free_port(), free_port());
    std::fs::write(
        dir.join("lokal.toml"),
        format!("project = \"p\"\n[services]\n\"@\" = {{ run = \"x\", port = {a} }}\napi = {{ run = \"y\", port = {b} }}\n"),
    )
    .unwrap();
    let o = lokal(d.dir.path(), &dir, &["up", "--yes"]);
    assert_eq!(code(&o), 0, "{}", stderr(&o));
    assert!(stdout(&o).contains("  routes ready in "), "{}", stdout(&o));
    // No §4.5 note: the only note is --yes's.
    let out = stdout(&o);
    let notes: Vec<&str> = out
        .lines()
        .filter(|l| l.contains("note:"))
        .map(str::trim)
        .collect();
    assert_eq!(
        notes,
        ["note: running these without asking (--yes); nothing is remembered"],
        "{out}"
    );
    for (url, port) in [("https://p.lokal/", a), ("https://api.p.lokal/", b)] {
        let r = https_get(&d, url).unwrap();
        assert_eq!(r.status, 502, "{url}");
        assert!(
            r.text().contains(&format!("localhost:{port}")),
            "{}",
            r.text()
        );
        assert!(r.text().contains("is not responding"), "{}", r.text());
    }
    let o = lokal(d.dir.path(), &dir, &["down"]);
    assert_eq!(code(&o), 0, "{}", stderr(&o));
    assert!(
        stdout(&o).contains("✓ Removed 2 services from lokal.toml  (done in "),
        "{}",
        stdout(&o)
    );
    for url in ["https://p.lokal/", "https://api.p.lokal/"] {
        let e = https_get(&d, url).unwrap_err();
        assert!(format!("{e:#}").contains("TLS handshake"), "{url}: {e:#}");
    }
}

/// `lokal run` on a managed service reuses it as it is, marker included,
/// and `--rm` keeps it, since the run didn't add it.
#[test]
fn lokal_run_on_a_managed_service_keeps_it_managed() {
    let d = Daemon::start("none");
    let repos = tempfile::Builder::new().prefix("lkr").tempdir().unwrap();
    let dir = repos.path().join("shop");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("lokal.toml"),
        "project = \"p\"\n[services]\nweb = \"x\"\n",
    )
    .unwrap();
    assert_eq!(code(&lokal(d.dir.path(), &dir, &["up", "--yes"])), 0);
    let before = services(&list(d.dir.path()), "p");
    let o = lokal(
        d.dir.path(),
        &dir,
        &["run", "web", "--rm", "--", "/usr/bin/true"],
    );
    assert_eq!(code(&o), 0, "{}", stderr(&o));
    assert!(
        stderr(&o).contains("kept web.p.lokal: it existed before this run"),
        "{}",
        stderr(&o)
    );
    assert_eq!(services(&list(d.dir.path()), "p"), before);
    assert_eq!(before[0].2, managed());
    let state = std::fs::read_to_string(d.dir.path().join("state.toml")).unwrap();
    assert!(state.contains("managed = \"lokal.toml\""), "{state}");
}

// ───────────────────────── the trust check (SPEC §16.11) ─────────────────────────

/// Every kind of line the listing has: a plain `run`, a `cwd`, `{port}` as
/// written, and an `env` pair. No fixed port, which could be busy here.
const TRUST_TOML: &str = "project = \"shop\"\n[services]\n\"@\" = \"npm start\"\napi = { run = \"cargo run -p api\", cwd = \"services/api\" }\nweb = { run = \"vite --port {port}\", env = { NODE_ENV = \"development\" } }\n";

fn trust_repo(s: &Sandbox, name: &str) -> PathBuf {
    let dir = s.repo(name, TRUST_TOML);
    std::fs::create_dir_all(dir.join("services/api")).unwrap();
    dir
}

/// The listing of TRUST_TOML at `file`.
fn trust_listing(file: &Path) -> String {
    format!(
        "{} runs these commands:\n  @    npm start\n  api  {:<18}  in services/api\n  web  vite --port {{port}}\n       env NODE_ENV=development\n",
        file.display(),
        "cargo run -p api"
    )
}

const REFUSAL_HINT: &str =
    "hint: a person should review the file, then run 'lokal up' in a terminal and answer y\n";

/// Neither stream may suggest skipping the question: an agent reading it
/// would just add the flag.
fn assert_no_yes(o: &Output) {
    for text in [stdout(o), stderr(o)] {
        assert!(!text.contains("--yes"), "{text}");
        assert!(!text.contains(" -y"), "{text}");
    }
}

fn subdomains(s: &Sandbox, project: &str) -> Vec<String> {
    services(&s.list(), project)
        .into_iter()
        .map(|x| x.0)
        .collect()
}

#[test]
fn without_a_terminal_up_lists_the_commands_and_changes_nothing() {
    let s = Sandbox::new();
    s.ok(s.repos.path(), &["project", "create", "other"]);
    let before = s.state();
    let dir = trust_repo(&s, "shop");
    let file = dir.join("lokal.toml");
    for args in [&["up"][..], &["up", "web"]] {
        let o = s.run(&dir, args);
        let (out, err) = (stdout(&o), stderr(&o));
        assert_eq!(code(&o), 1, "{args:?}: {out}{err}");
        // Every service, also with a SUB: the approval is of the whole file.
        assert_eq!(out, trust_listing(&file), "{args:?}");
        assert_eq!(
            err,
            format!(
                "error: lokal.toml has commands nobody has approved yet: {}\n{REFUSAL_HINT}",
                file.display()
            )
        );
        assert_no_yes(&o);
        assert_eq!(s.state(), before, "nothing routed");
        assert!(s.records().is_empty());
    }
    // up would have made shop the current project.
    assert_eq!(s.list()["current_project"], "other");
}

#[test]
fn with_yes_up_lists_the_commands_routes_and_remembers_nothing() {
    let s = Sandbox::new();
    let dir = trust_repo(&s, "shop");
    let file = dir.join("lokal.toml");
    for flag in ["--yes", "-y"] {
        let o = s.ok(&dir, &["up", flag]);
        let out = stdout(&o);
        let note = "  note: running these without asking (--yes); nothing is remembered\n";
        // The listing and the note come first, then the routes.
        assert!(
            out.starts_with(&format!(
                "{}{note}✓ https://shop.lokal → ",
                trust_listing(&file)
            )),
            "{out}"
        );
        assert!(
            out.contains("✓ https://api.shop.lokal → localhost:"),
            "{out}"
        );
        assert_eq!(subdomains(&s, "shop"), ["@", "api", "web"]);
        assert!(s.records().is_empty(), "{:?}", s.records());
    }
    // Nothing was remembered: without the flag, it still refuses.
    assert_eq!(code(&s.run(&dir, &["up"])), 1);
}

#[test]
fn on_a_terminal_only_y_approves_and_the_record_holds_the_files_digest() {
    let s = Sandbox::new();
    let dir = trust_repo(&s, "shop");
    let file = dir.join("lokal.toml");
    let declined = "Not approved; nothing was started.";
    // n, an empty answer, and EOF (Ctrl-D) all decline.
    for answer in ["n\n", "\n", "\x04"] {
        let (code, out) = s.up_on_pty(&dir, answer, declined);
        assert_eq!(code, 1, "{answer:?}: {out}");
        assert!(
            out.contains(&format!("{} runs these commands:", file.display())),
            "{out}"
        );
        assert!(out.contains("  web  vite --port {port}"), "{out}");
        assert!(!out.contains("✓ https://"), "{out}");
        assert_eq!(s.state(), None, "{answer:?}: nothing routed");
        assert!(s.records().is_empty(), "{answer:?}");
    }
    let (code, out) = s.up_on_pty(&dir, "y\n", "routes saved");
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains("✓ https://api.shop.lokal → localhost:"),
        "{out}"
    );
    assert!(!out.contains(declined), "{out}");
    assert_eq!(subdomains(&s, "shop"), ["@", "api", "web"]);
    let rec = s.record_of(&file);
    assert_eq!(s.records(), std::slice::from_ref(&rec));
    assert_eq!(
        std::fs::read_to_string(&rec).unwrap(),
        format!("{}\n", sha256_hex(&std::fs::read(&file).unwrap()))
    );
    assert_eq!(mode(&rec), 0o600);
    assert_eq!(mode(&s.trusted_dir()), 0o700);
    // Approved: no listing and no question, even with no terminal.
    let o = s.ok(&dir, &["up"]);
    assert!(
        stdout(&o).starts_with("✓ https://shop.lokal → "),
        "{}",
        stdout(&o)
    );
    assert!(
        !stdout(&o).contains("runs these commands"),
        "{}",
        stdout(&o)
    );
}

#[test]
fn a_changed_file_asks_again_and_yes_leaves_the_old_approval() {
    let s = Sandbox::new();
    let dir = trust_repo(&s, "shop");
    let file = dir.join("lokal.toml");
    assert_eq!(s.up_on_pty(&dir, "y\n", "routes saved").0, 0);
    let rec = s.record_of(&file);
    let approved = std::fs::read(&rec).unwrap();
    // Any change, even a comment.
    let mut changed = std::fs::read_to_string(&file).unwrap();
    changed.push_str("# a comment\n");
    std::fs::write(&file, &changed).unwrap();
    let before = s.state();
    let o = s.run(&dir, &["up"]);
    assert_eq!(code(&o), 1, "{}", stderr(&o));
    assert_eq!(stdout(&o), trust_listing(&file));
    assert_eq!(
        stderr(&o),
        format!(
            "error: lokal.toml changed since it was approved: {}\n{REFUSAL_HINT}",
            file.display()
        )
    );
    assert_no_yes(&o);
    assert_eq!(s.state(), before);
    // --yes runs it and doesn't touch the approval.
    s.ok(&dir, &["up", "--yes"]);
    assert_eq!(std::fs::read(&rec).unwrap(), approved);
    assert_eq!(code(&s.run(&dir, &["up"])), 1);
    // A person approves the new bytes: the record now holds their digest.
    assert_eq!(s.up_on_pty(&dir, "y\n", "routes saved").0, 0);
    assert_eq!(
        std::fs::read_to_string(&rec).unwrap(),
        format!("{}\n", sha256_hex(changed.as_bytes()))
    );
    assert_eq!(s.records(), [rec]);
    s.ok(&dir, &["up"]);
}

#[test]
fn an_approval_follows_the_real_path_and_a_copy_asks_again() {
    let s = Sandbox::new();
    let dir = trust_repo(&s, "shop");
    assert_eq!(s.up_on_pty(&dir, "y\n", "routes saved").0, 0);
    // Through a symlinked repo path (from a subdirectory): the same file,
    // so the same approval, and no question.
    let link = s.repos.path().join("link");
    std::os::unix::fs::symlink(&dir, &link).unwrap();
    let o = s.ok(&link.join("services/api"), &["up"]);
    assert!(
        !stdout(&o).contains("runs these commands"),
        "{}",
        stdout(&o)
    );
    assert_eq!(s.records().len(), 1);
    // A copy of the file at another path is another file: it asks again.
    let copy = trust_repo(&s, "copy/shop");
    let o = s.run(&copy, &["up"]);
    assert_eq!(code(&o), 1, "{}", stderr(&o));
    assert!(
        stderr(&o).contains(&format!(
            "error: lokal.toml has commands nobody has approved yet: {}\n",
            copy.join("lokal.toml").display()
        )),
        "{}",
        stderr(&o)
    );
    assert_eq!(s.records().len(), 1);
}

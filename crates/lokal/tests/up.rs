//! `lokal.toml`, `lokal up` and `lokal down` (SPEC §4.11, M5 steps 1 to 5):
//! the file's errors and warnings, finding the file, the derived project
//! name, idempotence, managed versus hand-added services, clashes, domains,
//! busy ports, the other commands on managed services, the trust check
//! (§16.11): no terminal, `--yes`, a person's answer on a pty, a changed
//! file, and symlinks; and running the commands: the ready line, each
//! stream's lines, the environment, Ctrl-C and SIGTERM reaching every
//! service once, waiting for a whole process group, selective start, the
//! lock, exit codes, and a closed stdout; each service's log: its format,
//! truncation and rotation, and reading it with `lokal logs <SUB>`; the
//! wrong-port hint and the retry when a picked port turns out to be taken;
//! servers that detach from their command (`setsid`), which lokal names,
//! counts once and records for the next `up`'s busy-port error; and
//! `lokal restart <SUB>`, which asks the running `up` to restart a service
//! and returns when its URL works again.
//! Each test makes a temp repo and runs lokal in it with a `LOKAL_HOME`
//! sandbox; the ones that need live routes use the sandbox daemon.
//!
//! The tests that aren't about the trust check pass `--yes`, the agent and
//! CI path, unless they fail before it (an error in the file, an unknown
//! SUB). Every long-running command a test starts carries a unique tag in
//! its command line, and [`UpRun`] kills what is left with that tag when
//! the test ends, also when it fails.

mod common;

use std::io::{BufRead, BufReader, Write};
use std::os::fd::OwnedFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

use common::{
    Daemon, PtyOutput, children, group_exists, kill_tagged, spawn_on_pty, tagged, testkit_bin,
    unique_tag,
};
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
        "project = \"p\"\nflavour = 1\n[services]\nweb = \"echo web {port}\"\napi = { run = \"true\", restart = true }\n",
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
            "warning: {}:5:23: unknown key `restart` (ignored)",
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
            "✓ https://web.p.lokal → localhost:{0}  echo web {0}\n",
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
    let root = s.repo("mono", "project = \"outer\"\n[services]\nweb = \"true\"\n");
    let deep = root.join("services/api/src");
    std::fs::create_dir_all(&deep).unwrap();
    s.ok(&deep, &["up", "--yes"]);
    assert_eq!(project_names(&s.list()), ["outer"]);
    // A nested lokal.toml without `project`: named after its directory.
    let inner = s.repo("mono/Inner App", "[services]\napi = \"true\"\n");
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
        "project = \"shop\"\n[services]\n\"@\" = { run = \"true\", port = 3000 }\napi = \"true\"\nweb = { run = \"true\", host_header = \"upstream\" }\n",
    );
    let first = s.ok(&dir, &["up", "--yes"]);
    let state = s.state().unwrap();
    let ports = services(&s.list(), "shop");
    let second = s.ok(&dir, &["up", "--yes"]);
    assert_eq!(s.state().unwrap(), state, "byte-identical");
    assert_eq!(services(&s.list(), "shop"), ports);
    // The same route lines (the commands' own output follows them).
    let routes = |o: &Output| {
        stdout(o)
            .split("  routes saved\n")
            .next()
            .unwrap()
            .to_string()
    };
    assert_eq!(routes(&first), routes(&second));
    // A picked port stays even after other services change.
    std::fs::write(
        dir.join("lokal.toml"),
        "project = \"shop\"\n[services]\napi = \"true\"\nweb = \"true\"\ndocs = \"true\"\n",
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
        "project = \"shop\"\n[services]\nweb = \"true\"\napi = \"true\"\n",
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
        "project = \"shop\"\n[services]\nweb = \"true\"\n",
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
        "project = \"shop\"\n[services]\nweb = \"true\"\napi = \"true\"\n",
    );
    s.ok(&dir, &["project", "create", "shop"]);
    s.ok(
        &dir,
        &["service", "add", "web", "3000", "--host", "127.0.0.1"],
    );
    let before = s.state();
    let o = s.run(&dir, &["up", "--yes"]);
    let err = stderr(&o);
    assert_eq!(code(&o), 2, "{err}");
    assert!(
        err.contains(
            "error: service web already exists with another route than lokal.toml gives it \
(host 127.0.0.1, not localhost), and lokal.toml doesn't manage it (added by hand, or left by lokal run)"
        ),
        "{err}"
    );
    assert!(
        err.contains("hint: lokal service remove web -p shop\n"),
        "{err}"
    );
    assert_eq!(s.state(), before);
    // A route that `lokal run` left with another host-header mode is a
    // clash too, and one command clears every clash.
    s.ok(
        &dir,
        &[
            "run",
            "api",
            "--host-header",
            "upstream",
            "--",
            "/usr/bin/true",
        ],
    );
    let before = s.state();
    let o = s.run(&dir, &["up", "--yes"]);
    let err = stderr(&o);
    assert_eq!(code(&o), 2, "{err}");
    assert!(
        err.contains(
            "error: services api, web already exist with other routes than lokal.toml gives them \
(api: host_header \"upstream\", not \"preserve\"; web: host 127.0.0.1, not localhost)"
        ),
        "{err}"
    );
    assert!(err.contains("left by lokal run"), "{err}");
    assert!(
        err.contains("hint: lokal service remove api web -p shop\n"),
        "{err}"
    );
    assert_eq!(s.state(), before);
    // The hint works: then up adds both with the file's routes.
    s.ok(&dir, &["service", "remove", "api", "web", "-p", "shop"]);
    s.ok(&dir, &["up", "--yes"]);
    let svcs = services(&s.list(), "shop");
    assert!(svcs.iter().all(|x| x.2 == managed()), "{svcs:?}");
}

/// A service added by hand (or by `lokal run`) whose route is the one the
/// file gives it is adopted: no error, no route moves, one note, and
/// `lokal down` removes it from then on.
#[test]
fn a_hand_added_service_with_the_files_route_is_adopted() {
    let s = Sandbox::new();
    let port = free_port();
    let dir = s.repo(
        "shop",
        &format!(
            "project = \"shop\"\n[services]\nweb = {{ run = \"true\", port = {port} }}\napi = \"true\"\n"
        ),
    );
    s.ok(&dir, &["project", "create", "shop"]);
    s.ok(&dir, &["service", "add", "web", &port.to_string()]);
    s.ok(&dir, &["run", "api", "--", "/usr/bin/true"]);
    s.ok(&dir, &["service", "add", "db", "5432"]);
    let before = services(&s.list(), "shop");
    let o = s.ok(&dir, &["up", "--yes"]);
    let out = stdout(&o);
    assert!(
        out.contains(
            "  note: adopted api, web (added by hand or by lokal run, with the routes lokal.toml gives \
them); lokal down removes them now\n"
        ),
        "{out}"
    );
    // The same ports, now managed; the hand-added db stays as it was.
    let after = services(&s.list(), "shop");
    let marked: Vec<(String, u64, Value)> = before
        .into_iter()
        .map(|(sub, port, m)| {
            let m = if sub == "db" { m } else { managed() };
            (sub, port, m)
        })
        .collect();
    assert_eq!(after, marked);
    let state = String::from_utf8(s.state().unwrap()).unwrap();
    assert_eq!(
        state.matches("managed = \"lokal.toml\"").count(),
        2,
        "{state}"
    );
    // The next up has nothing to adopt and changes nothing.
    let bytes = s.state();
    let o = s.ok(&dir, &["up", "--yes"]);
    assert!(!stdout(&o).contains("adopted"), "{}", stdout(&o));
    assert_eq!(s.state(), bytes);
    let o = s.ok(&dir, &["down"]);
    assert!(
        stdout(&o).contains("✓ Removed 2 services from lokal.toml"),
        "{}",
        stdout(&o)
    );
    assert_eq!(
        services(&s.list(), "shop"),
        [("db".to_string(), 5432, Value::Null)]
    );
}

#[test]
fn a_different_domain_fails_and_no_domain_key_takes_the_projects() {
    let s = Sandbox::new();
    let dir = s.repo(
        "shop",
        "project = \"shop\"\ndomain = \"shop.lokal\"\n[services]\nweb = \"true\"\n",
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
        "project = \"shop\"\n[services]\nweb = \"true\"\n",
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
        &format!(
            "project = \"shop\"\n[services]\na = {{ run = \"x\", port = {p} }}\nb = \"true\"\n"
        ),
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
        "project = \"shop\"\n[services]\n\"@\" = \"a\"\napi = \"true\"\n",
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
        api.as_deref().is_some_and(|l| l.ends_with("  true")),
        "{}",
        stdout(&o)
    );
}

#[test]
fn colliding_url_variables_warn_once() {
    let s = Sandbox::new();
    let dir = s.repo(
        "shop",
        "project = \"shop\"\n[services]\n\"v1.api\" = \"true\"\nv1-api = \"true\"\nweb = \"true\"\n",
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
    let dir = s.repo("shop", "project = \"shop\"\n[services]\nweb = \"true\"\n");
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
        "project = \"shop\"\n[services]\nweb = \"true\"\napi = \"true\"\n",
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
    let dir = s.repo("Shop", "[services]\nweb = \"true\"\n");
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
    std::fs::write(dir.join("lokal.toml"), "[services]\nweb = \"true\"\n").unwrap();
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
        format!("project = \"p\"\n[services]\n\"@\" = {{ run = \"true\", port = {a} }}\napi = {{ run = \"true\", port = {b} }}\n"),
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
        "project = \"p\"\n[services]\nweb = \"true\"\n",
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
/// The commands only echo: `up` runs them once the check passes.
const TRUST_TOML: &str = "project = \"shop\"\n[services]\n\"@\" = \"echo npm start\"\napi = { run = \"echo cargo run -p api\", cwd = \"services/api\" }\nweb = { run = \"echo vite --port {port}\", env = { NODE_ENV = \"development\" } }\n";

fn trust_repo(s: &Sandbox, name: &str) -> PathBuf {
    let dir = s.repo(name, TRUST_TOML);
    std::fs::create_dir_all(dir.join("services/api")).unwrap();
    dir
}

/// The listing of TRUST_TOML at `file`.
fn trust_listing(file: &Path) -> String {
    format!(
        "{} runs these commands:\n  @    echo npm start\n  api  {:<23}  in services/api\n  web  echo vite --port {{port}}\n       env NODE_ENV=development\n",
        file.display(),
        "echo cargo run -p api"
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
        assert!(out.contains("  web  echo vite --port {port}"), "{out}");
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

/// A `lokal.toml` that is a symlink to another repo's is that repo's file:
/// the approval, the directory its commands run in (the `cwd` base) and the
/// derived project name all come from the real path. So an approval can't
/// be reused to run the same commands somewhere else, and `up` and `down`
/// agree on the project.
#[test]
fn a_symlinked_file_is_the_real_files_project_and_runs_in_its_directory() {
    let s = Sandbox::new();
    // No `project` key: the name comes from the directory.
    let shop = s.repo(
        "shop",
        "[services]\napi = { run = \"true\", cwd = \"sub\" }\n",
    );
    std::fs::create_dir(shop.join("sub")).unwrap();
    assert_eq!(s.up_on_pty(&shop, "y\n", "routes saved").0, 0);
    let b = s.repos.path().join("b");
    std::fs::create_dir(&b).unwrap();
    std::os::unix::fs::symlink("../shop/lokal.toml", b.join("lokal.toml")).unwrap();
    // No terminal and no --yes: shop's approval, because it is shop's file,
    // and shop's directory: `sub` exists only there, so a cwd based on b
    // would be an error.
    let o = s.run(&b, &["up"]);
    assert_eq!(code(&o), 0, "{}{}", stdout(&o), stderr(&o));
    assert!(
        !stdout(&o).contains("runs these commands"),
        "{}",
        stdout(&o)
    );
    assert!(
        stdout(&o).contains("✓ https://api.shop.lokal → localhost:"),
        "{}",
        stdout(&o)
    );
    assert_eq!(project_names(&s.list()), ["shop"]);
    // With a `sub` of b's own, it is still shop's project, not one named b.
    std::fs::create_dir(b.join("sub")).unwrap();
    s.ok(&b, &["up"]);
    let v = s.list();
    assert_eq!(project_names(&v), ["shop"]);
    assert_eq!(v["current_project"], "shop");
    assert_eq!(s.records().len(), 1);
    // down derives the same name.
    let o = s.ok(&b, &["down"]);
    assert!(
        stdout(&o).contains("✓ Removed 1 service from lokal.toml"),
        "{}",
        stdout(&o)
    );
    assert!(services(&s.list(), "shop").is_empty());
}

// ───────────────────────── running the commands (SPEC §4.11) ─────────────────────────

/// A `lokal up` started in the background, with stdout and stderr collected
/// (on a pty, both are the terminal). Dropping it stops lokal with SIGTERM,
/// which stops its services, then kills whatever is left with one of the
/// test's tags, so a failed test leaves nothing running.
struct UpRun {
    child: Child,
    out: PtyOutput,
    err: PtyOutput,
    master: Option<OwnedFd>,
    tags: Vec<String>,
}

impl UpRun {
    /// `lokal args…` in `cwd` with stdin closed: no terminal.
    fn start(home: &Path, cwd: &Path, args: &[&str], tags: &[&str]) -> Self {
        Self::spawn(lokal_cmd(home, cwd, args), tags)
    }

    fn spawn(mut cmd: Command, tags: &[&str]) -> Self {
        let mut child = cmd
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let out = PtyOutput::from_reader(child.stdout.take().unwrap());
        let err = PtyOutput::from_reader(child.stderr.take().unwrap());
        Self {
            child,
            out,
            err,
            master: None,
            tags: tags.iter().map(|t| t.to_string()).collect(),
        }
    }

    /// On a new pty, which is lokal's controlling terminal, as in a shell.
    fn on_pty(home: &Path, cwd: &Path, args: &[&str], tags: &[&str]) -> Self {
        let (child, master) = spawn_on_pty(lokal_cmd(home, cwd, args));
        let out = PtyOutput::collect(&master);
        Self {
            child,
            err: out.clone(),
            out,
            master: Some(master),
            tags: tags.iter().map(|t| t.to_string()).collect(),
        }
    }

    fn both(&self) -> String {
        format!("stdout:\n{}\nstderr:\n{}", self.out.text(), self.err.text())
    }

    /// Waits up to `limit` until stdout contains `needle`.
    fn wait_out(&self, needle: &str, limit: Duration) {
        let deadline = Instant::now() + limit;
        while !self.out.text().contains(needle) {
            assert!(
                Instant::now() < deadline,
                "no {needle:?} within {limit:?}; {}",
                self.both()
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn signal(&self, sig: libc::c_int) {
        // SAFETY: kill only sends a signal to our child's pid.
        assert_eq!(unsafe { libc::kill(self.child.id() as i32, sig) }, 0);
    }

    /// Types `keys` on the pty.
    fn type_keys(&self, keys: &[u8]) {
        let master = self.master.as_ref().expect("a pty run");
        std::fs::File::from(master.try_clone().unwrap())
            .write_all(keys)
            .unwrap();
    }

    /// Waits up to `limit` for lokal to exit and returns its exit code.
    fn wait_exit(&mut self, limit: Duration) -> i32 {
        let deadline = Instant::now() + limit;
        loop {
            if let Some(st) = self.child.try_wait().unwrap() {
                return st.code().unwrap_or(-1);
            }
            assert!(
                Instant::now() < deadline,
                "lokal didn't exit within {limit:?}; {}",
                self.both()
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// lokal's children: each service's shell, whose pid is its process
    /// group's id.
    fn shells(&self) -> Vec<i32> {
        children(self.child.id())
    }
}

impl Drop for UpRun {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            // lokal's own stop: SIGINT, SIGTERM at 5s, SIGKILL at 10s.
            // SAFETY: kill only sends a signal to our child's pid.
            unsafe { libc::kill(self.child.id() as i32, libc::SIGTERM) };
            let deadline = Instant::now() + Duration::from_secs(15);
            while self.child.try_wait().ok().flatten().is_none() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(20));
            }
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
        kill_tagged(&self.tags);
    }
}

/// `'<path>'`: a testkit program in a `run` string.
fn quoted(bin: &str) -> String {
    assert!(!bin.contains('\''), "{bin}");
    format!("'{bin}'")
}

/// The port in `✓ https://<fqdn> → localhost:<port>  …`.
fn routed_port(out: &str, fqdn: &str) -> u16 {
    let start = format!("✓ https://{fqdn} → localhost:");
    out.lines()
        .find_map(|l| l.strip_prefix(start.as_str()))
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|p| p.parse().ok())
        .unwrap_or_else(|| panic!("no route line for {fqdn} in:\n{out}"))
}

/// Every process group in `shells` is gone, and nothing with `tag` runs.
fn assert_all_gone(shells: &[i32], tag: &str) {
    for g in shells {
        assert!(!group_exists(*g), "process group {g} is still there");
    }
    let left = tagged(&[tag.to_string()]);
    assert!(left.is_empty(), "left running: {left:?}");
}

/// A repo `name` with `toml`, next to the sandbox daemon (which has its
/// own `LOKAL_HOME`).
fn daemon_repo(repos: &Path, name: &str, toml: &str) -> PathBuf {
    let dir = repos.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("lokal.toml"), toml).unwrap();
    dir.canonicalize().unwrap()
}

#[test]
fn up_starts_every_service_and_each_url_works_on_the_first_request() {
    let d = Daemon::start("none");
    let repos = tempfile::Builder::new().prefix("lkr").tempdir().unwrap();
    let echo = quoted(&testkit_bin("echo-server"));
    let tag = unique_tag();
    let dir = daemon_repo(
        repos.path(),
        "shop",
        &format!(
            "project = \"p\"\n[services]\na = \"{echo} --port {{port}} --marker A-{tag}\"\nb = \"{echo} --port {{port}} --marker B-{tag}\"\n"
        ),
    );
    let mut up = UpRun::start(d.dir.path(), &dir, &["up", "--yes"], &[&tag]);
    up.wait_out(
        "✓ ready: 2/2 services accepting connections (",
        Duration::from_secs(10),
    );
    let out = up.out.text();
    // The routes are ready before any command starts, and come first.
    let routes = out.find("  routes ready in ").expect("routes line");
    assert!(
        out.find("✓ https://a.p.lokal → localhost:").unwrap() < routes,
        "{out}"
    );
    assert!(routes < out.find("✓ ready:").unwrap(), "{out}");
    // `✓ ready:` promises every URL works: the first request, no retry.
    for (url, marker) in [
        ("https://a.p.lokal/", format!("A-{tag}")),
        ("https://b.p.lokal/", format!("B-{tag}")),
    ] {
        let r = https_get(&d, url).unwrap();
        assert_eq!(r.status, 200, "{url}");
        assert_eq!(r.json().unwrap()["marker"], marker.as_str(), "{url}");
    }
    let shells = up.shells();
    assert_eq!(shells.len(), 2, "{shells:?}");
    up.signal(libc::SIGINT);
    assert_eq!(up.wait_exit(Duration::from_secs(15)), 130, "{}", up.both());
    let out = up.out.text();
    assert!(out.contains("stopping 2 services…"), "{out}");
    assert!(
        out.ends_with("done. Routes kept; 'lokal down' removes them.\n"),
        "{out}"
    );
    assert_all_gone(&shells, &tag);
    // The routes stay.
    let subs: Vec<String> = services(&list(d.dir.path()), "p")
        .into_iter()
        .map(|x| x.0)
        .collect();
    assert_eq!(subs, ["a", "b"]);
}

#[test]
fn each_line_keeps_its_stream_and_gets_the_services_name() {
    let s = Sandbox::new();
    let echo = quoted(&testkit_bin("echo-server"));
    let tag = unique_tag();
    let dir = s.repo(
        "shop",
        &format!(
            "project = \"p\"\n[services]\na = \"exec {echo} --port {{port}} --marker A-{tag}\"\nbb = \"echo oops >&2; exec {echo} --port {{port}} --marker B-{tag}\"\n"
        ),
    );
    let up = UpRun::start(s.home.path(), &dir, &["up", "--yes"], &[&tag]);
    // No daemon here: the URLs don't work, so the line says listening.
    up.wait_out(
        "✓ listening: 2/2 services on localhost\n",
        Duration::from_secs(10),
    );
    let (out, err) = (up.out.text(), up.err.text());
    let port = routed_port(&out, "a.p.lokal");
    // Padded to the longest started name.
    assert!(
        out.contains(&format!("\na  | LISTENING 127.0.0.1:{port}\n")),
        "{out}"
    );
    assert!(err.contains("bb | oops\n"), "{err}");
    assert!(!out.contains("| oops"), "{out}");
    assert!(!err.contains("LISTENING"), "{err}");
    // lokal's own lines go to stdout, the §4.5 note before the ready line.
    assert!(!err.contains('✓'), "{err}");
    let note = out
        .find("note: the background service isn't running")
        .expect("the §4.5 note");
    assert!(note < out.find("✓ listening:").unwrap(), "{out}");
}

#[test]
fn the_shell_gets_the_port_the_urls_its_cwd_and_the_files_env() {
    let s = Sandbox::new();
    let dir = s.repo(
        "shop",
        "project = \"p\"\n[services]\na = \"true\"\nb = { run = \"echo port={port} PORT=$PORT HOST=$HOST URL=$LOKAL_URL A=$LOKAL_URL_A B=$LOKAL_URL_B X=$FROM_FILE\", env = { FROM_FILE = \"from lokal.toml\" } }\nc = { run = \"pwd\", cwd = \"sub\" }\n",
    );
    std::fs::create_dir(dir.join("sub")).unwrap();
    let mut up = UpRun::start(s.home.path(), &dir, &["up", "--yes"], &[]);
    assert_eq!(up.wait_exit(Duration::from_secs(10)), 0, "{}", up.both());
    let out = up.out.text();
    let port = routed_port(&out, "b.p.lokal");
    assert!(
        out.contains(&format!(
            "b | port={port} PORT={port} HOST=127.0.0.1 URL=https://b.p.lokal A=https://a.p.lokal B=https://b.p.lokal X=from lokal.toml\n"
        )),
        "{out}"
    );
    assert!(
        out.contains(&format!("c | {}\n", dir.join("sub").display())),
        "{out}"
    );
    for sub in ["a", "b", "c"] {
        assert!(out.contains(&format!("\n{sub} exited 0\n")), "{out}");
    }
    // Every service exited before it listened: none counts as ready, and
    // the ready line names them.
    assert!(
        out.ends_with(
            "✓ listening: 0/3 services on localhost; not ready: a (exited 0), b (exited 0), c (exited 0)\n"
        ),
        "{out}"
    );
}

/// Two `sig-probe` services, each printing how many signals reached it.
fn sig_probe_repo(s: &Sandbox, tag: &str) -> PathBuf {
    let probe = quoted(&testkit_bin("sig-probe"));
    s.repo(
        "shop",
        &format!(
            "project = \"p\"\n[services]\na = \"exec {probe} --port {{port}} --tag {tag}\"\nb = \"exec {probe} --port {{port}} --tag {tag}\"\n"
        ),
    )
}

/// The terminal's Ctrl-C reaches only lokal (its services run in their own
/// process groups), and lokal sends each group exactly one SIGINT.
#[test]
fn ctrl_c_on_a_terminal_reaches_each_service_once() {
    let s = Sandbox::new();
    let tag = unique_tag();
    let dir = sig_probe_repo(&s, &tag);
    let mut up = UpRun::on_pty(s.home.path(), &dir, &["up", "--yes"], &[&tag]);
    up.wait_out("✓ listening: 2/2", Duration::from_secs(10));
    let shells = up.shells();
    assert_eq!(shells.len(), 2, "{shells:?}");
    up.type_keys(b"\x03");
    assert_eq!(up.wait_exit(Duration::from_secs(10)), 130, "{}", up.both());
    let out = up.out.text();
    for sub in ["a", "b"] {
        assert!(
            out.contains(&format!("{sub} | signals: 1")),
            "want exactly one SIGINT for {sub}:\n{out}"
        );
    }
    assert_all_gone(&shells, &tag);
}

/// With no terminal (an agent's `kill`), SIGTERM stops every service the
/// same way, with one SIGINT each, and lokal exits 143.
#[test]
fn sigterm_without_a_terminal_stops_every_service() {
    let s = Sandbox::new();
    let tag = unique_tag();
    let dir = sig_probe_repo(&s, &tag);
    let mut cmd = lokal_cmd(s.home.path(), &dir, &["up", "--yes"]);
    // SAFETY: only setsid runs between fork and exec: a new session with no
    // controlling terminal, so the test's own terminal can't interfere.
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut up = UpRun::spawn(cmd, &[&tag]);
    up.wait_out("✓ listening: 2/2", Duration::from_secs(10));
    let shells = up.shells();
    assert_eq!(shells.len(), 2, "{shells:?}");
    up.signal(libc::SIGTERM);
    assert_eq!(up.wait_exit(Duration::from_secs(10)), 143, "{}", up.both());
    let out = up.out.text();
    for sub in ["a", "b"] {
        assert!(out.contains(&format!("{sub} | signals: 1\n")), "{out}");
    }
    assert!(
        out.contains("done. Routes kept; 'lokal down' removes them.\n"),
        "{out}"
    );
    assert_all_gone(&shells, &tag);
}

/// Waits up to 5s until a process runs whose command line is exactly `cmd`.
fn wait_for_process(tag: &str, cmd: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !tagged(&[tag.to_string()]).iter().any(|p| p.2 == cmd) {
        assert!(
            Instant::now() < deadline,
            "no `{cmd}` within 5s: {:?}",
            tagged(&[tag.to_string()])
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// "Stopped" means the process group is empty: a background job, which a
/// shell starts with SIGINT ignored, outlives the SIGINT, and lokal waits
/// for it until the SIGTERM at 5s ends it (VERIFY V10).
#[test]
fn up_waits_for_the_whole_process_group() {
    let s = Sandbox::new();
    let probe = quoted(&testkit_bin("sig-probe"));
    let tag = unique_tag();
    let sleep = format!("sleep 60.{tag}");
    let dir = s.repo(
        "shop",
        &format!(
            "project = \"p\"\n[services]\na = \"{sleep} & exec {probe} --port {{port}} --tag {tag}\"\n"
        ),
    );
    let mut up = UpRun::start(s.home.path(), &dir, &["up", "--yes"], &[&tag]);
    up.wait_out("✓ listening: 1/1", Duration::from_secs(10));
    wait_for_process(&tag, &sleep);
    let shells = up.shells();
    let t = Instant::now();
    up.signal(libc::SIGINT);
    assert_eq!(up.wait_exit(Duration::from_secs(15)), 130, "{}", up.both());
    let took = t.elapsed();
    assert!(up.out.text().contains("a | signals: 1\n"), "{}", up.both());
    // Not when the shell's sig-probe exited (~0.3s), but after SIGTERM at 5s
    // ended the background sleep, and before SIGKILL at 10s.
    assert!(took >= Duration::from_secs(5), "exited after {took:?}");
    assert!(took < Duration::from_secs(8), "exited after {took:?}");
    assert_all_gone(&shells, &tag);
}

/// A service that ignores SIGINT and SIGTERM: a second signal sends SIGKILL
/// at once.
#[test]
fn a_second_signal_kills_at_once() {
    let s = Sandbox::new();
    let tag = unique_tag();
    let sleep = format!("sleep 60.{tag}");
    let dir = s.repo(
        "shop",
        &format!("project = \"p\"\n[services]\na = \"trap '' INT TERM; exec {sleep}\"\n"),
    );
    let mut up = UpRun::start(s.home.path(), &dir, &["up", "--yes"], &[&tag]);
    // After the exec, the trap is in place.
    wait_for_process(&tag, &sleep);
    let shells = up.shells();
    let t = Instant::now();
    up.signal(libc::SIGINT);
    up.wait_out("stopping 1 service…", Duration::from_secs(5));
    let second = Instant::now();
    up.signal(libc::SIGINT);
    assert_eq!(up.wait_exit(Duration::from_secs(5)), 130, "{}", up.both());
    assert!(
        second.elapsed() < Duration::from_secs(1),
        "{:?}",
        second.elapsed()
    );
    // Not the SIGKILL at 10s, nor the SIGTERM at 5s, which it ignores.
    assert!(t.elapsed() < Duration::from_secs(5), "{:?}", t.elapsed());
    assert_all_gone(&shells, &tag);
}

#[test]
fn up_with_a_sub_starts_only_that_service_and_routes_every_one() {
    let s = Sandbox::new();
    let echo = quoted(&testkit_bin("echo-server"));
    let (ta, tb) = (unique_tag(), unique_tag());
    let dir = s.repo(
        "shop",
        &format!(
            "project = \"p\"\n[services]\na = \"exec {echo} --port {{port}} --marker A-{ta}\"\nb = \"exec {echo} --port {{port}} --marker B-{tb}\"\n"
        ),
    );
    let mut up = UpRun::start(s.home.path(), &dir, &["up", "--yes", "a"], &[&ta, &tb]);
    up.wait_out("✓ listening: 1/1", Duration::from_secs(10));
    let out = up.out.text();
    let b = out
        .lines()
        .find(|l| l.starts_with("✓ https://b.p.lokal → localhost:"))
        .unwrap_or_else(|| panic!("{out}"));
    assert!(b.ends_with("  (not started)"), "{b}");
    assert!(
        tagged(std::slice::from_ref(&tb)).is_empty(),
        "b must not start"
    );
    let subs: Vec<String> = services(&s.list(), "p").into_iter().map(|x| x.0).collect();
    assert_eq!(subs, ["a", "b"]);
    up.signal(libc::SIGINT);
    assert_eq!(up.wait_exit(Duration::from_secs(10)), 130, "{}", up.both());
    assert!(
        up.out.text().contains("stopping 1 service…"),
        "{}",
        up.both()
    );
}

/// One `up` per project: a second one and `down` both exit 1 while the
/// first runs, and change nothing. The kernel drops the lock with lokal.
#[test]
fn the_lock_keeps_a_second_up_and_down_out() {
    let s = Sandbox::new();
    let echo = quoted(&testkit_bin("echo-server"));
    let tag = unique_tag();
    let dir = s.repo(
        "shop",
        &format!(
            "project = \"p\"\n[services]\na = \"exec {echo} --port {{port}} --marker A-{tag}\"\n"
        ),
    );
    let mut first = UpRun::start(s.home.path(), &dir, &["up", "--yes"], &[&tag]);
    first.wait_out("✓ listening: 1/1", Duration::from_secs(10));
    let pid = first.child.id();
    let lock = s.home.path().join("logs/up/p/up.lock");
    assert_eq!(std::fs::read_to_string(&lock).unwrap(), format!("{pid}\n"));
    assert_eq!(mode(&lock), 0o600);
    assert_eq!(mode(lock.parent().unwrap()), 0o700);
    // Its socket for lokal restart, next to the lock.
    let sock = s.home.path().join("logs/up/p/up.sock");
    assert_eq!(mode(&sock), 0o600);
    let state = s.state();
    // Refused, with the way to restart a service of the running up, which
    // names the SUBs the refused up was given.
    for (args, hint) in [
        (
            &["up", "--yes"][..],
            "restart a service: lokal restart <SUB> -p p; see its output: lokal logs <SUB>",
        ),
        (
            &["up", "--yes", "a"],
            "restart it: lokal restart a -p p; see its output: lokal logs a",
        ),
    ] {
        let o = s.run(&dir, args);
        assert_eq!(code(&o), 1, "{}", stderr(&o));
        assert!(
            stderr(&o).ends_with(&format!(
                "error: lokal up is already running for p (pid {pid})\nhint: {hint}\n"
            )),
            "{}",
            stderr(&o)
        );
        assert!(!stdout(&o).contains("✓ https://"), "{}", stdout(&o));
    }
    let o = s.run(&dir, &["down"]);
    assert_eq!(code(&o), 1, "{}", stderr(&o));
    assert!(
        stderr(&o).contains(&format!(
            "error: lokal up is running for p (pid {pid}); stop it first (Ctrl-C in its terminal)\n"
        )),
        "{}",
        stderr(&o)
    );
    assert_eq!(s.state(), state, "nothing removed");
    first.signal(libc::SIGINT);
    assert_eq!(first.wait_exit(Duration::from_secs(10)), 130);
    assert!(!sock.exists(), "up removes its socket");
    // The file is left with the old pid; it doesn't block the next up.
    let mut again = UpRun::start(s.home.path(), &dir, &["up", "--yes"], &[&tag]);
    again.wait_out("✓ listening: 1/1", Duration::from_secs(10));
    assert_eq!(
        std::fs::read_to_string(&lock).unwrap(),
        format!("{}\n", again.child.id())
    );
    again.signal(libc::SIGINT);
    assert_eq!(again.wait_exit(Duration::from_secs(10)), 130);
    let o = s.ok(&dir, &["down"]);
    assert!(
        stdout(&o).contains("✓ Removed 1 service from lokal.toml"),
        "{}",
        stdout(&o)
    );
}

#[test]
fn exit_codes_when_every_command_exits_on_its_own() {
    let s = Sandbox::new();
    let up = |toml: &str| {
        let dir = s.repo("shop", &format!("project = \"p\"\n[services]\n{toml}"));
        let mut up = UpRun::start(s.home.path(), &dir, &["up", "--yes"], &[]);
        let code = up.wait_exit(Duration::from_secs(10));
        (code, up.out.text(), up.err.text())
    };
    let (code, out, _) = up("a = \"exit 0\"\nb = \"exit 0\"\n");
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains("\na exited 0\n") && out.contains("\nb exited 0\n"),
        "{out}"
    );
    let (code, out, _) = up("a = \"exit 0\"\nb = \"exit 3\"\n");
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("\nb exited 3\n"), "{out}");
    let (code, out, err) = up("x = \"no-such-cmd-lk\"\n");
    assert_eq!(code, 1, "{out}");
    let line = err
        .lines()
        .find(|l| l.starts_with("x | "))
        .unwrap_or_else(|| panic!("{err}"));
    assert!(line.contains("no-such-cmd-lk: command not found"), "{line}");
    assert!(out.contains("\nx exited 127\n"), "{out}");
}

#[test]
fn a_line_longer_than_16_kib_is_split() {
    let s = Sandbox::new();
    // A TOML literal string: the shell gets printf's `\n` as written.
    let dir = s.repo(
        "shop",
        "project = \"p\"\n[services]\na = 'printf \"%040000d\\n\" 0'\n",
    );
    let mut up = UpRun::start(s.home.path(), &dir, &["up", "--yes"], &[]);
    assert_eq!(up.wait_exit(Duration::from_secs(10)), 0, "{}", up.both());
    let lens: Vec<usize> = up
        .out
        .text()
        .lines()
        .filter_map(|l| l.strip_prefix("a | "))
        .map(str::len)
        .collect();
    assert_eq!(lens, [16384, 16384, 40000 - 2 * 16384]);
}

/// Regression guard: with its stdout closed (`lokal up | head -5`), lokal
/// must not die of SIGPIPE and leave the services running in their own
/// process groups. It stops them, like SIGHUP, and exits 128 + SIGPIPE.
#[test]
fn a_closed_stdout_stops_the_services() {
    let s = Sandbox::new();
    let tag = unique_tag();
    let dir = s.repo(
        "shop",
        &format!(
            "project = \"p\"\n[services]\na = \"while :; do echo tick-{tag}; sleep 0.05; done\"\n"
        ),
    );
    let mut child = lokal_cmd(s.home.path(), &dir, &["up", "--yes"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let err = PtyOutput::from_reader(child.stderr.take().unwrap());
    let mut first = BufReader::new(child.stdout.take().unwrap());
    let mut up = UpRun {
        child,
        out: PtyOutput::from_reader(std::io::empty()),
        err,
        master: None,
        tags: vec![tag.clone()],
    };
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut line = String::new();
    while !line.contains("a | tick-") {
        assert!(Instant::now() < deadline, "no tick: {}", up.both());
        line.clear();
        assert_ne!(first.read_line(&mut line).unwrap(), 0, "{}", up.both());
    }
    let shells = up.shells();
    // The reader is gone: lokal's next write fails.
    drop(first);
    assert_eq!(up.wait_exit(Duration::from_secs(10)), 141, "{}", up.both());
    assert!(!up.err.text().contains("panicked"), "{}", up.both());
    assert_all_gone(&shells, &tag);
}

// ───────────────────────── logs (SPEC §4.11, "Logs") ─────────────────────────

/// `<LOKAL_HOME>/logs/up/<project>/<file>`.
fn up_file(s: &Sandbox, project: &str, file: &str) -> PathBuf {
    s.home.path().join("logs/up").join(project).join(file)
}

/// The text after a line's `<timestamp> `, which must be RFC 3339 UTC with
/// microseconds: `^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d{6}Z `.
fn after_stamp(line: &str) -> &str {
    let b = line.as_bytes();
    let ok = b.len() >= 28
        && b[..28].iter().enumerate().all(|(i, c)| match i {
            4 | 7 => *c == b'-',
            10 => *c == b'T',
            13 | 16 => *c == b':',
            19 => *c == b'.',
            26 => *c == b'Z',
            27 => *c == b' ',
            _ => c.is_ascii_digit(),
        });
    assert!(ok, "no timestamp at the start of {line:?}");
    &line[28..]
}

/// A log's lines without their timestamps (each one checked).
fn log_lines(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
        .lines()
        .map(|l| after_stamp(l).to_string())
        .collect()
}

#[test]
fn a_services_log_has_both_streams_and_lokals_lines_after_a_timestamp() {
    let s = Sandbox::new();
    let dir = s.repo(
        "shop",
        "project = \"p\"\ndomain = \"shop.local\"\n[services]\na = \"echo out-line; echo err-line >&2; exit 3\"\n\"v1.api\" = \"echo dotted\"\n",
    );
    let o = s.run(&dir, &["up", "--yes"]);
    assert_eq!(code(&o), 1, "{}{}", stdout(&o), stderr(&o));
    let port = routed_port(&stdout(&o), "a.shop.local");
    let log = up_file(&s, "p", "a.log");
    let lines = log_lines(&log);
    // Both streams, in one file; lokal's own lines first and last.
    assert_eq!(lines.len(), 4, "{lines:?}");
    assert_eq!(
        lines[0],
        format!("lokal: started on port {port}: echo out-line; echo err-line >&2; exit 3")
    );
    let mut middle = lines[1..3].to_vec();
    middle.sort();
    assert_eq!(middle, ["err-line", "out-line"]);
    assert_eq!(lines[3], "lokal: exited 3");
    assert_eq!(mode(&log), 0o600);
    // A dotted subdomain is one file.
    assert_eq!(
        log_lines(&up_file(&s, "p", "v1.api.log"))[1..],
        ["dotted", "lokal: exited 0"]
    );
    // `-n 3`: the file's last 3 lines, timestamps and all.
    let file = std::fs::read_to_string(&log).unwrap();
    let last3: String = file.lines().skip(1).map(|l| format!("{l}\n")).collect();
    let o = s.ok(&dir, &["logs", "a", "-n", "3"]);
    assert_eq!(stdout(&o), last3);
    // By default, up to 50: all of it.
    assert_eq!(stdout(&s.ok(&dir, &["logs", "a"])), file);
    // After down, with the project gone, the log still reads; SUB is then
    // normalised against the file's apex.
    s.ok(&dir, &["down", "--remove-project"]);
    assert!(project_names(&s.list()).is_empty());
    assert_eq!(stdout(&s.ok(&dir, &["logs", "a"])), file);
    let o = s.ok(&dir, &["logs", "a.shop.local", "-n", "1"]);
    assert!(stdout(&o).ends_with(" lokal: exited 3\n"), "{}", stdout(&o));
}

/// Regression guard: a shell's exit reached lokal before the last of its
/// output, which was still in the pipe (up to its 64 KiB buffer), so
/// `a exited 0` and `lokal: exited 0` came before the service's last lines.
/// The exit is reported once the output it wrote has arrived, and still
/// before the ready line.
#[test]
fn a_services_exit_is_reported_after_its_last_output() {
    let s = Sandbox::new();
    // About 110 KB: more than a pipe holds.
    let dir = s.repo(
        "shop",
        "project = \"p\"\n[services]\na = \"seq 1 20000\"\nb = \"seq 1 20000; exit 4\"\n",
    );
    let o = s.run(&dir, &["up", "--yes"]);
    assert_eq!(code(&o), 1, "{}", stderr(&o));
    let out = stdout(&o);
    for (sub, code) in [("a", 0), ("b", 4)] {
        let last = out
            .find(&format!("\n{sub} | 20000\n"))
            .unwrap_or_else(|| panic!("no last line for {sub}"));
        let exit = out
            .find(&format!("\n{sub} exited {code}\n"))
            .unwrap_or_else(|| panic!("no exit line for {sub}"));
        assert!(last < exit, "{sub} exited before its last line");
        let lines = log_lines(&up_file(&s, "p", &format!("{sub}.log")));
        assert_eq!(lines.len(), 20002, "{sub}");
        assert_eq!(
            lines[20000..],
            ["20000".to_string(), format!("lokal: exited {code}")]
        );
    }
    assert!(
        out.ends_with(
            "✓ listening: 0/2 services on localhost; not ready: a (exited 0), b (exited 4)\n"
        ),
        "the ready line comes after both exits"
    );
}

/// A `lokal logs … -f`, killed when the test ends.
struct Follow {
    child: Child,
    out: PtyOutput,
}

impl Follow {
    fn start(home: &Path, cwd: &Path, args: &[&str]) -> Self {
        let mut child = lokal_cmd(home, cwd, args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let out = PtyOutput::from_reader(child.stdout.take().unwrap());
        Self { child, out }
    }
}

impl Drop for Follow {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn lokal_logs_f_shows_lines_written_after_it_started() {
    let s = Sandbox::new();
    let tag = unique_tag();
    // Prints a second line once the test creates `go`, then idles; the tag
    // is in the shell's own command line.
    let dir = s.repo(
        "shop",
        &format!(
            "project = \"p\"\n[services]\na = \"echo first-{tag}; while [ ! -e go ]; do sleep 0.02; done; echo after-{tag}; while :; do sleep 1; done\"\n"
        ),
    );
    let mut up = UpRun::start(s.home.path(), &dir, &["up", "--yes"], &[&tag]);
    up.wait_out(&format!("a | first-{tag}\n"), Duration::from_secs(10));
    let follow = Follow::start(s.home.path(), &dir, &["logs", "a", "-f"]);
    // The tail first, then what comes after it.
    follow
        .out
        .wait_for(&format!(" first-{tag}\n"), Duration::from_secs(5));
    let after = format!(" after-{tag}\n");
    assert!(!follow.out.text().contains(&after), "{}", follow.out.text());
    std::fs::write(dir.join("go"), "").unwrap();
    up.wait_out(&format!("a | after-{tag}\n"), Duration::from_secs(5));
    // `-f` looks every 200ms.
    follow.out.wait_for(&after, Duration::from_secs(2));
    up.signal(libc::SIGINT);
    assert_eq!(up.wait_exit(Duration::from_secs(10)), 130, "{}", up.both());
    // Stopping is in the log too, for a reader who sees only the file.
    let lines = log_lines(&up_file(&s, "p", "a.log"));
    assert_eq!(
        lines[lines.len() - 2..],
        ["lokal: stopping (SIGINT)", "lokal: exited 130"],
        "{lines:?}"
    );
    follow
        .out
        .wait_for(" lokal: exited 130\n", Duration::from_secs(2));
}

#[test]
fn the_next_up_truncates_the_log_and_a_service_it_doesnt_start_keeps_its_own() {
    let s = Sandbox::new();
    let toml = |a: &str, b: &str| {
        format!("project = \"p\"\n[services]\na = \"echo {a}\"\nb = \"echo {b}\"\n")
    };
    let dir = s.repo("shop", &toml("old-a", "old-b"));
    s.ok(&dir, &["up", "--yes"]);
    assert_eq!(log_lines(&up_file(&s, "p", "a.log"))[1], "old-a");
    // An older rotated log goes too: a log holds one run.
    let rotated = up_file(&s, "p", "a.log.1");
    std::fs::write(&rotated, "2026-10-25T10:44:34.635510Z old-rotated\n").unwrap();
    std::fs::write(dir.join("lokal.toml"), toml("new-a", "new-b")).unwrap();
    s.ok(&dir, &["up", "--yes", "a"]);
    let a = log_lines(&up_file(&s, "p", "a.log"));
    assert_eq!(a[1..], ["new-a", "lokal: exited 0"], "{a:?}");
    assert!(!rotated.exists());
    let o = s.ok(&dir, &["logs", "a"]);
    assert!(!stdout(&o).contains("old-"), "{}", stdout(&o));
    // b wasn't started: its log is the last run's.
    let b = log_lines(&up_file(&s, "p", "b.log"));
    assert_eq!(b[1..], ["old-b", "lokal: exited 0"], "{b:?}");
}

#[test]
fn a_log_past_5_mb_moves_to_log_1_and_n_reads_across_both() {
    const CAP: u64 = 5 * 1024 * 1024;
    let s = Sandbox::new();
    // About 353,000 lines of output, which with their timestamps make
    // about 16 MB of log.
    let dir = s.repo(
        "shop",
        "project = \"p\"\n[services]\na = \"yes lk-rotation-line | head -c 6000000\"\n",
    );
    s.ok(&dir, &["up", "--yes"]);
    let (old, new) = (up_file(&s, "p", "a.log.1"), up_file(&s, "p", "a.log"));
    let len = |p: &Path| std::fs::metadata(p).unwrap().len();
    // Rotated before a line would take it past the cap: full, but not over.
    assert!(len(&old) <= CAP && len(&old) > CAP - 100, "{}", len(&old));
    assert!(len(&new) <= CAP, "{}", len(&new));
    let old_lines = log_lines(&old);
    let new_lines = log_lines(&new);
    assert!(old_lines.iter().all(|l| l == "lk-rotation-line"));
    assert_eq!(new_lines.last().unwrap(), "lokal: exited 0");
    // -n counts back from the end of a.log into a.log.1.
    let n = new_lines.len() + 2;
    let o = s.ok(&dir, &["logs", "a", "-n", &n.to_string()]);
    let old_text = std::fs::read_to_string(&old).unwrap();
    let want: String = old_text
        .lines()
        .skip(old_lines.len() - 2)
        .map(|l| format!("{l}\n"))
        .chain(std::iter::once(std::fs::read_to_string(&new).unwrap()))
        .collect();
    assert!(stdout(&o) == want, "-n {n} doesn't read across both files");
}

#[test]
fn plain_lokal_logs_still_tails_the_daemon_log() {
    let s = Sandbox::new();
    let dir = s.repo(
        "shop",
        "project = \"p\"\n[services]\nd1 = \"echo up-line\"\n",
    );
    s.ok(&dir, &["up", "--yes"]);
    std::fs::write(s.home.path().join("logs/daemon.log"), "d1\nd2\nd3\n").unwrap();
    // In a repo with a lokal.toml, and with a service log named like a
    // daemon line, plain `logs` is still the background service's.
    assert_eq!(stdout(&s.ok(&dir, &["logs", "-n", "2"])), "d2\nd3\n");
    assert_eq!(stdout(&s.ok(&dir, &["logs"])), "d1\nd2\nd3\n");
    assert!(stdout(&s.ok(&dir, &["logs", "d1"])).contains(" up-line\n"));
}

#[test]
fn logs_of_an_unknown_service_or_one_lokal_up_didnt_start() {
    let s = Sandbox::new();
    let dir = s.repo("shop", "project = \"p\"\n[services]\na = \"true\"\n");
    s.ok(&dir, &["up", "--yes"]);
    let o = s.run(&dir, &["logs", "zz"]);
    assert_eq!(code(&o), 2, "{}", stderr(&o));
    assert!(
        stderr(&o).starts_with("error: no service zz in project p\n"),
        "{}",
        stderr(&o)
    );
    // Added by hand, or by `lokal run`: they print to their own terminal.
    s.ok(&dir, &["service", "add", "hand", "4000", "-p", "p"]);
    s.ok(&dir, &["run", "viarun", "-p", "p", "--", "true"]);
    for sub in ["hand", "viarun"] {
        let o = s.run(&dir, &["logs", sub]);
        assert_eq!(code(&o), 1, "{sub}: {}", stderr(&o));
        assert_eq!(
            stderr(&o),
            format!(
                "error: no log for {sub} yet: only services that lokal up starts have one\n\
hint: services started with lokal run or by hand print to their own terminal\n"
            )
        );
        assert_eq!(stdout(&o), "");
    }
    // -p names the project of a SUB: alone, it's a usage error.
    let o = s.run(&dir, &["logs", "-p", "p"]);
    assert_eq!(code(&o), 2, "{}", stderr(&o));
    assert!(stderr(&o).contains("<SUB>"), "{}", stderr(&o));
}

#[test]
fn logs_finds_the_repos_project_even_when_another_is_current() {
    let s = Sandbox::new();
    let dir = s.repo("shop", "project = \"p\"\n[services]\na = \"echo from-p\"\n");
    std::fs::create_dir(dir.join("src")).unwrap();
    s.ok(&dir, &["up", "--yes"]);
    s.ok(&dir, &["project", "create", "other"]);
    s.ok(&dir, &["service", "add", "a", "4001", "-p", "other"]);
    // In the repo, or below it: the file's project, not the current one.
    for cwd in [dir.clone(), dir.join("src")] {
        let o = s.ok(&cwd, &["logs", "a"]);
        assert!(stdout(&o).contains(" from-p\n"), "{}", stdout(&o));
    }
    // Outside it, §4.4: the current project, whose `a` has no log.
    let o = s.run(s.home.path(), &["logs", "a"]);
    assert_eq!(code(&o), 1, "{}", stderr(&o));
    assert!(
        stderr(&o).starts_with("error: no log for a yet"),
        "{}",
        stderr(&o)
    );
    // -p wins over both.
    let o = s.ok(s.home.path(), &["logs", "a", "-p", "p"]);
    assert!(stdout(&o).contains(" from-p\n"), "{}", stdout(&o));
}

// ─────── the wrong-port hint and the retry (SPEC §4.11, M5 step 5) ───────

/// Two different ports nothing listens on.
fn two_free_ports() -> (u16, u16) {
    let a = free_port();
    loop {
        let b = free_port();
        if b != a {
            return (a, b);
        }
    }
}

/// Each of `wants` in `out`, in this order. A line break that ends one may
/// start the next.
fn assert_in_order(out: &str, wants: &[String]) {
    let mut at = 0;
    for want in wants {
        let i = out[at..]
            .find(want.as_str())
            .unwrap_or_else(|| panic!("no {want:?} after byte {at} of:\n{out}"));
        at += i + want.strip_suffix('\n').unwrap_or(want).len();
    }
}

/// The port `list --json` has for `sub` in project `p`.
fn port_in_state(home: &Path, sub: &str) -> u16 {
    services(&list(home), "p")
        .into_iter()
        .find(|s| s.0 == sub)
        .unwrap_or_else(|| panic!("no service {sub}"))
        .1 as u16
}

/// Holds `127.0.0.1:<port>` without listening on it, as something that
/// takes a port after lokal checked it may: a connect finds nothing, so
/// neither the busy check nor the ready probe sees it, but a server's bind
/// fails with `Address already in use`, and lsof names this test, outside
/// the service's process group, as its owner (VERIFY V11).
fn hold(port: u16) -> tokio::net::TcpSocket {
    let holder = tokio::net::TcpSocket::new_v4().unwrap();
    holder
        .bind(std::net::SocketAddr::from(([127, 0, 0, 1], port)))
        .unwrap();
    holder
}

/// The start of a `run` that waits until the test creates `go` in its
/// directory ([`hold_then_open_gate`]).
const GATE: &str = "while [ ! -e go ]; do sleep 0.01; done; ";

/// Once `up` printed its routes, holds the port of `fqdn` ([`hold`]) and
/// lets a run that starts with [`GATE`] go on. Returns the port and the
/// holder.
fn hold_then_open_gate(up: &UpRun, dir: &Path, fqdn: &str) -> (u16, tokio::net::TcpSocket) {
    // `routes ready in …` or `routes saved` ends the route lines.
    up.wait_out("\n  routes ", Duration::from_secs(10));
    let port = routed_port(&up.out.text(), fqdn);
    let holder = hold(port);
    std::fs::write(dir.join("go"), "").unwrap();
    (port, holder)
}

/// The hint for a service that says a port is in use while its own port is
/// free (§4.11).
fn port_in_use_hint(sub: &str, port: u16) -> String {
    format!(
        "hint: {sub} says a port is in use, but its own port {port} is free. Another port it opens may be \
shared with another service, such as a debugger port (9229): give each server its own. See \
https://mehmetkose.github.io/lokal/docs/guides/frameworks#several-node-or-workers-servers-debugger-ports"
    )
}

/// The wrong-port hint: a server that ignores the port lokal picked (no
/// `{port}`, no `port`) gets one hint, once two lookups (at 3s and 5s after
/// the start) agree, naming its own port and the two fixes; one that listens
/// on nothing yet (still compiling) gets none; and one whose `run` already
/// has `{port}` gets only the `port =` fix. The hint is lokal's own line:
/// stdout, and the service's log.
#[test]
fn a_server_on_another_port_gets_one_hint_and_one_listening_on_nothing_none() {
    let s = Sandbox::new();
    let echo = quoted(&testkit_bin("echo-server"));
    let tag = unique_tag();
    let (pa, pc) = two_free_ports();
    let run_a = format!("{echo} --port {pa} --marker A-{tag}");
    let dir = s.repo(
        "shop",
        &format!(
            "project = \"p\"\n[services]\na = \"{run_a}\"\nb = \"sleep 30.{tag}\"\nc = \"{echo} --port {pc} --marker C-{tag} # {{port}}\"\n"
        ),
    );
    let t = Instant::now();
    let mut up = UpRun::start(s.home.path(), &dir, &["up", "--yes"], &[&tag]);
    up.wait_out("hint: a listens on", Duration::from_secs(8));
    assert!(
        t.elapsed() >= Duration::from_secs(5),
        "the hint waits for the second lookup, at 5s"
    );
    up.wait_out("hint: c listens on", Duration::from_secs(8));
    // Long enough for b's lookups at 3s, 5s and 7s, and for a second hint for
    // a or c, if there were one.
    std::thread::sleep((t + Duration::from_millis(7600)).saturating_duration_since(Instant::now()));
    let (out, err) = (up.out.text(), up.err.text());
    let (port_a, port_c) = (
        routed_port(&out, "a.p.lokal"),
        routed_port(&out, "c.p.lokal"),
    );
    // The lookups run side by side: in either order.
    let mut hints: Vec<&str> = out.lines().filter(|l| l.starts_with("hint: ")).collect();
    hints.sort_unstable();
    let hint_a = format!(
        "hint: a listens on localhost:{pa}, not {port_a}. Set port = {pa} for a in lokal.toml, or put {{port}} in its run: {run_a} --port {{port}}"
    );
    let hint_c = format!(
        "hint: c listens on localhost:{pc}, not {port_c}. Set port = {pc} for c in lokal.toml"
    );
    assert_eq!(hints, [hint_a.as_str(), hint_c.as_str()], "{}", up.both());
    assert!(!err.contains("hint:"), "{err}");
    // Never ready: nothing listens on the routed ports.
    assert!(!out.contains("✓ listening:"), "{out}");
    let log_a = log_lines(&up_file(&s, "p", "a.log"));
    assert!(
        log_a.contains(&format!("lokal: {}", &hint_a[..])),
        "{log_a:?}"
    );
    let log_b = log_lines(&up_file(&s, "p", "b.log"));
    assert!(!log_b.iter().any(|l| l.contains("hint")), "{log_b:?}");
    let shells = up.shells();
    up.signal(libc::SIGINT);
    assert_eq!(up.wait_exit(Duration::from_secs(15)), 130, "{}", up.both());
    assert_all_gone(&shells, &tag);
}

/// Regression: a server that opens a helper listener before its own port
/// (Cloudflare's Vite plugin starts Miniflare and its inspector first) got
/// the hint for the helper's port at the 3s lookup, a moment before it was
/// ready on its own. The hint waits for two lookups that agree: here the
/// helper listens from the start, and the server's own port from 3.5s,
/// between the lookups at 3s and 5s. Once it's ready, no lookup can hint any
/// more, so its ready line is the moment to check. The helper is a
/// background job, which ignores SIGINT until the SIGTERM at 5s.
#[test]
fn a_helper_listener_before_the_servers_own_port_gets_no_hint() {
    let s = Sandbox::new();
    let echo = quoted(&testkit_bin("echo-server"));
    let tag = unique_tag();
    let dir = s.repo(
        "shop",
        &format!(
            "project = \"p\"\n[services]\na = \"{echo} --port 0 --marker H-{tag} & sleep 3.5; exec {echo} --port {{port}} --marker A-{tag}\"\n"
        ),
    );
    let mut up = UpRun::start(s.home.path(), &dir, &["up", "--yes"], &[&tag]);
    up.wait_out(
        "✓ listening: 1/1 services on localhost\n",
        Duration::from_secs(10),
    );
    let out = up.out.text();
    let port = routed_port(&out, "a.p.lokal");
    // Both listened: the helper, then the server on its port.
    assert_in_order(
        &out,
        &[
            "a | LISTENING 127.0.0.1:".into(),
            format!("a | LISTENING 127.0.0.1:{port}\n"),
        ],
    );
    assert!(!out.contains("hint:"), "{}", up.both());
    assert!(!up.err.text().contains("hint:"), "{}", up.both());
    let log = log_lines(&up_file(&s, "p", "a.log"));
    assert!(!log.iter().any(|l| l.contains("hint")), "{log:?}");
    let shells = up.shells();
    up.signal(libc::SIGINT);
    assert_eq!(up.wait_exit(Duration::from_secs(15)), 130, "{}", up.both());
    assert_all_gone(&shells, &tag);
}

/// A picked port that's taken: something holds it once lokal has picked it
/// (here the test, without listening, so the ready probe doesn't take it
/// for the server). The server says EADDRINUSE and exits 1 before it's
/// ready, lokal finds its own port held outside its process group, so it
/// moves the service to a new port (one state write and a reload that
/// waits), says so after the exit line, and starts it again there. The
/// ready line counts it once, and its URL works on the first request.
#[test]
fn a_picked_port_thats_taken_is_retried_on_a_new_port() {
    let d = Daemon::start("none");
    let repos = tempfile::Builder::new().prefix("lkr").tempdir().unwrap();
    let echo = quoted(&testkit_bin("echo-server"));
    let tag = unique_tag();
    let run = format!(
        "{GATE}if [ ! -e first ]; then touch first; echo 'Error: listen EADDRINUSE: address already in use'; exit 1; fi; exec {echo} --port {{port}} --marker R-{tag}"
    );
    let dir = daemon_repo(
        repos.path(),
        "shop",
        &format!("project = \"p\"\n[services]\na = \"{run}\"\n"),
    );
    let mut up = UpRun::start(d.dir.path(), &dir, &["up", "--yes"], &[&tag]);
    let (old, holder) = hold_then_open_gate(&up, &dir, "a.p.lokal");
    up.wait_out(
        "✓ ready: 1/1 services accepting connections (",
        Duration::from_secs(10),
    );
    let out = up.out.text();
    let new = port_in_state(d.dir.path(), "a");
    assert_ne!(new, old, "state.toml has the new port");
    assert_in_order(
        &out,
        &[
            "\na | Error: listen EADDRINUSE: address already in use\n".into(),
            "\na exited 1\n".into(),
            format!("\n  note: a's port {old} was taken; retrying on {new}\n"),
            "\n✓ ready: 1/1".into(),
        ],
    );
    assert_eq!(out.matches("note: a's port").count(), 1, "{out}");
    assert!(!out.contains("hint:"), "{out}");
    // The route went to the new port before the command started again.
    let r = https_get(&d, "https://a.p.lokal/").unwrap();
    assert_eq!(r.status, 200);
    assert_eq!(r.json().unwrap()["marker"], format!("R-{tag}").as_str());
    up.wait_out(
        &format!("a | LISTENING 127.0.0.1:{new}\n"),
        Duration::from_secs(5),
    );
    let shown = |port: u16| run.replace("{port}", &port.to_string());
    assert_eq!(
        log_lines(&d.dir.path().join("logs/up/p/a.log")),
        [
            format!("lokal: started on port {old}: {}", shown(old)),
            "Error: listen EADDRINUSE: address already in use".into(),
            "lokal: exited 1".into(),
            format!("lokal: port {old} was taken; retrying on {new}"),
            format!("lokal: started on port {new}: {}", shown(new)),
            format!("LISTENING 127.0.0.1:{new}"),
        ]
    );
    drop(holder);
    let shells = up.shells();
    up.signal(libc::SIGINT);
    assert_eq!(up.wait_exit(Duration::from_secs(15)), 130, "{}", up.both());
    assert!(
        up.out.text().contains("stopping 1 service…"),
        "{}",
        up.both()
    );
    assert_all_gone(&shells, &tag);
}

/// Once per service: a server that always says a port is in use is retried
/// once, while something else holds its first port, then reported as a
/// normal exit, with the hint, since its new port is free. A fixed `port`
/// is never moved. The match ignores case, and either stream counts.
#[test]
fn a_retry_happens_once_and_never_for_a_fixed_port() {
    let s = Sandbox::new();
    let fixed = free_port();
    let x_run = format!("{GATE}echo 'OSError: [Errno 48] Address already in use' >&2; exit 1");
    let dir = s.repo(
        "shop",
        &format!(
            "project = \"p\"\n[services]\nx = \"{x_run}\"\ny = {{ run = \"echo 'Error: listen EADDRINUSE: address already in use'; exit 1\", port = {fixed} }}\n"
        ),
    );
    let mut up = UpRun::start(s.home.path(), &dir, &["up", "--yes"], &[]);
    let (old, holder) = hold_then_open_gate(&up, &dir, "x.p.lokal");
    assert_eq!(up.wait_exit(Duration::from_secs(10)), 1, "{}", up.both());
    drop(holder);
    let (out, err) = (up.out.text(), up.err.text());
    let new = port_in_state(s.home.path(), "x");
    assert_ne!(new, old);
    assert_in_order(
        &out,
        &[
            "\nx exited 1\n".into(),
            format!("\n  note: x's port {old} was taken; retrying on {new}\n"),
            "\nx exited 1\n".into(),
            format!("\n{}\n", port_in_use_hint("x", new)),
        ],
    );
    assert_eq!(out.matches("note: x's port").count(), 1, "{out}");
    assert_eq!(out.matches("\nx exited 1\n").count(), 2, "{out}");
    assert_eq!(out.matches("hint: x says").count(), 1, "{out}");
    assert_eq!(err.matches("x | OSError").count(), 2, "{err}");
    // The fixed port: one exit, no note, the hint, and the port stays.
    assert_eq!(out.matches("\ny exited 1\n").count(), 1, "{out}");
    assert!(!out.contains("y's port"), "{out}");
    assert_in_order(
        &out,
        &[
            "\ny exited 1\n".into(),
            format!("\n{}\n", port_in_use_hint("y", fixed)),
        ],
    );
    assert_eq!(port_in_state(s.home.path(), "y"), fixed);
    // Both services counted once, after every exit and hint, x by its
    // second run.
    assert!(
        out.ends_with(
            "✓ listening: 0/2 services on localhost; not ready: x (exited 1), y (exited 1)\n"
        ),
        "{out}"
    );
    let shown = |port: u16| format!("lokal: started on port {port}: {x_run}");
    assert_eq!(
        log_lines(&up_file(&s, "p", "x.log")),
        [
            shown(old),
            "OSError: [Errno 48] Address already in use".into(),
            "lokal: exited 1".into(),
            format!("lokal: port {old} was taken; retrying on {new}"),
            shown(new),
            "OSError: [Errno 48] Address already in use".into(),
            "lokal: exited 1".into(),
            format!("lokal: {}", port_in_use_hint("x", new)),
        ]
    );
}

/// The misattribution this replaces: workerd says its inspector's port is in
/// use, and lokal moved the service, whose own port was free. Now a service
/// whose own port is free isn't retried: after its exit line it gets the
/// hint, which points at debugger ports. A fixed `port` gets the same. The
/// hint comes before the ready line, which counts each service once.
#[test]
fn a_server_that_says_another_port_is_in_use_gets_a_hint_not_a_retry() {
    let s = Sandbox::new();
    let fixed = free_port();
    let line = "workerd/server/server.c++:1882: bind(): Address already in use; port 9229";
    let run = format!("echo '{line}' >&2; exit 1");
    let dir = s.repo(
        "shop",
        &format!(
            "project = \"p\"\n[services]\nw = \"{run}\"\nf = {{ run = \"{run}\", port = {fixed} }}\n"
        ),
    );
    let o = s.run(&dir, &["up", "--yes"]);
    assert_eq!(code(&o), 1, "{}{}", stdout(&o), stderr(&o));
    let (out, err) = (stdout(&o), stderr(&o));
    let port = routed_port(&out, "w.p.lokal");
    assert_eq!(port_in_state(s.home.path(), "w"), port, "not moved");
    assert_eq!(port_in_state(s.home.path(), "f"), fixed);
    assert!(!out.contains("was taken"), "{out}");
    for (sub, port) in [("w", port), ("f", fixed)] {
        assert_eq!(
            out.matches(&format!("\n{sub} exited 1\n")).count(),
            1,
            "{out}"
        );
        assert_in_order(
            &out,
            &[
                format!("\n{sub} exited 1\n"),
                format!("\n{}\n", port_in_use_hint(sub, port)),
            ],
        );
        assert_eq!(err.matches(&format!("{sub} | workerd")).count(), 1, "{err}");
        assert_eq!(
            log_lines(&up_file(&s, "p", &format!("{sub}.log"))),
            [
                format!("lokal: started on port {port}: {run}"),
                line.to_string(),
                "lokal: exited 1".into(),
                format!("lokal: {}", port_in_use_hint(sub, port)),
            ]
        );
    }
    assert!(!err.contains("hint:"), "{err}");
    assert!(
        out.ends_with(
            "✓ listening: 0/2 services on localhost; not ready: f (exited 1), w (exited 1)\n"
        ),
        "{out}"
    );
}

/// The first run of a retried service (its port held, as in
/// [`a_picked_port_thats_taken_is_retried_on_a_new_port`]) can leave a
/// background job in its process group. lokal still waits for that group to empty, and stops it
/// with the rest: here a `sleep` that ignores SIGINT, until the SIGTERM at
/// 5s. "Stopping 1 service" counts the service once.
#[test]
fn a_retried_services_first_process_group_is_still_stopped() {
    let s = Sandbox::new();
    let echo = quoted(&testkit_bin("echo-server"));
    let tag = unique_tag();
    let sleep = format!("sleep 60.{tag}");
    let dir = s.repo(
        "shop",
        &format!(
            "project = \"p\"\n[services]\na = \"{GATE}if [ ! -e first ]; then touch first; {sleep} & echo 'listen EADDRINUSE'; exit 1; fi; exec {echo} --port {{port}} --marker R-{tag}\"\n"
        ),
    );
    let mut up = UpRun::start(s.home.path(), &dir, &["up", "--yes"], &[&tag]);
    let (_, holder) = hold_then_open_gate(&up, &dir, "a.p.lokal");
    up.wait_out(
        "✓ listening: 1/1 services on localhost\n",
        Duration::from_secs(10),
    );
    assert!(up.out.text().contains("  note: a's port "), "{}", up.both());
    drop(holder);
    // The first run's job is still there, in the first run's group.
    wait_for_process(&tag, &sleep);
    let shells = up.shells();
    let t = Instant::now();
    up.signal(libc::SIGINT);
    assert_eq!(up.wait_exit(Duration::from_secs(15)), 130, "{}", up.both());
    let took = t.elapsed();
    assert!(
        up.out.text().contains("stopping 1 service…"),
        "{}",
        up.both()
    );
    assert!(took >= Duration::from_secs(5), "exited after {took:?}");
    assert!(took < Duration::from_secs(8), "exited after {took:?}");
    assert_all_gone(&shells, &tag);
}

/// The real thing, for a kept port (no `port` in the file, so it may move):
/// something holds the port without listening on it, so the busy check
/// before the start finds it free, and the server fails to bind it with its
/// own `Address already in use`. lsof shows the holder, outside the
/// service's process group, so the port counts as taken.
#[test]
fn a_kept_port_that_a_server_cant_bind_moves_too() {
    let s = Sandbox::new();
    let echo = quoted(&testkit_bin("echo-server"));
    let tag = unique_tag();
    let dir = s.repo(
        "shop",
        &format!(
            "project = \"p\"\n[services]\na = \"exec {echo} --port {{port}} --marker K-{tag}\"\nb = \"true\"\n"
        ),
    );
    // Routes both, starts only b: a gets its port, to keep.
    s.ok(&dir, &["up", "--yes", "b"]);
    let kept = port_in_state(s.home.path(), "a");
    let holder = hold(kept);
    let mut up = UpRun::start(s.home.path(), &dir, &["up", "--yes", "a"], &[&tag]);
    up.wait_out(
        "✓ listening: 1/1 services on localhost\n",
        Duration::from_secs(10),
    );
    let out = up.out.text();
    assert_eq!(routed_port(&out, "a.p.lokal"), kept);
    let new = port_in_state(s.home.path(), "a");
    assert_ne!(new, kept);
    assert!(
        up.err.text().contains("a | Error: Address already in use"),
        "{}",
        up.both()
    );
    assert!(
        out.contains(&format!(
            "\n  note: a's port {kept} was taken; retrying on {new}\n"
        )),
        "{out}"
    );
    up.wait_out(
        &format!("a | LISTENING 127.0.0.1:{new}\n"),
        Duration::from_secs(5),
    );
    drop(holder);
    let shells = up.shells();
    up.signal(libc::SIGINT);
    assert_eq!(up.wait_exit(Duration::from_secs(15)), 130, "{}", up.both());
    assert_all_gone(&shells, &tag);
}

// ─────── servers that detach (SPEC §4.11, "A server that detaches") ───────

/// The pid in a `<sub> | DETACHED <pid> …` line: the server that the
/// testkit echo-server's `--detach` started in a new session.
fn detached_pid(out: &str, sub: &str) -> i32 {
    let start = format!("{sub} | DETACHED ");
    out.lines()
        .find_map(|l| l.strip_prefix(start.as_str()))
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|p| p.parse().ok())
        .unwrap_or_else(|| panic!("no DETACHED line for {sub} in:\n{out}"))
}

/// Stops a server that this test's command left running outside `lokal
/// up`, by the pid lokal named for it, once that pid still runs with the
/// test's `tag` in its command line, and waits up to 5s until it's gone
/// (launchd, its parent now, reaps it).
fn kill_left(pid: i32, tag: &str) {
    assert!(
        tagged(&[tag.to_string()]).iter().any(|p| p.0 == pid),
        "pid {pid} isn't this test's server"
    );
    // SAFETY: kill only sends a signal to a server this test started.
    unsafe { libc::kill(pid, libc::SIGKILL) };
    let deadline = Instant::now() + Duration::from_secs(5);
    // SAFETY: signal 0 only checks whether the pid exists.
    while unsafe { libc::kill(pid, 0) } == 0 {
        assert!(Instant::now() < deadline, "pid {pid} is still there");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// `exited 0, and left …`: the exit line of a service whose command left
/// one echo-server on its port, after `<sub> `.
fn left_line(sub: &str, port: u16, pid: i32) -> String {
    format!(
        "exited 0, and left localhost:{port} to echo-server (pid {pid}), which runs outside lokal up: \
lokal logs {sub} won't show its output, and stopping lokal up won't stop it."
    )
}

fn detached_hint(sub: &str) -> String {
    format!(
        "hint: {sub}'s run must keep its server in the foreground. See \
https://mehmetkose.github.io/lokal/docs/guides/frameworks#servers-that-detach-themselves"
    )
}

/// Two commands that start their server in a new session (`setsid`) and
/// exit 0, as `astro dev` does under a coding agent: `fast` once its server
/// listens, so the ready probe saw the port first, and `slow` at once, half
/// a second before its server binds. Before, the ready line counted only
/// the one whose port answered before its exit was handled. Now each exit
/// line names the server it left and the hint follows; the ready line
/// counts both once, as detached; and lokal exits 0 once both commands
/// have, naming both servers last. The next `up` finds their ports taken
/// and names them as left by the last one, for a fixed and a kept port.
/// The test stops them by the pids lokal named; then an `up` that starts
/// both services again drops them from the record.
#[test]
fn servers_that_detach_are_named_counted_once_and_named_by_the_next_up() {
    let s = Sandbox::new();
    let echo = quoted(&testkit_bin("echo-server"));
    let tag = unique_tag();
    let fixed = free_port();
    let dir = s.repo(
        "shop",
        &format!(
            "project = \"p\"\n[services]\nfast = {{ run = \"{echo} --port {{port}} --marker F-{tag} --detach --exit-after-ms 300\", port = {fixed} }}\nslow = \"{echo} --port {{port}} --marker S-{tag} --detach --bind-after-ms 500\"\n"
        ),
    );
    let mut up = UpRun::start(s.home.path(), &dir, &["up", "--yes"], &[&tag]);
    assert_eq!(up.wait_exit(Duration::from_secs(15)), 0, "{}", up.both());
    let out = up.out.text();
    let (pf, ps) = (detached_pid(&out, "fast"), detached_pid(&out, "slow"));
    assert_eq!(routed_port(&out, "fast.p.lokal"), fixed);
    let port_s = routed_port(&out, "slow.p.lokal");
    let ready = "\n✓ listening: 2/2 services on localhost; detached: fast, slow\n";
    for (sub, port, pid) in [("fast", fixed, pf), ("slow", port_s, ps)] {
        assert_in_order(
            &out,
            &[
                format!("\n{sub} {}\n", left_line(sub, port, pid)),
                format!("\n{}\n", detached_hint(sub)),
                ready.into(),
            ],
        );
        assert_eq!(out.matches(&format!("\n{sub} exited")).count(), 1, "{out}");
        let log = log_lines(&up_file(&s, "p", &format!("{sub}.log")));
        assert_eq!(
            log[log.len() - 2..],
            [
                format!("lokal: {}", left_line(sub, port, pid)),
                format!("lokal: {}", detached_hint(sub)),
            ],
            "{log:?}"
        );
    }
    assert_eq!(out.matches("✓ listening:").count(), 1, "{out}");
    assert!(
        out.ends_with(&format!(
            "{ready}2 servers keep running outside lokal up: fast (echo-server, pid {pf}), slow (echo-server, pid {ps})\n"
        )),
        "{out}"
    );
    assert_eq!(mode(&up_file(&s, "p", "detached.json")), 0o600);

    // The next up: a fixed port, then a kept one. lsof's short name.
    let next = |args: &[&str], sub: &str, port: u16, pid: i32| {
        let o = s.run(&dir, args);
        assert_eq!(code(&o), 1, "{}{}", stdout(&o), stderr(&o));
        assert!(
            stderr(&o).ends_with(&format!(
                "error: localhost:{port} is already in use by echo-serv (pid {pid}), which {sub} left running at \
the last lokal up; not starting {sub}\nhint: stop it: kill {pid}. {sub}'s run must keep its server in the \
foreground. See https://mehmetkose.github.io/lokal/docs/guides/frameworks#servers-that-detach-themselves\n"
            )),
            "{}",
            stderr(&o)
        );
    };
    next(&["up", "--yes"], "fast", fixed, pf);
    next(&["up", "--yes", "slow"], "slow", port_s, ps);
    kill_left(pf, &tag);
    kill_left(ps, &tag);
    assert!(tagged(std::slice::from_ref(&tag)).is_empty());

    // An up that starts both again replaces what they left: nothing now.
    std::fs::write(
        dir.join("lokal.toml"),
        format!("project = \"p\"\n[services]\nfast = {{ run = \"true\", port = {fixed} }}\nslow = \"true\"\n"),
    )
    .unwrap();
    s.ok(&dir, &["up", "--yes"]);
    assert!(!up_file(&s, "p", "detached.json").exists());
}

/// A command that exits 0 without leaving a server (`true`) gets a plain
/// exit line, isn't called detached, and holds the ready line no longer
/// than the 2s lokal watches its port for a server that binds late.
#[test]
fn a_command_that_exits_0_without_a_server_isnt_detached() {
    let s = Sandbox::new();
    let dir = s.repo("shop", "project = \"p\"\n[services]\na = \"true\"\n");
    let mut up = UpRun::start(s.home.path(), &dir, &["up", "--yes"], &[]);
    up.wait_out("\n  routes ", Duration::from_secs(10));
    let t = Instant::now();
    up.wait_out("✓ listening: ", Duration::from_secs(5));
    let took = t.elapsed();
    assert_eq!(up.wait_exit(Duration::from_secs(5)), 0, "{}", up.both());
    assert!(took < Duration::from_millis(2500), "{took:?}");
    let out = up.out.text();
    assert!(
        out.ends_with(
            "\na exited 0\n✓ listening: 0/1 services on localhost; not ready: a (exited 0)\n"
        ),
        "{out}"
    );
    for not in ["left", "detached", "hint:", "keep"] {
        assert!(!out.contains(not), "{not}: {out}");
    }
    assert_eq!(
        log_lines(&up_file(&s, "p", "a.log"))[1..],
        ["lokal: exited 0"]
    );
    assert!(!up_file(&s, "p", "detached.json").exists());
}

/// A server that a background job holds in the service's own process group
/// isn't detached: lokal still follows it, so it counts as ready, and
/// Ctrl-C stops it with its group (at the SIGTERM at 5s: a background job
/// ignores SIGINT).
#[test]
fn a_server_left_in_its_own_group_isnt_detached_and_is_stopped() {
    let s = Sandbox::new();
    let echo = quoted(&testkit_bin("echo-server"));
    let tag = unique_tag();
    let dir = s.repo(
        "shop",
        &format!(
            "project = \"p\"\n[services]\na = \"{echo} --port {{port}} --marker B-{tag} & exit 0\"\n"
        ),
    );
    let mut up = UpRun::start(s.home.path(), &dir, &["up", "--yes"], &[&tag]);
    up.wait_out(
        "✓ listening: 1/1 services on localhost\n",
        Duration::from_secs(10),
    );
    let out = up.out.text();
    assert_in_order(
        &out,
        &[
            "\na exited 0\n".into(),
            "\n✓ listening: 1/1 services on localhost\n".into(),
        ],
    );
    for not in ["left", "detached", "hint:"] {
        assert!(!out.contains(not), "{not}: {out}");
    }
    // The shell is gone; its group lives on in the job.
    let found = tagged(std::slice::from_ref(&tag));
    assert_eq!(found.len(), 1, "{found:?}");
    let group = found[0].1;
    let t = Instant::now();
    up.signal(libc::SIGINT);
    assert_eq!(up.wait_exit(Duration::from_secs(15)), 130, "{}", up.both());
    let took = t.elapsed();
    let out = up.out.text();
    assert!(out.contains("stopping 1 service…"), "{out}");
    assert!(!out.contains("keep running"), "{out}");
    assert!(took >= Duration::from_secs(5), "exited after {took:?}");
    assert!(took < Duration::from_secs(8), "exited after {took:?}");
    assert_all_gone(&[group], &tag);
    assert!(!up_file(&s, "p", "detached.json").exists());
}

// ─────── restarting a service (SPEC §4.11, "Restarting a service") ───────

/// `lokal args…` started in the background, its output collected, for a
/// `lokal restart` that waits while the test does something else.
fn spawn_lokal(home: &Path, cwd: &Path, args: &[&str]) -> std::thread::JoinHandle<Output> {
    let mut cmd = lokal_cmd(home, cwd, args);
    std::thread::spawn(move || cmd.output().unwrap())
}

/// The lines of a log that start a run.
fn starts_in(log: &[String]) -> Vec<&String> {
    log.iter()
        .filter(|l| l.starts_with("lokal: started on port "))
        .collect()
}

/// The common case, with the sandbox daemon: `lokal restart a` returns
/// once a's new run accepts connections, and a's URL answers the first
/// request after it. Only a restarts: b's server is the same process. `up`
/// logs and prints the restart, keeps running, and stops both on Ctrl-C;
/// its socket is 0600 while it runs, and gone after.
#[test]
fn lokal_restart_restarts_one_service_whose_url_works_when_it_returns() {
    let d = Daemon::start("none");
    let repos = tempfile::Builder::new().prefix("lkr").tempdir().unwrap();
    let echo = quoted(&testkit_bin("echo-server"));
    let tag = unique_tag();
    let dir = daemon_repo(
        repos.path(),
        "shop",
        &format!(
            "project = \"p\"\n[services]\na = \"exec {echo} --port {{port}} --marker A-{tag}\"\nb = \"exec {echo} --port {{port}} --marker B-{tag}\"\n"
        ),
    );
    let mut up = UpRun::start(d.dir.path(), &dir, &["up", "--yes"], &[&tag]);
    up.wait_out(
        "✓ ready: 2/2 services accepting connections (",
        Duration::from_secs(10),
    );
    let sock = d.dir.path().join("logs/up/p/up.sock");
    assert_eq!(mode(&sock), 0o600);
    assert_eq!(mode(sock.parent().unwrap()), 0o700);
    let (a_before, b_before) = (tagged(&[format!("A-{tag}")]), tagged(&[format!("B-{tag}")]));
    assert_eq!((a_before.len(), b_before.len()), (1, 1));
    let o = lokal(d.dir.path(), &dir, &["restart", "a"]);
    assert_eq!(code(&o), 0, "{}{}", stdout(&o), stderr(&o));
    assert!(
        stdout(&o).starts_with("✓ restarted https://a.p.lokal ("),
        "{}",
        stdout(&o)
    );
    assert_eq!(stdout(&o).lines().count(), 1, "{}", stdout(&o));
    assert_eq!(stderr(&o), "");
    // The first request after it returns, no retry.
    let r = https_get(&d, "https://a.p.lokal/").unwrap();
    assert_eq!(r.status, 200);
    assert_eq!(r.json().unwrap()["marker"], format!("A-{tag}").as_str());
    let a_after = tagged(&[format!("A-{tag}")]);
    assert_eq!(a_after.len(), 1, "{a_after:?}");
    assert_ne!(a_after[0].0, a_before[0].0, "a is a new process");
    assert_eq!(tagged(&[format!("B-{tag}")]), b_before, "b wasn't touched");
    let log = log_lines(&d.dir.path().join("logs/up/p/a.log"));
    assert_eq!(starts_in(&log).len(), 2, "{log:?}");
    let port = routed_port(&up.out.text(), "a.p.lokal");
    assert_in_order(
        &log.join("\n"),
        &[
            format!("lokal: started on port {port}: exec "),
            "lokal: restarting (lokal restart)\nlokal: stopping (SIGINT)\nlokal: exited 130\n"
                .into(),
            format!("lokal: started on port {port}: exec "),
            format!("LISTENING 127.0.0.1:{port}"),
        ],
    );
    up.wait_out("\na ready (", Duration::from_secs(5));
    assert_in_order(
        &up.out.text(),
        &[
            "\n✓ ready: 2/2".into(),
            "\nrestarting a\n".into(),
            "\na ready (".into(),
        ],
    );
    // The old run's exit was lokal's doing: the log has it, not the terminal.
    assert!(!up.out.text().contains("a exited"), "{}", up.both());
    assert!(up.child.try_wait().unwrap().is_none(), "up keeps running");
    let shells = up.shells();
    up.signal(libc::SIGINT);
    assert_eq!(up.wait_exit(Duration::from_secs(15)), 130, "{}", up.both());
    assert!(
        up.out.text().contains("stopping 2 services…"),
        "{}",
        up.both()
    );
    assert_all_gone(&shells, &tag);
    assert!(!sock.exists(), "the socket goes with up");
}

/// A new run that exits before it's ready: exit 1, its exit, the lines of
/// that run only, and where to read more. The service has exited, and the
/// next `lokal restart` starts it again.
#[test]
fn a_restart_that_fails_shows_that_runs_lines() {
    let s = Sandbox::new();
    let echo = quoted(&testkit_bin("echo-server"));
    let tag = unique_tag();
    let dir = s.repo(
        "shop",
        &format!(
            "project = \"p\"\n[services]\na = \"test -f fail && {{ echo boom-{tag}; exit 1; }}; exec {echo} --port {{port}} --marker A-{tag}\"\nb = \"exec {echo} --port {{port}} --marker B-{tag}\"\n"
        ),
    );
    let mut up = UpRun::start(s.home.path(), &dir, &["up", "--yes"], &[&tag]);
    up.wait_out("✓ listening: 2/2", Duration::from_secs(10));
    let port = routed_port(&up.out.text(), "a.p.lokal");
    std::fs::write(dir.join("fail"), "").unwrap();
    let o = s.run(&dir, &["restart", "a"]);
    assert_eq!(code(&o), 1, "{}{}", stdout(&o), stderr(&o));
    assert_eq!(stdout(&o), "");
    let err = stderr(&o);
    let lines: Vec<&str> = err.lines().collect();
    assert_eq!(lines.len(), 5, "{err}");
    assert_eq!(lines[0], "error: a exited 1 before it was ready");
    // This run's lines only: not the first run's `LISTENING`.
    let shown: Vec<&str> = lines[1..4].iter().map(|l| after_stamp(l)).collect();
    assert!(
        shown[0].starts_with(&format!("lokal: started on port {port}: test -f fail")),
        "{err}"
    );
    assert_eq!(shown[1..], [&format!("boom-{tag}"), "lokal: exited 1"]);
    assert_eq!(lines[4], "hint: lokal logs a -n 50");
    // up says so too, and keeps running b.
    up.wait_out("\na exited 1\n", Duration::from_secs(5));
    assert!(up.child.try_wait().unwrap().is_none());
    // An exited service starts again.
    std::fs::remove_file(dir.join("fail")).unwrap();
    let o = s.run(&dir, &["restart", "a", "-p", "p"]);
    assert_eq!(code(&o), 0, "{}{}", stdout(&o), stderr(&o));
    assert!(
        stdout(&o).starts_with(&format!("✓ restarted a, listening on localhost:{port} (")),
        "{}",
        stdout(&o)
    );
    up.signal(libc::SIGINT);
    assert_eq!(up.wait_exit(Duration::from_secs(15)), 130, "{}", up.both());
}

/// Without a running `up` there is nothing to ask: exit 1, and a hint
/// that never offers `--yes` (§16.11). A service that isn't in the file is
/// a usage error, and nothing restarts. Without SUB, `lokal restart` is the
/// background service's, as before (refused in a sandbox), and `-p` needs
/// a SUB.
#[test]
fn lokal_restart_needs_a_running_up_and_a_service_in_its_file() {
    let s = Sandbox::new();
    let echo = quoted(&testkit_bin("echo-server"));
    let tag = unique_tag();
    let dir = s.repo(
        "shop",
        &format!(
            "project = \"p\"\n[services]\na = \"exec {echo} --port {{port}} --marker A-{tag}\"\nb = \"exec {echo} --port {{port}} --marker B-{tag}\"\n"
        ),
    );
    let none = |o: &Output| {
        assert_eq!(code(o), 1, "{}{}", stdout(o), stderr(o));
        assert_eq!(
            stderr(o),
            "error: no lokal up is running for p\nhint: start it: lokal up\n"
        );
    };
    none(&s.run(&dir, &["restart", "a"]));
    let o = s.run(&dir, &["restart"]);
    assert_eq!(code(&o), 1, "{}", stderr(&o));
    assert_eq!(
        stderr(&o),
        "error: managing the background service is disabled under LOKAL_HOME (sandbox)\n"
    );
    let o = s.run(&dir, &["restart", "-p", "p"]);
    assert_eq!(code(&o), 2, "{}", stderr(&o));
    let mut up = UpRun::start(s.home.path(), &dir, &["up", "--yes"], &[&tag]);
    up.wait_out("✓ listening: 2/2", Duration::from_secs(10));
    for args in [&["restart", "nope"][..], &["restart", "a", "nope"]] {
        let o = s.run(&dir, args);
        assert_eq!(code(&o), 2, "{}{}", stdout(&o), stderr(&o));
        assert_eq!(
            stderr(&o),
            format!(
                "error: no service `nope` in {}\nhint: its services: a, b\n",
                dir.join("lokal.toml").display()
            )
        );
    }
    let o = s.run(&dir, &["restart", "a.b.c!"]);
    assert_eq!(code(&o), 2, "{}", stderr(&o));
    assert_eq!(starts_in(&log_lines(&up_file(&s, "p", "a.log"))).len(), 1);
    up.signal(libc::SIGINT);
    assert_eq!(up.wait_exit(Duration::from_secs(15)), 130, "{}", up.both());
    none(&s.run(&dir, &["restart", "a"]));
}

/// `lokal up a` routes b and doesn't start it; `lokal restart b` does, with
/// a log of its own, and returns once it's ready. `up` then stops both.
#[test]
fn lokal_restart_starts_a_service_that_up_didnt() {
    let s = Sandbox::new();
    let echo = quoted(&testkit_bin("echo-server"));
    let tag = unique_tag();
    let dir = s.repo(
        "shop",
        &format!(
            "project = \"p\"\n[services]\na = \"exec {echo} --port {{port}} --marker A-{tag}\"\nbee = \"exec {echo} --port {{port}} --marker B-{tag}\"\n"
        ),
    );
    let mut up = UpRun::start(s.home.path(), &dir, &["up", "--yes", "a"], &[&tag]);
    up.wait_out("✓ listening: 1/1", Duration::from_secs(10));
    let port = routed_port(&up.out.text(), "bee.p.lokal");
    assert!(tagged(&[format!("B-{tag}")]).is_empty());
    let o = s.run(&dir, &["restart", "bee"]);
    assert_eq!(code(&o), 0, "{}{}", stdout(&o), stderr(&o));
    assert!(
        stdout(&o).starts_with(&format!("✓ restarted bee, listening on localhost:{port} (")),
        "{}",
        stdout(&o)
    );
    assert_eq!(tagged(&[format!("B-{tag}")]).len(), 1);
    up.wait_out("\nbee ready (", Duration::from_secs(5));
    assert_in_order(
        &up.out.text(),
        &[
            "\nstarting bee\n".into(),
            format!("\nbee | LISTENING 127.0.0.1:{port}\n"),
            "\nbee ready (".into(),
        ],
    );
    let log = log_lines(&up_file(&s, "p", "bee.log"));
    assert_eq!(log[0], "lokal: starting (lokal restart)");
    assert!(log[1].starts_with(&format!("lokal: started on port {port}: exec ")));
    let shells = up.shells();
    assert_eq!(shells.len(), 2, "{shells:?}");
    up.signal(libc::SIGINT);
    assert_eq!(up.wait_exit(Duration::from_secs(15)), 130, "{}", up.both());
    assert!(
        up.out.text().contains("stopping 2 services…"),
        "{}",
        up.both()
    );
    assert_all_gone(&shells, &tag);
}

/// `up` never reads `lokal.toml` again: after the file changes, a restart
/// runs the command `up` started with, and says how to use the new file.
#[test]
fn a_restart_runs_the_command_up_started_with_after_the_file_changed() {
    let s = Sandbox::new();
    let echo = quoted(&testkit_bin("echo-server"));
    let tag = unique_tag();
    let toml = format!(
        "project = \"p\"\n[services]\na = \"exec {echo} --port {{port}} --marker A-{tag}\"\n"
    );
    let dir = s.repo("shop", &toml);
    let mut up = UpRun::start(s.home.path(), &dir, &["up", "--yes"], &[&tag]);
    up.wait_out("✓ listening: 1/1", Duration::from_secs(10));
    std::fs::write(dir.join("lokal.toml"), toml.replace("A-", "CHANGED-")).unwrap();
    let o = s.run(&dir, &["restart", "a"]);
    assert_eq!(code(&o), 0, "{}{}", stdout(&o), stderr(&o));
    assert!(
        stdout(&o).starts_with(
            "  note: lokal.toml changed since lokal up started; restarted with the commands it started with. \
Stop lokal up and start it again to use the new file.\n✓ restarted a, listening on localhost:"
        ),
        "{}",
        stdout(&o)
    );
    let log = log_lines(&up_file(&s, "p", "a.log"));
    let runs = starts_in(&log);
    assert_eq!(runs.len(), 2, "{log:?}");
    assert!(
        runs[1].ends_with(&format!("--marker A-{tag}")),
        "{}",
        runs[1]
    );
    assert!(tagged(&[format!("CHANGED-{tag}")]).is_empty());
    // The same bytes again: no note.
    std::fs::write(dir.join("lokal.toml"), &toml).unwrap();
    let o = s.ok(&dir, &["restart", "a"]);
    assert!(!stdout(&o).contains("note:"), "{}", stdout(&o));
    up.signal(libc::SIGINT);
    assert_eq!(up.wait_exit(Duration::from_secs(15)), 130, "{}", up.both());
}

/// Two `lokal restart a` at once share one restart: one new run, and both
/// return when it's ready. a ignores SIGINT, so its stop takes until the
/// SIGTERM at 5s. Once `up` stops, it refuses restarts.
#[test]
fn restarts_of_one_service_share_one_run_and_none_starts_while_up_stops() {
    let s = Sandbox::new();
    let echo = quoted(&testkit_bin("echo-server"));
    let tag = unique_tag();
    let dir = s.repo(
        "shop",
        &format!(
            "project = \"p\"\n[services]\na = \"trap '' INT; exec {echo} --port {{port}} --marker A-{tag}\"\nb = \"exec {echo} --port {{port}} --marker B-{tag}\"\n"
        ),
    );
    let mut up = UpRun::start(s.home.path(), &dir, &["up", "--yes"], &[&tag]);
    up.wait_out("✓ listening: 2/2", Duration::from_secs(10));
    let t = Instant::now();
    let first = spawn_lokal(s.home.path(), &dir, &["restart", "a"]);
    up.wait_out("\nrestarting a\n", Duration::from_secs(5));
    let second = s.run(&dir, &["restart", "a"]);
    let first = first.join().unwrap();
    let took = t.elapsed();
    for o in [&first, &second] {
        assert_eq!(code(o), 0, "{}{}", stdout(o), stderr(o));
        assert!(
            stdout(o).starts_with("✓ restarted a, listening on localhost:"),
            "{}",
            stdout(o)
        );
    }
    assert!(took >= Duration::from_secs(5), "{took:?}");
    assert!(took < Duration::from_secs(8), "{took:?}");
    let log = log_lines(&up_file(&s, "p", "a.log"));
    assert_eq!(starts_in(&log).len(), 2, "{log:?}");
    assert_in_order(
        &log.join("\n"),
        &[
            "lokal: restarting (lokal restart)\nlokal: stopping (SIGINT)\nlokal: stopping (SIGTERM)\n".into(),
            "lokal: started on port ".into(),
        ],
    );
    assert_eq!(up.out.text().matches("\nrestarting a\n").count(), 1);
    // Stopping: a ignores SIGINT again, so up stops for 5s.
    up.signal(libc::SIGINT);
    up.wait_out("stopping 2 services…", Duration::from_secs(5));
    let o = s.run(&dir, &["restart", "b"]);
    assert_eq!(code(&o), 1, "{}{}", stdout(&o), stderr(&o));
    assert_eq!(stderr(&o), "error: lokal up is stopping\n");
    assert_eq!(up.wait_exit(Duration::from_secs(15)), 130, "{}", up.both());
}

/// A server that a service's command left running outside `lokal up`
/// keeps the port: a restart finds it busy, names it as left by the
/// service, and starts nothing. Once the server is gone, the restart runs,
/// and its server detaches again: the URL works, and the hint says so.
#[test]
fn a_restart_names_a_server_its_service_left_running() {
    let s = Sandbox::new();
    let echo = quoted(&testkit_bin("echo-server"));
    let tag = unique_tag();
    let dir = s.repo(
        "shop",
        &format!(
            "project = \"p\"\n[services]\nd = \"{echo} --port {{port}} --marker D-{tag} --detach\"\nb = \"exec {echo} --port {{port}} --marker B-{tag}\"\n"
        ),
    );
    let mut up = UpRun::start(s.home.path(), &dir, &["up", "--yes"], &[&tag]);
    up.wait_out(
        "✓ listening: 2/2 services on localhost; detached: d\n",
        Duration::from_secs(10),
    );
    let out = up.out.text();
    let (pid, port) = (detached_pid(&out, "d"), routed_port(&out, "d.p.lokal"));
    let o = s.run(&dir, &["restart", "d"]);
    assert_eq!(code(&o), 1, "{}{}", stdout(&o), stderr(&o));
    assert_eq!(
        stderr(&o),
        format!(
            "error: localhost:{port} is already in use by echo-serv (pid {pid}), which d left running at the \
last lokal up; not starting d\nhint: stop it: kill {pid}. d's run must keep its server in the foreground. \
See https://mehmetkose.github.io/lokal/docs/guides/frameworks#servers-that-detach-themselves\n"
        )
    );
    kill_left(pid, &tag);
    let o = s.run(&dir, &["restart", "d"]);
    assert_eq!(code(&o), 0, "{}{}", stdout(&o), stderr(&o));
    let out = stdout(&o);
    assert!(
        out.starts_with(&format!(
            "{}\n✓ restarted d, listening on localhost:{port} (",
            detached_hint("d")
        )),
        "{out}"
    );
    up.wait_out("\nd ready (", Duration::from_secs(5));
    let pids: Vec<i32> = up
        .out
        .text()
        .lines()
        .filter_map(|l| l.strip_prefix("d | DETACHED "))
        .filter_map(|l| l.split_whitespace().next()?.parse().ok())
        .collect();
    assert_eq!(pids.len(), 2, "{}", up.both());
    kill_left(pids[1], &tag);
    up.signal(libc::SIGINT);
    assert_eq!(up.wait_exit(Duration::from_secs(15)), 130, "{}", up.both());
}

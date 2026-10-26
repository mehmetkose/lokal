//! Live test runner (docs/TESTING.md) against the lokal installed on this Mac.
//!
//! Rules: random project `lt-<6 hex>`, free ports from binding to port 0,
//! readiness checks never poll (the CLI promises the URL works when it
//! returns), only crash-recovery and stop/start checks poll (every 50ms up
//! to their limit). The test project and upstreams are always cleaned up.
//! The live test never passes `--cacert`/`--connect` to `check`.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

const CHROME: &str = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";

#[derive(Clone, Copy, PartialEq, Eq)]
enum St {
    Pass,
    Fail,
    Skip,
}

struct Result_ {
    id: &'static str,
    desc: &'static str,
    status: St,
    ms: u128,
    detail: String,
}

struct Ctx {
    bin_dir: PathBuf,
    lokal: PathBuf,
    p: String,
    a: u16,
    b: u16,
    c: u16,
    d: u16,
    upstreams: Vec<Child>,
    results: Vec<Result_>,
    tmp: PathBuf,
    curl_cacert: bool,
    project_created: bool,
    /// The developer's current project before the run. Creating the test
    /// project makes it current (SPEC §4.4), so cleanup restores this.
    prev_current: Option<String>,
}

/// `current_project` from `lokal list --json`.
fn current_project(list: &Value) -> Option<String> {
    list["current_project"].as_str().map(str::to_string)
}

type Check = Result<String, String>;

fn out_str(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

fn run(cmd: &Path, args: &[&str]) -> Output {
    Command::new(cmd)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap_or_else(|e| Output {
            status: std::process::ExitStatus::default(),
            stdout: vec![],
            stderr: format!("cannot run {}: {e}", cmd.display()).into_bytes(),
        })
}

fn ms(d: Duration) -> f64 {
    (d.as_secs_f64() * 1e5).round() / 100.0
}

impl Ctx {
    /// FQDN in the main (default `.lokal`) project.
    fn fqdn(&self, sub: &str) -> String {
        if sub == "@" {
            format!("{}.lokal", self.p)
        } else {
            format!("{sub}.{}.lokal", self.p)
        }
    }

    /// The second test project lives under `.local` (mDNS path).
    fn mp(&self) -> String {
        format!("{}-m", self.p)
    }

    fn mdns_fqdn(&self) -> String {
        format!("api.{}.local", self.mp())
    }

    fn lokal(&self, args: &[&str]) -> Output {
        let mut c = Command::new(&self.lokal);
        c.args(args)
            .env("NO_COLOR", "1")
            .env_remove("LOKAL_HOME")
            .stdin(Stdio::null());
        c.output().expect("run lokal")
    }

    fn check(&self, args: &[&str]) -> (bool, Value) {
        let o = run(&self.bin_dir.join("check"), args);
        let v: Value =
            serde_json::from_slice(&o.stdout).unwrap_or_else(|_| json!({ "raw": out_str(&o) }));
        (o.status.success(), v)
    }

    fn status(&self) -> Value {
        serde_json::from_slice(&self.lokal(&["status", "--json"]).stdout).unwrap_or(Value::Null)
    }

    fn record(&mut self, id: &'static str, desc: &'static str, t: Instant, r: Check) {
        let elapsed = t.elapsed();
        let (status, detail) = match r {
            Ok(d) if d.starts_with("SKIP ") => (St::Skip, d[5..].to_string()),
            Ok(d) => (St::Pass, d),
            Err(d) => (St::Fail, d),
        };
        let tag = match status {
            St::Pass => "PASS",
            St::Fail => "FAIL",
            St::Skip => "SKIP",
        };
        let det = if detail.is_empty() {
            String::new()
        } else {
            format!(" [{}]", detail.replace('\n', " | "))
        };
        println!(
            "{tag} {id} {desc} ({:.0}ms){det}",
            elapsed.as_secs_f64() * 1000.0
        );
        let _ = std::io::stdout().flush();
        self.results.push(Result_ {
            id,
            desc,
            status,
            ms: elapsed.as_millis(),
            detail,
        });
    }

    fn step(&mut self, id: &'static str, desc: &'static str, f: impl FnOnce(&mut Ctx) -> Check) {
        let t = Instant::now();
        let r = f(self);
        self.record(id, desc, t, r);
    }

    // ── shared probes ──

    /// L05 for one name: both addresses, first < 20ms, repeat < 5ms, first try.
    fn resolve_ok(&self, fqdn: &str) -> Check {
        let (ok, v) = self.check(&["resolve", fqdn]);
        if !ok {
            return Err(format!("{fqdn}: {}", v));
        }
        let addrs: Vec<String> = v["addrs"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|x| x.as_str().map(String::from))
            .collect();
        if !(addrs.contains(&"127.0.0.1".to_string()) && addrs.contains(&"::1".to_string())) {
            return Err(format!("{fqdn}: addrs {addrs:?}"));
        }
        let first = v["first_ms"].as_f64().unwrap_or(999.0);
        let repeat = v["repeat_ms"].as_f64().unwrap_or(999.0);
        if first >= 20.0 || repeat >= 5.0 {
            return Err(format!(
                "{fqdn}: first {first}ms (<20), repeat {repeat}ms (<5)"
            ));
        }
        Ok(format!("{fqdn} {first}/{repeat}ms"))
    }

    fn curl_https(&self, url: &str) -> Output {
        let mut args = vec!["-sS", "--fail", "--max-time", "10"];
        let ca;
        if self.curl_cacert {
            ca = String::from_utf8_lossy(&self.lokal(&["ca", "path"]).stdout)
                .trim()
                .to_string();
            args.push("--cacert");
            args.push(&ca);
        }
        args.push(url);
        run(Path::new("/usr/bin/curl"), &args)
    }

    /// L06: `check tls` + curl (no -k, no --cacert unless V6 says so) returns `marker`.
    fn l06(&self, marker: &str) -> Check {
        let api = self.fqdn("api");
        let (ok, v) = self.check(&["tls", &api]);
        if !ok {
            return Err(format!("check tls: {v}"));
        }
        let o = self.curl_https(&format!("https://{api}/"));
        let body = String::from_utf8_lossy(&o.stdout);
        if !o.status.success() || !body.contains(&format!("\"marker\":\"{marker}\"")) {
            return Err(format!("curl: {}", out_str(&o).trim()));
        }
        // The .local project (mDNS path) works the same way.
        let m = self.mdns_fqdn();
        let o = self.curl_https(&format!("https://{m}/"));
        if !o.status.success()
            || !String::from_utf8_lossy(&o.stdout).contains("\"marker\":\"MARKER_A\"")
        {
            return Err(format!("curl {m}: {}", out_str(&o).trim()));
        }
        Ok(format!(
            "tls ok (alpn {}), curl → {marker}, .local curl → MARKER_A{}",
            v["alpn"],
            if self.curl_cacert {
                " (curl --cacert per V6)"
            } else {
                ""
            }
        ))
    }

    fn get_marker(&self, url: &str, marker: &str) -> Check {
        let (ok, v) = self.check(&["get", url]);
        if !ok {
            return Err(format!("{url}: {v}"));
        }
        let body = v["body"].as_str().unwrap_or("");
        if v["status"] != 200 || !body.contains(&format!("\"marker\":\"{marker}\"")) {
            return Err(format!(
                "{url}: status {} body {}",
                v["status"],
                &body[..body.len().min(200)]
            ));
        }
        Ok(format!("{url} → {marker}"))
    }

    /// `check resolve` must fail for each name (runs in parallel: negative
    /// .local lookups take ~5s).
    fn all_unresolvable(&self, names: &[String]) -> Check {
        let check = self.bin_dir.join("check");
        let handles: Vec<_> = names
            .iter()
            .cloned()
            .map(|n| {
                let check = check.clone();
                std::thread::spawn(move || {
                    let o = run(&check, &["resolve", &n]);
                    (
                        n,
                        o.status.success(),
                        String::from_utf8_lossy(&o.stdout).into_owned(),
                    )
                })
            })
            .collect();
        let mut still = vec![];
        for h in handles {
            let (n, ok, out) = h.join().unwrap();
            if ok {
                still.push(format!("{n}: {}", out.trim()));
            }
        }
        if still.is_empty() {
            Ok(format!("{} name(s) gone", names.len()))
        } else {
            Err(still.join("; "))
        }
    }

    /// A TLS connection to `fqdn` forced to 127.0.0.1 must fail.
    fn forced_tls_fails(&self, fqdn: &str) -> Check {
        let o = run(
            Path::new("/usr/bin/curl"),
            &[
                "-sS",
                "--max-time",
                "5",
                "--resolve",
                &format!("{fqdn}:443:127.0.0.1"),
                &format!("https://{fqdn}/"),
            ],
        );
        if o.status.success() {
            Err(format!("TLS to {fqdn} succeeded: {}", out_str(&o)))
        } else {
            Ok(format!(
                "curl exit {} ({})",
                o.status.code().unwrap_or(-1),
                out_str(&o).trim()
            ))
        }
    }

    fn daemon_pid(&self) -> Option<u64> {
        self.status()["pid"].as_u64()
    }

    /// Makes the developer's previous current project current again.
    fn restore_current(&self) -> Check {
        let Some(prev) = &self.prev_current else {
            return Ok("no current project before the run".into());
        };
        let o = self.lokal(&["project", "use", prev]);
        if !o.status.success() {
            return Err(format!("project use {prev}: {}", out_str(&o)));
        }
        Ok(format!("current project {prev} restored"))
    }

    fn cleanup(&mut self) {
        if self.project_created {
            let _ = self.lokal(&["project", "remove", &self.p.clone(), "--force"]);
            let _ = self.lokal(&["project", "remove", &self.mp(), "--force"]);
            let _ = self.restore_current();
            self.project_created = false;
        }
        for c in &mut self.upstreams {
            let _ = c.kill();
            let _ = c.wait();
        }
        self.upstreams.clear();
    }
}

impl Drop for Ctx {
    fn drop(&mut self) {
        self.cleanup();
        let _ = std::fs::remove_dir_all(&self.tmp);
    }
}

fn spawn_echo(bin_dir: &Path, bind: &str, marker: &str) -> (Child, u16) {
    let mut c = Command::new(bin_dir.join("echo-server"))
        .args(["--bind", bind, "--port", "0", "--marker", marker])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("start echo-server");
    let mut line = String::new();
    BufReader::new(c.stdout.as_mut().unwrap())
        .read_line(&mut line)
        .unwrap();
    let port = line
        .trim()
        .rsplit(':')
        .next()
        .and_then(|p| p.parse().ok())
        .expect("echo port");
    (c, port)
}

fn free_port() -> u16 {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap().port()
}

fn closed(addr: &str) -> bool {
    TcpStream::connect_timeout(
        &addr.parse::<SocketAddr>().unwrap(),
        Duration::from_millis(200),
    )
    .is_err()
}

fn rand_hex() -> String {
    let mut b = [0u8; 3];
    ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut b).unwrap();
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// `lokal 0.1.0 (abc1234 2026-10-10T12:00:00Z)` → `0.1.0+abc1234.20261010T120000Z`
fn build_from_version(v: &str) -> Option<String> {
    let v = v.trim().strip_prefix("lokal ")?;
    let (ver, rest) = v.split_once(" (")?;
    let (hash, date) = rest.trim_end_matches(')').split_once(' ')?;
    let stamp: String = date.chars().filter(|c| *c != '-' && *c != ':').collect();
    Some(format!("{ver}+{hash}.{stamp}"))
}

// ───────────────────────────── checks ─────────────────────────────

fn l01(cx: &mut Ctx) -> Check {
    let o = Command::new(&cx.lokal)
        .args(["setup", "--yes"])
        .env("LOKAL_FORBID_SUDO", "1")
        .env("NO_COLOR", "1")
        .stdin(Stdio::null())
        .output()
        .map_err(|e| e.to_string())?;
    let text = out_str(&o);
    if !o.status.success() {
        return Err(format!(
            "exit {:?}: {}",
            o.status.code(),
            text.lines().rev().take(4).collect::<Vec<_>>().join(" | ")
        ));
    }
    if !text.contains("nothing to change") {
        return Err(format!(
            "setup changed something: {}",
            text.lines()
                .filter(|l| l.contains('✓'))
                .collect::<Vec<_>>()
                .join(" | ")
        ));
    }
    Ok("already set up, no sudo".into())
}

fn doctor(cx: &Ctx) -> Result<Value, String> {
    let o = cx.lokal(&["doctor", "--json"]);
    serde_json::from_slice(&o.stdout).map_err(|e| format!("doctor --json: {e}: {}", out_str(&o)))
}

fn l02(cx: &mut Ctx) -> Check {
    let v = doctor(cx)?;
    let checks = v["checks"].as_array().cloned().unwrap_or_default();
    let fails: Vec<String> = checks
        .iter()
        .filter(|c| c["status"] == "fail")
        .map(|c| {
            format!(
                "{}: {}",
                c["id"].as_str().unwrap_or(""),
                c["detail"].as_str().unwrap_or("")
            )
        })
        .collect();
    if !fails.is_empty() {
        return Err(fails.join("; "));
    }
    let st = checks
        .iter()
        .find(|c| c["id"] == "selftest")
        .map(|c| c["status"].clone());
    if st != Some(json!("pass")) {
        return Err(format!("selftest {st:?}"));
    }
    Ok(format!("{} checks, no fail", checks.len()))
}

fn l03(cx: &mut Ctx) -> Check {
    let (ca, a) = spawn_echo(&cx.bin_dir, "127.0.0.1", "MARKER_A");
    let (cb, b) = spawn_echo(&cx.bin_dir, "::1", "MARKER_B");
    let (cd, d) = spawn_echo(&cx.bin_dir, "127.0.0.1", "MARKER_D");
    cx.upstreams.extend([ca, cb, cd]);
    cx.a = a;
    cx.b = b;
    cx.d = d;
    cx.c = free_port();
    if !closed(&format!("127.0.0.1:{b}")) {
        return Err(format!(
            "B must listen on ::1 only, but 127.0.0.1:{b} accepts"
        ));
    }
    if !closed(&format!("127.0.0.1:{}", cx.c)) || !closed(&format!("[::1]:{}", cx.c)) {
        return Err("port C is not closed".into());
    }
    Ok(format!(
        "A=127.0.0.1:{a} B=[::1]:{b} C={} (closed) D=127.0.0.1:{d}",
        cx.c
    ))
}

fn l04(cx: &mut Ctx) -> Check {
    let p = cx.p.clone();
    let o = cx.lokal(&["project", "create", &p]);
    if !o.status.success() {
        return Err(format!("project create: {}", out_str(&o)));
    }
    cx.project_created = true;
    let mut times = vec![];
    for (sub, port) in [("api", cx.a), ("web", cx.b), ("down", cx.c), ("@", cx.a)] {
        let o = cx.lokal(&["service", "add", sub, &port.to_string()]);
        let text = out_str(&o);
        if !o.status.success() || !text.contains("ready in") {
            return Err(format!("service add {sub} {port}: {text}"));
        }
        let t = text
            .split("ready in ")
            .nth(1)
            .and_then(|s| s.split(')').next())
            .unwrap_or("?")
            .to_string();
        times.push(format!("{sub} {t}"));
    }
    // Setup commands are safe to re-run (identical arguments → exit 0, still ready).
    let o = cx.lokal(&["project", "create", &p]);
    if !o.status.success() || !out_str(&o).contains("already exists (current project)") {
        return Err(format!("re-run project create: {}", out_str(&o)));
    }
    let o = cx.lokal(&["service", "add", "api", &cx.a.to_string()]);
    if !o.status.success() || !out_str(&o).contains("already configured, ready in") {
        return Err(format!("re-run service add: {}", out_str(&o)));
    }
    // A second project under .local (mDNS), with one service on A.
    let mp = cx.mp();
    let o = cx.lokal(&["project", "create", &mp, "--domain", &format!("{mp}.local")]);
    if !o.status.success() {
        return Err(format!("project create {mp} (.local): {}", out_str(&o)));
    }
    let o = cx.lokal(&["service", "add", "api", &cx.a.to_string(), "-p", &mp]);
    if !o.status.success() || !out_str(&o).contains("ready in") {
        return Err(format!("service add api (.local): {}", out_str(&o)));
    }
    // Back to the main project for the rest of the run.
    cx.lokal(&["project", "use", &p]);
    Ok(format!(
        "{}; re-runs ok; {} added",
        times.join(", "),
        cx.mdns_fqdn()
    ))
}

fn l05(cx: &mut Ctx) -> Check {
    let mut v = vec![];
    for sub in ["api", "web", "down", "@"] {
        v.push(cx.resolve_ok(&cx.fqdn(sub))?);
    }
    v.push(cx.resolve_ok(&cx.mdns_fqdn())?);
    Ok(v.join("; "))
}

fn l07(cx: &mut Ctx) -> Check {
    if !Path::new(CHROME).exists() {
        return Err("Google Chrome is not installed".into());
    }
    let profile = cx.tmp.join(format!("chrome-{}", rand_hex()));
    let url = format!("https://{}/", cx.fqdn("web"));
    let mut child = Command::new(CHROME)
        .args([
            "--headless=new",
            &format!("--user-data-dir={}", profile.display()),
            "--no-first-run",
            "--no-default-browser-check",
            "--dump-dom",
            "--virtual-time-budget=3000",
            &url,
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .map_err(|e| e.to_string())?;
    let pid = child.id() as i32;
    // Chrome headless on macOS 27 prints the DOM but doesn't always exit:
    // read until the document ends (or Chrome reports a failed load on
    // stderr), then end the whole process group.
    let (tx, rx) = std::sync::mpsc::channel::<(bool, Vec<u8>)>();
    let mut stdout = child.stdout.take().unwrap();
    let txo = tx.clone();
    std::thread::spawn(move || {
        let mut all = Vec::new();
        let mut buf = [0u8; 8192];
        loop {
            match stdout.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    all.extend_from_slice(&buf[..n]);
                    if String::from_utf8_lossy(&all).contains("</html>") {
                        break;
                    }
                }
            }
        }
        let _ = txo.send((true, all));
    });
    let stderr = child.stderr.take().unwrap();
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            if line.contains("Page load failed") || line.contains("ERR_") {
                let _ = tx.send((false, line.into_bytes()));
                return;
            }
        }
    });
    let (is_dom, data) = rx
        .recv_timeout(Duration::from_secs(30))
        .unwrap_or((true, vec![]));
    unsafe { libc::killpg(pid, libc::SIGKILL) };
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&profile);
    if !is_dom {
        return Err(format!("Chrome: {}", String::from_utf8_lossy(&data).trim()));
    }
    let dom = data;
    let text = String::from_utf8_lossy(&dom);
    if text.contains("ERR_") {
        return Err(format!(
            "Chrome error page: {}",
            &text[..text.len().min(300)]
        ));
    }
    if !text.contains("MARKER_B") {
        return Err(format!(
            "marker B missing from DOM: {}",
            &text[..text.len().min(300)]
        ));
    }
    Ok(format!("{url} → MARKER_B via ::1-only upstream"))
}

fn l08(cx: &mut Ctx) -> Check {
    let api = cx.fqdn("api");
    let o = Command::new("/usr/bin/openssl")
        .args([
            "s_client",
            "-connect",
            "127.0.0.1:443",
            "-servername",
            &api,
            "-showcerts",
        ])
        .stdin(Stdio::null())
        .output()
        .map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&o.stdout);
    let start = text
        .find("-----BEGIN CERTIFICATE-----")
        .ok_or("no certificate from s_client")?;
    let end = text[start..]
        .find("-----END CERTIFICATE-----")
        .ok_or("bad PEM")?
        + start
        + 25;
    let leaf = cx.tmp.join("l08-leaf.pem");
    std::fs::write(&leaf, &text[start..end]).map_err(|e| e.to_string())?;
    let o = run(
        Path::new("/usr/bin/security"),
        &[
            "verify-cert",
            "-c",
            leaf.to_str().unwrap(),
            "-p",
            "ssl",
            "-s",
            &api,
            "-L",
        ],
    );
    if o.status.success() {
        Ok("security verify-cert: successful".into())
    } else {
        Err(out_str(&o))
    }
}

fn l10(cx: &mut Ctx) -> Check {
    let api = cx.fqdn("api");
    let o = run(
        Path::new("/usr/bin/curl"),
        &["-sI", "--max-time", "5", &format!("http://{api}/x?y=1")],
    );
    let text = String::from_utf8_lossy(&o.stdout).to_string();
    let first = text.lines().next().unwrap_or("").to_string();
    let loc = text
        .lines()
        .find_map(|l| {
            l.to_ascii_lowercase()
                .starts_with("location:")
                .then(|| l[9..].trim().to_string())
        })
        .unwrap_or_default();
    if first.contains("308") && loc == format!("https://{api}/x?y=1") {
        Ok(format!("{first} → {loc}"))
    } else {
        Err(format!("{first} location={loc:?}"))
    }
}

fn l13(cx: &mut Ctx) -> Check {
    let url = format!("https://{}/", cx.fqdn("down"));
    let (ok, v) = cx.check(&["get", &url]);
    if !ok {
        return Err(v.to_string());
    }
    let body = v["body"].as_str().unwrap_or("");
    let want = format!("localhost:{}", cx.c);
    if v["status"] == 502 && body.contains(&want) {
        Ok(format!("502 mentions {want}"))
    } else {
        Err(format!(
            "status {} body {}",
            v["status"],
            &body[..body.len().min(200)]
        ))
    }
}

fn l14(cx: &mut Ctx) -> Check {
    let nope = cx.fqdn("nope");
    let tls = cx.forced_tls_fails(&nope)?;
    let o = run(
        Path::new("/usr/bin/curl"),
        &[
            "-s",
            "-o",
            "/dev/null",
            "-w",
            "%{http_code}",
            "--max-time",
            "5",
            "--resolve",
            &format!("{nope}:80:127.0.0.1"),
            &format!("http://{nope}/"),
        ],
    );
    let code = String::from_utf8_lossy(&o.stdout).to_string();
    if code != "404" {
        return Err(format!("HTTP status {code}"));
    }
    Ok(format!("TLS refused ({tls}); HTTP 404"))
}

fn l15(cx: &mut Ctx) -> Check {
    let pid = cx.daemon_pid();
    let o = cx.lokal(&["service", "add", "new", &cx.d.to_string()]);
    if !o.status.success() {
        return Err(out_str(&o));
    }
    let r = cx.get_marker(&format!("https://{}/", cx.fqdn("new")), "MARKER_D")?;
    let pid2 = cx.daemon_pid();
    if pid != pid2 {
        return Err(format!("daemon PID changed {pid:?} → {pid2:?}"));
    }
    Ok(format!("{r}; pid {pid:?} unchanged"))
}

fn l16(cx: &mut Ctx) -> Check {
    let o = cx.lokal(&["service", "update", "api", "--port", &cx.d.to_string()]);
    if !o.status.success() {
        return Err(out_str(&o));
    }
    cx.get_marker(&format!("https://{}/", cx.fqdn("api")), "MARKER_D")
}

fn l17(cx: &mut Ctx) -> Check {
    let o = cx.lokal(&["service", "remove", "new"]);
    if !o.status.success() {
        return Err(out_str(&o));
    }
    // `.lokal` answers loopback for every name (no negative DNS answers,
    // which macOS would cache ~75s); a removed name is no longer *served*:
    // it has no certificate, so TLS fails at once.
    let n = cx.fqdn("new");
    let t = cx.forced_tls_fails(&n)?;
    let (ok, v) = cx.check(&["tls", &n]);
    if ok {
        return Err(format!("check tls {n} still succeeds: {v}"));
    }
    Ok(format!("TLS refused ({t}); check tls fails"))
}

/// Polls `f` every 50ms until it is true or `limit` passes.
fn poll(limit: Duration, mut f: impl FnMut() -> bool) -> Option<Duration> {
    let t = Instant::now();
    loop {
        if f() {
            return Some(t.elapsed());
        }
        if t.elapsed() > limit {
            return None;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn l18(cx: &mut Ctx) -> Check {
    let pid = cx.daemon_pid().ok_or("daemon not running")?;
    unsafe { libc::kill(pid as i32, libc::SIGKILL) };
    let mut newpid = None;
    let took = poll(Duration::from_secs(2), || {
        let s = cx.status();
        if s["running"] == true && s["pid"].as_u64().is_some_and(|p| p != pid) {
            newpid = s["pid"].as_u64();
            true
        } else {
            false
        }
    })
    .ok_or("no new daemon within 2s")?;
    let l06 = cx.l06("MARKER_D")?;
    Ok(format!(
        "pid {pid} → {:?} in {:.0}ms; {l06}",
        newpid,
        took.as_secs_f64() * 1000.0
    ))
}

fn port443_lokal_free() -> Result<(), String> {
    let o = run(
        Path::new("/usr/sbin/lsof"),
        &["-nP", "-iTCP:443", "-sTCP:LISTEN"],
    );
    let owned: Vec<String> = String::from_utf8_lossy(&o.stdout)
        .lines()
        .skip(1)
        .filter(|l| l.starts_with("lokal") || l.starts_with("launchd"))
        .map(String::from)
        .collect();
    if !owned.is_empty() {
        return Err(owned.join("; "));
    }
    if !closed("127.0.0.1:443") || !closed("[::1]:443") {
        return Err("something still accepts on 443".into());
    }
    Ok(())
}

fn l19(cx: &mut Ctx) -> Check {
    let o = cx.lokal(&["stop"]);
    if !o.status.success() {
        return Err(format!("stop: {}", out_str(&o)));
    }
    if cx.status()["running"] != false {
        return Err("still running after stop".into());
    }
    port443_lokal_free()?;
    // .local names stop resolving; .lokal ones can't connect (checked above:
    // nothing accepts on 443).
    cx.all_unresolvable(&[cx.mdns_fqdn()])?;
    let t = Instant::now();
    let o = cx.lokal(&["start"]);
    if !o.status.success() {
        return Err(format!("start: {}", out_str(&o)));
    }
    let mut last = String::new();
    let took = poll(Duration::from_secs(2), || {
        match (cx.l05_quiet(), cx.l06("MARKER_D")) {
            (Ok(_), Ok(_)) => true,
            (a, b) => {
                last = format!("{a:?} {b:?}");
                false
            }
        }
    });
    match took {
        Some(_) => Ok(format!(
            "stopped (443 free, names gone); start → L05+L06 in {:.0}ms",
            t.elapsed().as_secs_f64() * 1000.0
        )),
        None => Err(format!("after start, not passing within 2s: {last}")),
    }
}

impl Ctx {
    fn l05_quiet(&self) -> Check {
        for sub in ["api", "web", "down", "@"] {
            self.resolve_ok(&self.fqdn(sub))?;
        }
        self.resolve_ok(&self.mdns_fqdn())?;
        Ok(String::new())
    }
}

fn l20(cx: &mut Ctx) -> Check {
    let o = cx.lokal(&["upgrade"]);
    if !o.status.success() {
        return Err(format!("upgrade: {}", out_str(&o)));
    }
    let want = build_from_version(&String::from_utf8_lossy(&cx.lokal(&["--version"]).stdout))
        .ok_or("cannot parse --version")?;
    let got = cx.status()["build_id"].as_str().unwrap_or("").to_string();
    if got != want {
        return Err(format!("status.build_id {got} != {want}"));
    }
    let l06 = cx.l06("MARKER_D")?;
    Ok(format!(
        "{}; {l06}",
        String::from_utf8_lossy(&o.stdout).trim()
    ))
}

fn l21(cx: &mut Ctx) -> Check {
    let lokal = cx.lokal.display().to_string();
    let url = format!("https://{}/", cx.fqdn("api"));
    let mut parts = vec![];
    let sh = |script: String| {
        Command::new("/bin/bash")
            .args(["-c", &script])
            .env("LOKAL_BIN", &lokal)
            .env("URL", &url)
            .stdin(Stdio::null())
            .output()
    };
    let have = |tool: &str| {
        Command::new("/bin/bash")
            .args(["-lc", &format!("command -v {tool}")])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    };
    if have("node") {
        let o = sh(r#"eval "$("$LOKAL_BIN" env)"; node -e 'fetch(process.env.URL).then(r=>{if(!r.ok){console.error("status",r.status);process.exit(1)}console.log("node ok",r.status)}).catch(e=>{console.error(String(e.cause||e));process.exit(1)})'"#.into())
            .map_err(|e| e.to_string())?;
        if !o.status.success() {
            return Err(format!("node: {}", out_str(&o)));
        }
        parts.push("node ok".to_string());
    } else {
        parts.push("node SKIP (not installed)".into());
    }
    if have("python3") {
        let o = sh(r#"eval "$("$LOKAL_BIN" env)"; python3 -I -c 'import os,urllib.request; print("py", urllib.request.urlopen(os.environ["URL"], timeout=10).status)'"#.into())
            .map_err(|e| e.to_string())?;
        if !o.status.success() {
            return Err(format!("python: {}", out_str(&o)));
        }
        parts.push("python ok".into());
        let online =
            TcpStream::connect_timeout(&"1.1.1.1:443".parse().unwrap(), Duration::from_secs(2))
                .is_ok();
        if online {
            let o = sh(r#"eval "$("$LOKAL_BIN" env)"; python3 -I -c 'import urllib.request; print("ex", urllib.request.urlopen("https://example.com", timeout=10).status)'"#.into())
                .map_err(|e| e.to_string())?;
            if !o.status.success() {
                return Err(format!(
                    "python example.com (system roots in bundle): {}",
                    out_str(&o)
                ));
            }
            parts.push("python example.com ok".into());
        } else {
            parts.push("example.com SKIP (offline)".into());
        }
    } else {
        parts.push("python SKIP (not installed)".into());
    }
    Ok(parts.join(", "))
}

fn l22(cx: &mut Ctx) -> Check {
    let v = doctor(cx)?;
    let checks = v["checks"].as_array().cloned().unwrap_or_default();
    let mut bad = vec![];
    for c in &checks {
        let id = c["id"].as_str().unwrap_or("");
        let must_pass = id.starts_with("dns.")
            || id.starts_with("tls.")
            || id == "selftest"
            || id == "ca.trusted"
            || id == "ca.trust_scope";
        if must_pass && c["status"] != "pass" {
            bad.push(format!(
                "{id}={} ({})",
                c["status"],
                c["detail"].as_str().unwrap_or("")
            ));
        }
    }
    let down = format!("upstream.{}", cx.fqdn("down"));
    let dst = checks
        .iter()
        .find(|c| c["id"] == down.as_str())
        .map(|c| c["status"].clone());
    if dst != Some(json!("warn")) {
        bad.push(format!("{down}={dst:?}, want warn"));
    }
    if bad.is_empty() {
        Ok(format!("{} checks", checks.len()))
    } else {
        Err(bad.join("; "))
    }
}

/// A `lokal run` started by L26. A thread owns the child and reports its
/// exit code; if a check fails first, dropping the guard stops it
/// (SIGTERM, which lokal relays to the echo-server, then SIGKILL).
struct RunGuard {
    pid: u32,
    exited: std::sync::mpsc::Receiver<i32>,
    done: bool,
}

impl RunGuard {
    fn spawn(mut child: Child) -> Self {
        let pid = child.id();
        let (tx, exited) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let code = child.wait().ok().and_then(|s| s.code()).unwrap_or(-1);
            let _ = tx.send(code);
        });
        Self {
            pid,
            exited,
            done: false,
        }
    }

    /// The exit code, if lokal exits within `limit`.
    fn wait(&mut self, limit: Duration) -> Option<i32> {
        let code = self.exited.recv_timeout(limit).ok()?;
        self.done = true;
        Some(code)
    }

    fn signal(&self, sig: libc::c_int) {
        unsafe { libc::kill(self.pid as i32, sig) };
    }
}

impl Drop for RunGuard {
    fn drop(&mut self) {
        if self.done || self.exited.try_recv().is_ok() {
            return;
        }
        self.signal(libc::SIGTERM);
        if self.wait(Duration::from_secs(3)).is_none() {
            self.signal(libc::SIGKILL);
        }
    }
}

/// `lokal run lr -p <p> [extra] -- echo-server --port {port} --marker
/// MARKER_R`: `✓ ready:` within 5s, then the URL works on the first try;
/// SIGTERM ends lokal with 143 and the echo-server with it.
fn l26_run(cx: &Ctx, extra: &[&str]) -> Check {
    let echo = cx.bin_dir.join("echo-server");
    let echo = echo.to_str().ok_or("bin dir is not UTF-8")?;
    let mut args = vec!["run", "lr", "-p", cx.p.as_str()];
    args.extend_from_slice(extra);
    args.extend_from_slice(&["--", echo, "--port", "{port}", "--marker", "MARKER_R"]);
    let mut child = Command::new(&cx.lokal)
        .args(&args)
        .env("NO_COLOR", "1")
        .env_remove("LOKAL_HOME")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("start lokal run: {e}"))?;
    let lokal_pid = child.id();
    let stderr = child.stderr.take().ok_or("no stderr")?;
    let (line_tx, lines) = std::sync::mpsc::channel::<String>();
    std::thread::spawn(move || {
        for l in BufReader::new(stderr).lines().map_while(Result::ok) {
            if line_tx.send(l).is_err() {
                break;
            }
        }
    });
    let mut guard = RunGuard::spawn(child);
    let mut seen = vec![];
    let t = Instant::now();
    let ready = loop {
        let left = Duration::from_secs(5).saturating_sub(t.elapsed());
        match lines.recv_timeout(left) {
            Ok(l) if l.contains("✓ ready:") => break l,
            Ok(l) => seen.push(l),
            Err(_) => {
                return Err(format!("no `✓ ready:` within 5s: {}", seen.join(" | ")));
            }
        }
    };
    let url = format!("https://{}/", cx.fqdn("lr"));
    if !ready.contains(&url[..url.len() - 1]) {
        return Err(format!("ready line {ready:?} doesn't name {url}"));
    }
    // First try: `✓ ready:` promises the URL works.
    let o = cx.curl_https(&url);
    if !o.status.success()
        || !String::from_utf8_lossy(&o.stdout).contains("\"marker\":\"MARKER_R\"")
    {
        return Err(format!("curl {url}: {}", out_str(&o).trim()));
    }
    let o = run(Path::new("/usr/bin/pgrep"), &["-P", &lokal_pid.to_string()]);
    let echo_pid: i32 = String::from_utf8_lossy(&o.stdout)
        .lines()
        .next()
        .and_then(|l| l.trim().parse().ok())
        .ok_or_else(|| format!("no child of lokal run: {}", out_str(&o)))?;
    guard.signal(libc::SIGTERM);
    let code = guard
        .wait(Duration::from_secs(5))
        .ok_or("lokal run didn't exit within 5s of SIGTERM")?;
    // SAFETY: signal 0 only checks that the pid exists.
    let echo_alive = unsafe { libc::kill(echo_pid, 0) } == 0;
    if code != 143 || echo_alive {
        return Err(format!(
            "after SIGTERM: exit {code} (want 143), echo-server pid {echo_pid} {}",
            if echo_alive { "still running" } else { "gone" }
        ));
    }
    let took = ready
        .rsplit('(')
        .next()
        .map(|s| s.trim_end_matches(')').to_string())
        .unwrap_or_default();
    Ok(format!(
        "ready in {took}, curl → MARKER_R, SIGTERM → 143, echo-server gone"
    ))
}

/// Is `lr` in the test project, per `list --json`?
fn l26_listed(cx: &Ctx) -> Result<bool, String> {
    let v: Value =
        serde_json::from_slice(&cx.lokal(&["list", "--json"]).stdout).map_err(|e| e.to_string())?;
    Ok(v["projects"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|p| p["name"] == cx.p.as_str())
        .flat_map(|p| p["services"].as_array().cloned().unwrap_or_default())
        .any(|s| s["subdomain"] == "lr"))
}

fn l26(cx: &mut Ctx) -> Check {
    let first = l26_run(cx, &[])?;
    // The route stays after the command exits (SPEC §4.10).
    if !l26_listed(cx)? {
        return Err(format!("{first}; but lr is gone without --rm"));
    }
    let o = cx.lokal(&["service", "remove", "lr", "-p", &cx.p.clone()]);
    if !o.status.success() {
        return Err(format!("service remove lr: {}", out_str(&o)));
    }
    let second = l26_run(cx, &["--rm"])?;
    if l26_listed(cx)? {
        return Err(format!("--rm: {second}; but lr is still listed"));
    }
    Ok(format!(
        "{first}; route kept; with --rm: {second}, lr removed"
    ))
}

/// Are `ua` and `ub` in the test project, per `list --json`?
fn l27_listed(cx: &Ctx) -> Result<(bool, bool), String> {
    let v: Value =
        serde_json::from_slice(&cx.lokal(&["list", "--json"]).stdout).map_err(|e| e.to_string())?;
    let subs: Vec<String> = v["projects"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|p| p["name"] == cx.p.as_str())
        .flat_map(|p| p["services"].as_array().cloned().unwrap_or_default())
        .filter_map(|s| s["subdomain"].as_str().map(String::from))
        .collect();
    Ok((
        subs.iter().any(|s| s == "ua"),
        subs.iter().any(|s| s == "ub"),
    ))
}

/// The pids of processes whose command line has `marker` (`pgrep -f`).
fn pgrep(marker: &str) -> Vec<String> {
    let o = run(Path::new("/usr/bin/pgrep"), &["-f", marker]);
    String::from_utf8_lossy(&o.stdout)
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect()
}

/// The rest of `line` after an RFC 3339 UTC timestamp with microseconds
/// and a space (`2026-10-25T10:44:34.635510Z `), the format of a `lokal up`
/// service's log lines.
fn after_timestamp(line: &str) -> Option<&str> {
    let b = line.as_bytes();
    let stamped = b.len() >= 28
        && b[..28].iter().enumerate().all(|(i, c)| match i {
            4 | 7 => *c == b'-',
            10 => *c == b'T',
            13 | 16 => *c == b':',
            19 => *c == b'.',
            26 => *c == b'Z',
            27 => *c == b' ',
            _ => c.is_ascii_digit(),
        });
    stamped.then(|| &line[28..])
}

/// `lokal logs <sub> -p <p> -n 5` exits 0 and shows the echo-server's
/// first line, `LISTENING <addr>`, after a timestamp (SPEC §4.11, "Logs").
fn l27_logs(cx: &Ctx, sub: &str) -> Result<(), String> {
    let o = cx.lokal(&["logs", sub, "-p", &cx.p, "-n", "5"]);
    let listening = String::from_utf8_lossy(&o.stdout)
        .lines()
        .any(|l| after_timestamp(l).is_some_and(|t| t.starts_with("LISTENING ")));
    if !o.status.success() || !listening {
        return Err(format!(
            "lokal logs {sub} -p {} -n 5 (exit {:?}), no `<timestamp> LISTENING`: {}",
            cx.p,
            o.status.code(),
            out_str(&o).trim()
        ));
    }
    Ok(())
}

/// L27: `lokal up --yes` in a repo whose `lokal.toml` has two echo-servers.
/// `✓ ready: 2/2` within 10s, then both URLs return their markers on the
/// first try, and `ua`'s log has its `LISTENING` line; SIGINT ends lokal
/// with 130 within 15s and no echo-server is left; the routes stay until
/// `lokal down` removes them.
fn l27_run(cx: &Ctx, dir: &Path, markers: &[String; 2]) -> Check {
    let mut child = Command::new(&cx.lokal)
        .args(["up", "--yes"])
        .current_dir(dir)
        .env("NO_COLOR", "1")
        .env_remove("LOKAL_HOME")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("start lokal up: {e}"))?;
    let stdout = child.stdout.take().ok_or("no stdout")?;
    let stderr = child.stderr.take().ok_or("no stderr")?;
    let (line_tx, lines) = std::sync::mpsc::channel::<String>();
    std::thread::spawn(move || {
        for l in BufReader::new(stdout).lines().map_while(Result::ok) {
            if line_tx.send(l).is_err() {
                break;
            }
        }
    });
    let errs = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let e = errs.clone();
    std::thread::spawn(move || {
        let mut s = String::new();
        let _ = BufReader::new(stderr).read_to_string(&mut s);
        e.lock().unwrap().push_str(&s);
    });
    let mut guard = RunGuard::spawn(child);
    let mut seen = vec![];
    let t = Instant::now();
    let ready = loop {
        let left = Duration::from_secs(10).saturating_sub(t.elapsed());
        match lines.recv_timeout(left) {
            Ok(l) if l.contains("✓ ready: 2/2") => break l,
            Ok(l) => seen.push(l),
            Err(_) => {
                return Err(format!(
                    "no `✓ ready: 2/2` within 10s: {} | stderr: {}",
                    seen.join(" | "),
                    errs.lock().unwrap().trim()
                ));
            }
        }
    };
    // First try: `✓ ready:` promises every URL works.
    for (sub, marker) in ["ua", "ub"].iter().zip(markers) {
        let url = format!("https://{}/", cx.fqdn(sub));
        let o = cx.curl_https(&url);
        if !o.status.success()
            || !String::from_utf8_lossy(&o.stdout).contains(&format!("\"marker\":\"{marker}\""))
        {
            return Err(format!("curl {url}: {}", out_str(&o).trim()));
        }
    }
    // Its output is in its log too, readable from another process.
    l27_logs(cx, "ua")?;
    let restarted = l27_restart(cx, &markers[0])?;
    guard.signal(libc::SIGINT);
    let code = guard
        .wait(Duration::from_secs(15))
        .ok_or("lokal up didn't exit within 15s of SIGINT")?;
    let left: Vec<String> = markers.iter().flat_map(|m| pgrep(m)).collect();
    if code != 130 || !left.is_empty() {
        return Err(format!(
            "after SIGINT: exit {code} (want 130), echo-servers left: {left:?}"
        ));
    }
    let took = ready
        .rsplit('(')
        .next()
        .map(|s| s.trim_end_matches(')').to_string())
        .unwrap_or_default();
    Ok(format!(
        "ready in {took}, curl → both markers, logs ua → LISTENING, {restarted}, SIGINT → 130, echo-servers gone"
    ))
}

/// `lokal restart ua -p <p>` exits 0 and prints `✓ restarted
/// https://ua.<p>.lokal (…)` once ua's new run accepts connections; then
/// its URL returns `marker` on the first request (no retry, no `-k`), from
/// a new process (SPEC §4.11, "Restarting a service").
fn l27_restart(cx: &Ctx, marker: &str) -> Result<String, String> {
    let before = pgrep(marker);
    let t = Instant::now();
    let o = cx.lokal(&["restart", "ua", "-p", &cx.p]);
    let took = t.elapsed();
    let want = format!("✓ restarted https://{} (", cx.fqdn("ua"));
    if !o.status.success() || !String::from_utf8_lossy(&o.stdout).starts_with(&want) {
        return Err(format!(
            "lokal restart ua -p {} (exit {:?}), no `{want}`: {}",
            cx.p,
            o.status.code(),
            out_str(&o).trim()
        ));
    }
    let url = format!("https://{}/", cx.fqdn("ua"));
    let o = cx.curl_https(&url);
    if !o.status.success()
        || !String::from_utf8_lossy(&o.stdout).contains(&format!("\"marker\":\"{marker}\""))
    {
        return Err(format!(
            "after lokal restart ua returned, curl {url}: {}",
            out_str(&o).trim()
        ));
    }
    let after = pgrep(marker);
    if after.is_empty() || after.iter().any(|p| before.contains(p)) {
        return Err(format!(
            "lokal restart ua didn't start a new process: pids {before:?} before, {after:?} after"
        ));
    }
    Ok(format!("restart ua in {:.0}ms → curl → marker", ms(took)))
}

fn l27(cx: &mut Ctx) -> Check {
    let echo = cx.bin_dir.join("echo-server");
    let echo = echo.to_str().ok_or("bin dir is not UTF-8")?.to_string();
    let r = format!("{}{}", rand_hex(), rand_hex());
    let markers = [format!("UA-{r}"), format!("UB-{r}")];
    let dir = cx.tmp.join("up");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let toml = format!(
        "project = \"{p}\"\n[services]\nua = \"'{echo}' --port {{port}} --marker {a}\"\nub = \"'{echo}' --port {{port}} --marker {b}\"\n",
        p = cx.p,
        a = markers[0],
        b = markers[1]
    );
    std::fs::write(dir.join("lokal.toml"), toml).map_err(|e| e.to_string())?;
    let res = (|| {
        let first = l27_run(cx, &dir, &markers)?;
        // The routes stay after the commands stop (SPEC §4.11).
        if l27_listed(cx)? != (true, true) {
            return Err(format!("{first}; but ua and ub aren't both in list --json"));
        }
        let o = cx.lokal(&["down", "-p", &cx.p.clone()]);
        if !o.status.success() {
            return Err(format!("down -p {}: {}", cx.p, out_str(&o)));
        }
        if l27_listed(cx)? != (false, false) {
            return Err(format!("{first}; but ua or ub is still listed after down"));
        }
        cx.forced_tls_fails(&cx.fqdn("ua"))?;
        // A log stays readable after its service is gone.
        l27_logs(cx, "ua").map_err(|e| format!("{first}; after down: {e}"))?;
        Ok(format!(
            "{first}; routes kept; down removed them, TLS to ua fails, ua's log still reads"
        ))
    })();
    // Nothing of the run may stay behind, even after a failure: the
    // echo-servers (only these markers), and `up`'s folder in the
    // developer's log dir (L24 checks isolation).
    for m in &markers {
        for pid in pgrep(m) {
            let _ = run(Path::new("/bin/kill"), &["-KILL", &pid]);
        }
    }
    let paths: Value =
        serde_json::from_slice(&cx.lokal(&["internal", "paths"]).stdout).unwrap_or(Value::Null);
    match paths["log_dir"].as_str() {
        Some(log_dir) => {
            let up_dir = Path::new(log_dir).join("up").join(&cx.p);
            if let Err(e) = std::fs::remove_dir_all(&up_dir)
                && e.kind() != std::io::ErrorKind::NotFound
            {
                return Err(format!("cannot remove {}: {e}", up_dir.display()));
            }
            // And `up/` itself if this run made it: only when it's empty, so
            // the developer's own `lokal up` folders stay.
            let _ = std::fs::remove_dir(Path::new(log_dir).join("up"));
        }
        None => return Err("lokal internal paths has no log_dir".into()),
    }
    res
}

fn pct(v: &mut [f64], p: f64) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    if v.is_empty() {
        return 0.0;
    }
    v[(((p / 100.0) * (v.len() as f64 - 1.0)).round() as usize).min(v.len() - 1)]
}

#[derive(Default)]
struct Perf {
    lines: Vec<String>,
    fails: Vec<String>,
}

impl Perf {
    fn target(&mut self, name: &str, value: f64, limit: f64, unit: &str) {
        let ok = value < limit;
        self.lines.push(format!(
            "{name} {value:.2}{unit} (< {limit}{unit}) {}",
            if ok { "ok" } else { "FAIL" }
        ));
        if !ok {
            self.fails.push(name.to_string());
        }
    }
}

fn l23(cx: &mut Ctx) -> Check {
    let mut pf = Perf::default();
    // service add wall time p95 of 10, and the first request right after succeeds
    let mut adds = vec![];
    let mut first_try = true;
    for i in 0..10 {
        let t = Instant::now();
        let o = cx.lokal(&["service", "add", &format!("perf{i}"), &cx.a.to_string()]);
        adds.push(t.elapsed().as_secs_f64() * 1000.0);
        if !o.status.success() {
            return Err(format!("service add perf{i}: {}", out_str(&o)));
        }
        if cx
            .get_marker(
                &format!("https://{}/", cx.fqdn(&format!("perf{i}"))),
                "MARKER_A",
            )
            .is_err()
        {
            first_try = false;
        }
    }
    pf.target("service add p95", pct(&mut adds, 95.0), 300.0, "ms");
    pf.lines.push(format!(
        "first request after add: {}",
        if first_try { "ok" } else { "FAIL" }
    ));
    if !first_try {
        pf.fails.push("first request".into());
    }
    // name lookup
    let (_, v) = cx.check(&["resolve", &cx.fqdn("perf9")]);
    pf.target(
        "lookup first",
        v["first_ms"].as_f64().unwrap_or(999.0),
        20.0,
        "ms",
    );
    pf.target(
        "lookup repeat",
        v["repeat_ms"].as_f64().unwrap_or(999.0),
        5.0,
        "ms",
    );
    // proxy overhead (keep-alive)
    let (_, via) = cx.check(&[
        "bench",
        &format!("https://{}/", cx.fqdn("perf0")),
        "-n",
        "500",
    ]);
    let (_, direct) = cx.check(&["bench", &format!("http://127.0.0.1:{}/", cx.a), "-n", "500"]);
    let overhead =
        via["p95_ms"].as_f64().unwrap_or(999.0) - direct["p95_ms"].as_f64().unwrap_or(0.0);
    pf.target("proxy overhead p95", overhead, 2.0, "ms");
    // resumed TLS
    let (_, t) = cx.check(&["tls", &cx.fqdn("perf0"), "-n", "50"]);
    pf.target(
        "resumed TLS p95",
        t["resumed_p95_ms"].as_f64().unwrap_or(999.0),
        10.0,
        "ms",
    );
    // list / status
    for (name, args) in [
        ("lokal list", vec!["list"]),
        ("lokal status", vec!["status"]),
    ] {
        let mut v = vec![];
        for _ in 0..10 {
            let t = Instant::now();
            cx.lokal(&args);
            v.push(t.elapsed().as_secs_f64() * 1000.0);
        }
        pf.target(&format!("{name} p95"), pct(&mut v, 95.0), 30.0, "ms");
    }
    // 20 services: RSS
    for i in 10..20 {
        cx.lokal(&["service", "add", &format!("perf{i}"), &cx.a.to_string()]);
    }
    let pid = cx.daemon_pid().ok_or("no daemon")?;
    let rss_kb: f64 = String::from_utf8_lossy(
        &run(
            Path::new("/bin/ps"),
            &["-o", "rss=", "-p", &pid.to_string()],
        )
        .stdout,
    )
    .trim()
    .parse()
    .unwrap_or(1e9);
    let svc_count = serde_json::from_slice::<Value>(&cx.lokal(&["list", "--json"]).stdout)
        .ok()
        .and_then(|v| {
            v["projects"].as_array().map(|ps| {
                ps.iter()
                    .map(|p| p["services"].as_array().map(|s| s.len()).unwrap_or(0))
                    .sum::<usize>()
            })
        })
        .unwrap_or(0);
    pf.target(
        &format!("RSS with {svc_count} services"),
        rss_kb / 1024.0,
        30.0,
        "MB",
    );
    // idle CPU over 10s
    let cpu = |pid: u64| -> f64 {
        let s = String::from_utf8_lossy(
            &run(
                Path::new("/bin/ps"),
                &["-o", "time=", "-p", &pid.to_string()],
            )
            .stdout,
        )
        .trim()
        .to_string();
        // [[dd-]hh:]mm:ss.cc
        let mut secs = 0.0;
        for part in s.split(':') {
            secs = secs * 60.0 + part.parse::<f64>().unwrap_or(0.0);
        }
        secs
    };
    let c0 = cpu(pid);
    std::thread::sleep(Duration::from_secs(10));
    let c1 = cpu(pid);
    pf.target("idle CPU over 10s", (c1 - c0) / 10.0 * 100.0, 0.5, "%");
    // restart: daemon start to serving all names. Let the daemon be up >1s
    // first: launchd's ThrottleInterval (1s) delays respawning younger jobs.
    let t = Instant::now();
    let o = cx.lokal(&["restart"]);
    let restart_ms = t.elapsed().as_secs_f64() * 1000.0;
    if !o.status.success() {
        return Err(format!("restart: {}", out_str(&o)));
    }
    pf.target("restart → serving all names", restart_ms, 500.0, "ms");
    let ok_after = cx.l06("MARKER_D").is_ok() && cx.resolve_ok(&cx.fqdn("perf19")).is_ok();
    pf.lines.push(format!(
        "all names served after restart: {}",
        if ok_after { "ok" } else { "FAIL" }
    ));
    if !ok_after {
        pf.fails.push("served after restart".into());
    }
    for i in 0..20 {
        cx.lokal(&["service", "remove", &format!("perf{i}")]);
    }
    if pf.fails.is_empty() {
        Ok(pf.lines.join("\n"))
    } else {
        Err(pf.lines.join("\n"))
    }
}

fn l24(cx: &mut Ctx, snapshot: &Value) -> Check {
    let mut names: Vec<String> = ["api", "web", "down", "@"]
        .iter()
        .map(|s| cx.fqdn(s))
        .collect();
    names.push(cx.mdns_fqdn());
    let o = cx.lokal(&["project", "remove", &cx.mp(), "--force"]);
    if !o.status.success() {
        return Err(out_str(&o));
    }
    let o = cx.lokal(&["project", "remove", &cx.p.clone(), "--force"]);
    if !o.status.success() {
        return Err(out_str(&o));
    }
    cx.project_created = false;
    // .local names stop resolving; .lokal names are no longer served (TLS).
    let local: Vec<String> = names
        .iter()
        .filter(|n| n.ends_with(".local"))
        .cloned()
        .collect();
    cx.all_unresolvable(&local)?;
    for n in names.iter().filter(|n| n.ends_with(".lokal")) {
        cx.forced_tls_fails(n)?;
    }
    cx.restore_current()?;
    let now: Value =
        serde_json::from_slice(&cx.lokal(&["list", "--json"]).stdout).map_err(|e| e.to_string())?;
    if &now != snapshot {
        return Err(format!(
            "lokal list --json differs from the pre-test snapshot:\n{now}\nvs\n{snapshot}"
        ));
    }
    Ok("removed; names gone; list equals snapshot".into())
}

/// Minimal WebDriver call to safaridriver.
fn wd(port: u16, method: &str, path: &str, body: Option<&Value>) -> Result<Value, String> {
    let mut s = TcpStream::connect(("127.0.0.1", port)).map_err(|e| e.to_string())?;
    s.set_read_timeout(Some(Duration::from_secs(60))).ok();
    let b = body.map(|b| b.to_string()).unwrap_or_default();
    write!(
        s,
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{b}",
        b.len()
    )
    .map_err(|e| e.to_string())?;
    let mut resp = String::new();
    s.read_to_string(&mut resp).map_err(|e| e.to_string())?;
    let body = resp.split_once("\r\n\r\n").map(|x| x.1).unwrap_or("");
    // Body may be chunked; take the JSON object.
    let start = body
        .find('{')
        .ok_or_else(|| format!("bad WebDriver reply: {resp}"))?;
    let end = body.rfind('}').ok_or("bad WebDriver reply")?;
    serde_json::from_str(&body[start..=end]).map_err(|e| e.to_string())
}

fn l25(cx: &mut Ctx) -> Check {
    if !Path::new("/usr/bin/safaridriver").exists() {
        return Ok("SKIP safaridriver not found".into());
    }
    let port = free_port();
    let mut drv = Command::new("/usr/bin/safaridriver")
        .args(["-p", &port.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    let res = (|| {
        poll(Duration::from_secs(5), || {
            TcpStream::connect(("127.0.0.1", port)).is_ok()
        })
        .ok_or("safaridriver did not start")?;
        let sess = wd(
            port,
            "POST",
            "/session",
            Some(&json!({"capabilities": {"alwaysMatch": {"browserName": "safari"}}})),
        )?;
        let Some(id) = sess["value"]["sessionId"].as_str().map(String::from) else {
            // Remote automation disabled, or Safari can't be driven on this
            // Mac: safaridriver refuses before any page (or lokal) is involved.
            let msg = sess["value"]["message"].as_str().unwrap_or("").to_string();
            return Ok(format!(
                "SKIP safaridriver could not create a session ({})",
                msg.chars().take(160).collect::<String>()
            ));
        };
        if !cx.project_created {
            return Err("the test project is gone: L25 must run before L24's cleanup".into());
        }
        let url = format!("https://{}/", cx.fqdn("@"));
        let r = (|| {
            wd(
                port,
                "POST",
                &format!("/session/{id}/url"),
                Some(&json!({ "url": url })),
            )?;
            let src = wd(port, "GET", &format!("/session/{id}/source"), None)?;
            let text = src["value"].as_str().unwrap_or("");
            if text.contains("MARKER_A") {
                Ok(format!("Safari loaded {url} → MARKER_A"))
            } else {
                Err(format!("Safari page: {}", &text[..text.len().min(300)]))
            }
        })();
        let _ = wd(port, "DELETE", &format!("/session/{id}"), None);
        r
    })();
    let _ = drv.kill();
    let _ = drv.wait();
    res
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let full = args.iter().any(|a| a == "--full");
    let curl_cacert = args.iter().any(|a| a == "--curl-cacert");
    let exe = std::env::current_exe().unwrap();
    let bin_dir = exe.parent().unwrap().to_path_buf();
    let repo = bin_dir
        .parent()
        .and_then(|p| p.parent())
        .unwrap()
        .to_path_buf();
    let lokal = args
        .iter()
        .position(|a| a == "--lokal")
        .and_then(|i| args.get(i + 1))
        .map(PathBuf::from)
        .unwrap_or_else(|| bin_dir.join("lokal"));
    let tmp = std::env::temp_dir().join(format!("lokal-live-{}", rand_hex()));
    std::fs::create_dir_all(&tmp).unwrap();
    let mut cx = Ctx {
        bin_dir,
        lokal,
        p: format!("lt-{}", rand_hex()),
        a: 0,
        b: 0,
        c: 0,
        d: 0,
        upstreams: vec![],
        results: vec![],
        tmp,
        curl_cacert,
        project_created: false,
        prev_current: None,
    };
    let started = Instant::now();
    let snapshot: Value =
        serde_json::from_slice(&cx.lokal(&["list", "--json"]).stdout).unwrap_or(Value::Null);
    cx.prev_current = current_project(&snapshot);
    println!(
        "live test: project {} ({})",
        cx.p,
        String::from_utf8_lossy(&cx.lokal(&["--version"]).stdout).trim()
    );

    let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        cx.step("L01", "Setup is idempotent without sudo", l01);
        cx.step("L02", "Baseline doctor", l02);
        cx.step("L03", "Upstreams", l03);
        cx.step("L04", "Create", l04);
        cx.step("L05", "Resolution, first try", l05);
        cx.step("L06", "HTTPS via system trust", |cx| cx.l06("MARKER_A"));
        cx.step("L07", "Real browser (Chrome headless)", l07);
        cx.step("L08", "macOS trust evaluation", l08);
        cx.step("L09", "Apex", |cx| {
            cx.get_marker(&format!("https://{}/", cx.fqdn("@")), "MARKER_A")
        });
        cx.step("L10", "Redirect", l10);
        cx.step("L11", "WebSocket", |cx| {
            let (ok, v) = cx.check(&["ws", &format!("wss://{}/ws", cx.fqdn("api"))]);
            if ok {
                Ok(v["echo"].to_string())
            } else {
                Err(v.to_string())
            }
        });
        cx.step("L12", "SSE", |cx| {
            let (ok, v) = cx.check(&["sse", &format!("https://{}/sse", cx.fqdn("api"))]);
            if ok {
                Ok(format!("lags {}", v["lags_ms"]))
            } else {
                Err(v.to_string())
            }
        });
        cx.step("L13", "Upstream down", l13);
        cx.step("L14", "Unknown names", l14);
        cx.step("L15", "Hot add", l15);
        cx.step("L16", "Hot update", l16);
        cx.step("L17", "Hot remove", l17);
        cx.step("L18", "Crash recovery", l18);
        cx.step("L19", "Stop and start", l19);
        cx.step("L20", "Upgrade", l20);
        cx.step("L21", "lokal env", l21);
        cx.step("L22", "Full doctor", l22);
        cx.step("L26", "lokal run", l26);
        cx.step("L27", "lokal up", l27);
        if full {
            cx.step("L23", "Performance", l23);
        } else {
            let t = Instant::now();
            cx.record("L23", "Performance", t, Ok("SKIP only with --full".into()));
        }
        // L25 loads the test project, so it runs before L24 removes it.
        // L24 stays last: it cleans up and checks nothing else changed.
        cx.step("L25", "Safari (optional)", l25);
        let snap = snapshot.clone();
        cx.step("L24", "Cleanup and isolation", move |cx| l24(cx, &snap));
    }));
    if res.is_err() {
        let t = Instant::now();
        cx.record("ERR", "runner panicked", t, Err("see stderr".into()));
    }
    cx.cleanup();

    let pass = cx.results.iter().filter(|r| r.status == St::Pass).count();
    let fail = cx.results.iter().filter(|r| r.status == St::Fail).count();
    let skip = cx.results.iter().filter(|r| r.status == St::Skip).count();
    println!(
        "\n{pass} PASS / {fail} FAIL / {skip} SKIP in {:.1}s",
        started.elapsed().as_secs_f64()
    );
    let os =
        String::from_utf8_lossy(&run(Path::new("/usr/bin/sw_vers"), &["-productVersion"]).stdout)
            .trim()
            .to_string();
    let report = json!({
        "schema": 1,
        "macos": os,
        "lokal": String::from_utf8_lossy(&cx.lokal(&["--version"]).stdout).trim(),
        "project": cx.p,
        "full": full,
        "curl_cacert": curl_cacert,
        "duration_s": ms(started.elapsed()) / 1000.0,
        "summary": { "pass": pass, "fail": fail, "skip": skip },
        "results": cx.results.iter().map(|r| json!({
            "id": r.id, "description": r.desc,
            "status": match r.status { St::Pass => "PASS", St::Fail => "FAIL", St::Skip => "SKIP" },
            "ms": r.ms, "detail": r.detail,
        })).collect::<Vec<_>>(),
    });
    let path = repo.join("target/live-test-report.json");
    let _ = std::fs::write(&path, serde_json::to_string_pretty(&report).unwrap());
    println!("report: {}", path.display());
    std::process::exit(if fail > 0 { 1 } else { 0 });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression (2026-10-17): the run left the developer's current project
    /// unset, because the test project became current and was then removed.
    #[test]
    fn remembers_the_developers_current_project() {
        let list = json!({"schema": 1, "current_project": "myproject", "projects": []});
        assert_eq!(current_project(&list).as_deref(), Some("myproject"));
        let none = json!({"schema": 1, "current_project": null, "projects": []});
        assert_eq!(current_project(&none), None);
        assert_eq!(current_project(&Value::Null), None);
    }
}

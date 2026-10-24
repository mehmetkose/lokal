//! After `lokal uninstall` (gate G2): asserts no lokal traces are left,
//! except state.toml and the CA files, which a plain uninstall keeps.
//! Read-only.

use std::path::PathBuf;
use std::process::Command;

fn run(cmd: &str, args: &[&str]) -> (bool, String) {
    match Command::new(cmd).args(args).output() {
        Ok(o) => (
            o.status.success(),
            format!(
                "{}{}",
                String::from_utf8_lossy(&o.stdout),
                String::from_utf8_lossy(&o.stderr)
            ),
        ),
        Err(e) => (false, e.to_string()),
    }
}

fn main() {
    let home = PathBuf::from(std::env::var("HOME").expect("HOME"));
    let base = home.join("Library/Application Support/lokal");
    let uid = unsafe { libc::getuid() };
    let mut fails = 0;
    let mut check = |ok: bool, what: &str, detail: String| {
        println!(
            "{} {what}{}",
            if ok { "PASS" } else { "FAIL" },
            if detail.is_empty() {
                String::new()
            } else {
                format!(" [{}]", detail.trim())
            }
        );
        if !ok {
            fails += 1;
        }
    };

    let (found, out) = run(
        "/usr/bin/security",
        &[
            "find-certificate",
            "-c",
            "lokal development CA",
            "/Library/Keychains/System.keychain",
        ],
    );
    check(
        !found,
        "CA is not in the System keychain",
        if found { out } else { String::new() },
    );

    let plist = home.join("Library/LaunchAgents/dev.lokal.daemon.plist");
    check(
        !plist.exists(),
        "LaunchAgent plist is gone",
        plist.display().to_string(),
    );

    let (loaded, _) = run(
        "/bin/launchctl",
        &["print", &format!("gui/{uid}/dev.lokal.daemon")],
    );
    check(!loaded, "dev.lokal.daemon is not loaded", String::new());

    let (_, lsof) = run("/usr/sbin/lsof", &["-nP", "-iTCP:443", "-sTCP:LISTEN"]);
    let lokal_listening: Vec<&str> = lsof.lines().filter(|l| l.starts_with("lokal")).collect();
    check(
        lokal_listening.is_empty(),
        "nothing from lokal listens on 443",
        lokal_listening.join("; "),
    );

    let resolver = PathBuf::from("/etc/resolver/lokal");
    check(
        !resolver.exists(),
        "/etc/resolver/lokal is gone",
        resolver.display().to_string(),
    );

    let bin = base.join("bin/lokal");

    check(
        !bin.exists(),
        "daemon binary is gone",
        bin.display().to_string(),
    );

    let (_, list) = run("/bin/launchctl", &["list"]);
    let verify_loaded: Vec<&str> = list
        .lines()
        .filter(|l| l.contains("dev.lokal.verify."))
        .collect();
    let verify_plists: Vec<String> = std::fs::read_dir(home.join("Library/LaunchAgents"))
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.starts_with("dev.lokal.verify."))
                .collect()
        })
        .unwrap_or_default();
    check(
        verify_loaded.is_empty() && verify_plists.is_empty(),
        "no dev.lokal.verify.* agents left",
        format!("{verify_loaded:?} {verify_plists:?}"),
    );

    for f in ["state.toml", "ca/root.pem", "ca/root-key.pem"] {
        let p = base.join(f);
        check(p.exists(), &format!("{f} is kept"), p.display().to_string());
    }

    if fails > 0 {
        println!("{fails} check(s) failed");
        std::process::exit(1);
    }
    println!("clean");
}

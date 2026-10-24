//! Keychain lookup behind `TrustStore::lokal_cas_pem` (SPEC §4.7): uninstall
//! and setup use it to find the CA `install.json` records after its files are
//! gone. Read-only; kept as documentation.
//!
//! Checks that `security find-certificate -a -Z -p -c <prefix> <keychain>`:
//!   1. prints one PEM block per match, preceded by `SHA-1 hash:` lines;
//!   2. the SHA-1 lokal computes from each block's DER equals that line;
//!   3. exits 0 and prints nothing when no certificate matches.
//!
//! Usage: verify_keychain_lookup [prefix] [keychain]
//!   (defaults: "lokal development CA", /Library/Keychains/System.keychain)
//!
//! Measured on macOS 27.0.1 (26A434), 2026-10-23, with one lokal CA trusted:
//!   match:    exit 0, 1 PEM block, 1 SHA-1 line, computed SHA-1 equals it
//!   no match: exit 0, 0 bytes on stdout, 0 bytes on stderr

use std::process::Command;

fn find(prefix: &str, keychain: &str) -> std::process::Output {
    Command::new("/usr/bin/security")
        .args(["find-certificate", "-a", "-Z", "-p", "-c", prefix, keychain])
        .output()
        .expect("run security")
}

fn sha1_hex(der: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA1_FOR_LEGACY_USE_ONLY, der)
        .as_ref()
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect()
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let prefix = args
        .first()
        .map(String::as_str)
        .unwrap_or("lokal development CA");
    let keychain = args
        .get(1)
        .map(String::as_str)
        .unwrap_or("/Library/Keychains/System.keychain");
    let sw = Command::new("/usr/bin/sw_vers").output().expect("sw_vers");
    print!("{}", String::from_utf8_lossy(&sw.stdout));

    let out = find(prefix, keychain);
    let text = String::from_utf8_lossy(&out.stdout);
    let printed: Vec<String> = text
        .lines()
        .filter_map(|l| {
            l.trim()
                .strip_prefix("SHA-1 hash:")
                .map(|h| h.trim().to_string())
        })
        .collect();
    let computed: Vec<String> = rustls_pemfile::certs(&mut out.stdout.as_slice())
        .map(|c| sha1_hex(&c.expect("valid PEM")))
        .collect();
    println!(
        "match:    exit {:?}, {} PEM block(s), {} SHA-1 line(s), computed == printed: {}",
        out.status.code(),
        computed.len(),
        printed.len(),
        computed == printed
    );

    let none = find("lokal-verify-no-such-ca-7f3a", keychain);
    println!(
        "no match: exit {:?}, {} bytes on stdout, {} bytes on stderr",
        none.status.code(),
        none.stdout.len(),
        none.stderr.len()
    );
}

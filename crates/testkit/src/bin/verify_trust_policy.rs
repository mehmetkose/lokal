//! Trust-settings policy scope behind `TrustStore::plan_trust`,
//! `plan_retrust` and `trust_too_broad` (SPEC §6.3, §12 V8). Audit lead 1:
//! `add-trusted-cert` without `-p` trusts the CA for every policy (S/MIME,
//! code signing, …), not just TLS.
//!
//! Uses a throwaway CA with lokal's name constraints, named
//! "lokal verify CA <id>" so lokal never mistakes it for its own CA. Its
//! private key never touches the disk: it lives in memory only while the test
//! leaves are signed, so nothing can sign with the CA while it's trusted.
//!
//! Checks:
//!   1. trusted with no policy (lokal before the fix): which policies verify;
//!   2. trusted again with `-p ssl` over that entry: does it replace the entry
//!      (only ssl verifies) or add to it (everything still verifies)?
//!   3. which unprivileged probe tells the two apart.
//!
//! Usage:
//!   verify_trust_policy prepare      writes the CA and leaves, prints the steps
//!   verify_trust_policy probe <dir>  read-only: verify-cert per policy, plus
//!                                    the CA's dump-trust-settings entry
//!
//! The trust steps need sudo, so the developer runs them (AGENTS.md §1);
//! `prepare` prints them.
//!
//! Measured on macOS 27.0.1 (26A434), 2026-10-24 (SPEC §12 V8). The CA as an
//! explicit anchor (`-r`) passes every case, so the leaves are well-formed.
//!
//!   probe                     untrusted   step 1 (no -p)   step 2 (-p ssl)
//!   ssl, server leaf          fails       TRUSTED          TRUSTED
//!   ssl -C, client leaf       fails       TRUSTED          fails
//!   basic, server leaf        fails       TRUSTED          fails
//!   smime, smime leaf         fails       TRUSTED          fails
//!   codeSign, codesign leaf   fails       TRUSTED          fails
//!   dump-trust-settings -d    no entry    0 settings       1 setting, Policy OID SSL
//!
//! So: `-p ssl` limits trust to TLS server evaluation (even TLS client auth
//! is no longer trusted); step 2 *replaced* step 1's entry rather than adding
//! to it, so narrowing an old entry is one `add-trusted-cert -p ssl`; and
//! `-p basic` tells the two apart without privileges. Each `add-trusted-cert`
//! asked for the sudo password once and showed one macOS dialog.

use std::fs;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime};

use rcgen::{
    BasicConstraints, CertificateParams, CidrSubnet, DistinguishedName, DnType,
    ExtendedKeyUsagePurpose, GeneralSubtree, IsCa, Issuer, KeyPair, KeyUsagePurpose,
    NameConstraints, SanType,
};

const SECURITY: &str = "/usr/bin/security";
const SYSTEM_KEYCHAIN: &str = "/Library/Keychains/System.keychain";

fn rand_hex() -> String {
    let mut b = [0u8; 4];
    ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut b).unwrap();
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn sha1_hex(der: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA1_FOR_LEGACY_USE_ONLY, der)
        .as_ref()
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect()
}

/// The same name constraints as lokal's CA (SPEC §6.1).
fn lokal_name_constraints() -> NameConstraints {
    NameConstraints {
        permitted_subtrees: vec![
            GeneralSubtree::DnsName("local".into()),
            GeneralSubtree::DnsName("lokal".into()),
        ],
        excluded_subtrees: vec![
            GeneralSubtree::IpAddress(CidrSubnet::V4([0; 4], [0; 4])),
            GeneralSubtree::IpAddress(CidrSubnet::V6([0; 16], [0; 16])),
        ],
    }
}

fn leaf_params(cn: &str, sans: Vec<SanType>, eku: ExtendedKeyUsagePurpose) -> CertificateParams {
    let now = SystemTime::now();
    let mut p = CertificateParams::default();
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, cn);
    p.distinguished_name = dn;
    p.subject_alt_names = sans;
    p.is_ca = IsCa::ExplicitNoCa;
    p.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    p.extended_key_usages = vec![eku];
    p.not_before = (now - Duration::from_secs(3600)).into();
    p.not_after = (now + Duration::from_secs(86_400)).into();
    p.use_authority_key_identifier_extension = true;
    p
}

fn prepare() {
    let id = rand_hex();
    let dir = std::env::temp_dir().join(format!("lokal-verify-trust-{id}"));
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&dir)
        .expect("create dir");

    let cn = format!("lokal verify CA {id}");
    let now = SystemTime::now();
    let mut ca = CertificateParams::default();
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, cn.clone());
    dn.push(DnType::OrganizationName, "lokal verify");
    ca.distinguished_name = dn;
    ca.not_before = (now - Duration::from_secs(3600)).into();
    ca.not_after = (now + Duration::from_secs(86_400)).into();
    ca.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
    ca.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    ca.name_constraints = Some(lokal_name_constraints());
    let ca_key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
    let ca_cert = ca.self_signed(&ca_key).unwrap();
    let sha1 = sha1_hex(ca_cert.der());
    fs::write(dir.join("ca.pem"), ca_cert.pem()).unwrap();

    let host = format!("v2-{id}.local");
    let email = format!("v2-{id}@example.com");
    let issuer = Issuer::new(ca, ca_key);
    let leaves = [
        (
            "server",
            leaf_params(
                &host,
                vec![SanType::DnsName(host.as_str().try_into().unwrap())],
                ExtendedKeyUsagePurpose::ServerAuth,
            ),
        ),
        (
            "client",
            leaf_params(
                &host,
                vec![SanType::DnsName(host.as_str().try_into().unwrap())],
                ExtendedKeyUsagePurpose::ClientAuth,
            ),
        ),
        (
            "smime",
            leaf_params(
                &email,
                vec![SanType::Rfc822Name(email.as_str().try_into().unwrap())],
                ExtendedKeyUsagePurpose::EmailProtection,
            ),
        ),
        (
            "codesign",
            leaf_params(
                &format!("lokal verify code signing {id}"),
                vec![],
                ExtendedKeyUsagePurpose::CodeSigning,
            ),
        ),
    ];
    for (name, params) in leaves {
        let key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
        let cert = params.signed_by(&key, &issuer).unwrap();
        fs::write(dir.join(format!("{name}.pem")), cert.pem()).unwrap();
    }
    // The CA key goes out of scope here and was never written.
    drop(issuer);
    let info = serde_json::json!({ "cn": cn, "sha1": sha1, "host": host, "email": email });
    fs::write(dir.join("info.json"), info.to_string()).unwrap();

    let me = std::env::current_exe().unwrap();
    let ca_pem = dir.join("ca.pem");
    let (d, c) = (dir.display(), ca_pem.display());
    println!("Throwaway CA: {cn}  (SHA-1 {sha1})");
    println!("Files: {d}  (the CA key was never written)\n");
    println!("1. Trust with no policy (lokal before the fix), then probe:");
    println!("   sudo {SECURITY} add-trusted-cert -d -r trustRoot -k {SYSTEM_KEYCHAIN} {c}");
    println!("   {} probe {d}\n", me.display());
    println!("2. Trust again with -p ssl over that entry, then probe:");
    println!("   sudo {SECURITY} add-trusted-cert -d -r trustRoot -p ssl -k {SYSTEM_KEYCHAIN} {c}");
    println!("   {} probe {d}\n", me.display());
    println!("3. Remove it, then probe (everything should fail):");
    println!("   sudo {SECURITY} remove-trusted-cert -d {c}");
    println!("   sudo {SECURITY} delete-certificate -Z {sha1} {SYSTEM_KEYCHAIN}");
    println!("   {} probe {d}", me.display());
}

fn verify(args: &[&str]) -> String {
    let out = Command::new(SECURITY)
        .arg("verify-cert")
        .args(args)
        .output()
        .expect("run security verify-cert");
    if out.status.success() {
        return "TRUSTED".into();
    }
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let why = text
        .lines()
        .find_map(|l| l.trim().strip_prefix("Cert Verify Result:"))
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "failed".into());
    format!("not trusted ({why})")
}

/// The CA's entry in the admin-domain trust settings, as printed by
/// `security dump-trust-settings -d` (diagnostics only: entries are named by
/// common name, and the text format isn't a stable interface).
fn trust_settings_entry(cn: &str) -> String {
    let out = Command::new(SECURITY)
        .args(["dump-trust-settings", "-d"])
        .output()
        .expect("run security dump-trust-settings");
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let mut entry = Vec::new();
    let mut inside = false;
    for line in text.lines() {
        if line.starts_with("Cert ") {
            inside = line.ends_with(&format!(": {cn}"));
        }
        if inside {
            entry.push(line.to_string());
        }
    }
    if entry.is_empty() {
        "(no entry)".into()
    } else {
        entry.join("\n")
    }
}

fn probe(dir: &Path) {
    let info: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(dir.join("info.json")).expect("info.json"))
            .expect("valid info.json");
    let s = |k: &str| info[k].as_str().unwrap().to_string();
    let (cn, host, email) = (s("cn"), s("host"), s("email"));
    let p = |f: &str| dir.join(f).display().to_string();
    let ca = p("ca.pem");
    // The CA is passed as a chain certificate (-c), never as an anchor (-r),
    // so a leaf verifies only if the keychain trusts the CA (as lokal's
    // TrustStore::is_trusted does).
    let chain = |leaf: &str| {
        vec![
            "-c".to_string(),
            p(leaf),
            "-c".into(),
            ca.clone(),
            "-L".into(),
        ]
    };
    let cases: Vec<(&str, Vec<String>)> = vec![
        (
            "ssl, server leaf",
            [
                chain("server.pem"),
                vec!["-p".into(), "ssl".into(), "-n".into(), host.clone()],
            ]
            .concat(),
        ),
        (
            "ssl -C, client leaf",
            [
                chain("client.pem"),
                vec!["-p".into(), "ssl".into(), "-C".into()],
            ]
            .concat(),
        ),
        (
            "basic, server leaf",
            [chain("server.pem"), vec!["-p".into(), "basic".into()]].concat(),
        ),
        (
            "smime, smime leaf",
            [
                chain("smime.pem"),
                vec!["-p".into(), "smime".into(), "-n".into(), email.clone()],
            ]
            .concat(),
        ),
        (
            "codeSign, codesign leaf",
            [chain("codesign.pem"), vec!["-p".into(), "codeSign".into()]].concat(),
        ),
    ];
    let sw = Command::new("/usr/bin/sw_vers").output().expect("sw_vers");
    print!("{}", String::from_utf8_lossy(&sw.stdout));
    println!("CA: {cn}");
    for (label, args) in cases {
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        println!("  {label:<26} {}", verify(&refs));
    }
    println!("dump-trust-settings -d:\n{}", trust_settings_entry(&cn));
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("prepare") => prepare(),
        Some("probe") => probe(&PathBuf::from(args.get(1).expect("usage: probe <dir>"))),
        _ => {
            eprintln!("usage: verify_trust_policy prepare | probe <dir>");
            std::process::exit(2);
        }
    }
}

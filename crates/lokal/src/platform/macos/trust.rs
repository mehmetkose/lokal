//! Keychain trust (SPEC §6.3). Trusting/untrusting needs root and runs
//! only from `setup`/`uninstall`; checking is unprivileged
//! (`security verify-cert`, local only). The CA is trusted for the SSL policy
//! only (`-p ssl`), not for S/MIME, code signing or anything else (§12 V8).

use anyhow::{Context, Result};

use super::firefox;
use crate::ca::CaCert;
use crate::platform::{PrivilegedCommand, TrustStatus, TrustStore};
use crate::process;

const SECURITY: &str = "/usr/bin/security";
const SYSTEM_KEYCHAIN: &str = "/Library/Keychains/System.keychain";
const ROOTS_KEYCHAIN: &str = "/System/Library/Keychains/SystemRootCertificates.keychain";
pub const TRUSTCHECK_NAME: &str = "lokal-trustcheck.local";

pub struct Keychain;

impl TrustStore for Keychain {
    fn unavailable(&self) -> Option<String> {
        None
    }

    fn plan_trust(&self, ca: &CaCert) -> Vec<PrivilegedCommand> {
        vec![PrivilegedCommand {
            program: SECURITY.into(),
            args: vec![
                "add-trusted-cert".into(),
                "-d".into(),
                "-r".into(),
                "trustRoot".into(),
                // SSL only. Run over an entry with no policy, this replaces
                // it, which is how setup narrows an older lokal's trust (V8).
                "-p".into(),
                "ssl".into(),
                "-k".into(),
                SYSTEM_KEYCHAIN.into(),
                ca.path.display().to_string(),
            ],
        }]
    }

    fn plan_untrust(&self, ca: &CaCert) -> Vec<PrivilegedCommand> {
        vec![
            PrivilegedCommand {
                program: SECURITY.into(),
                args: vec![
                    "remove-trusted-cert".into(),
                    "-d".into(),
                    ca.path.display().to_string(),
                ],
            },
            PrivilegedCommand {
                program: SECURITY.into(),
                args: vec![
                    "delete-certificate".into(),
                    "-Z".into(),
                    ca.sha1.to_uppercase(),
                    SYSTEM_KEYCHAIN.into(),
                ],
            },
        ]
    }

    fn is_trusted(&self, ca: &CaCert) -> Result<TrustStatus> {
        let out = verify_trustcheck_leaf(ca, &["-p", "ssl", "-s", TRUSTCHECK_NAME])?;
        if out.success() {
            Ok(TrustStatus::Trusted)
        } else {
            let text = format!("{}\n{}", out.stdout, out.stderr);
            let why = text
                .lines()
                .find_map(|l| l.trim().strip_prefix("Cert Verify Result:"))
                .map(|s| s.trim().to_string())
                .unwrap_or_else(|| "not trusted".into());
            Ok(TrustStatus::NotTrusted(why))
        }
    }

    fn trust_too_broad(&self, ca: &CaCert) -> Result<bool> {
        // A root trusted with `-p ssl` verifies for the SSL policy only. One
        // trusted with no policy (lokal before 0.1.0, or "Always Trust" in
        // Keychain Access) also verifies for the basic X.509 policy (V8).
        Ok(verify_trustcheck_leaf(ca, &["-p", "basic"])?.success())
    }

    fn is_present(&self, ca: &CaCert) -> Result<bool> {
        let out = process::run(
            SECURITY,
            &[
                "find-certificate",
                "-a",
                "-Z",
                "-c",
                crate::ca::CA_CN_PREFIX,
                SYSTEM_KEYCHAIN,
            ],
        )?;
        let want = ca.sha1.to_uppercase();
        Ok(out
            .stdout
            .lines()
            .any(|l| l.trim().strip_prefix("SHA-1 hash:").map(str::trim) == Some(want.as_str())))
    }

    fn lokal_cas_pem(&self) -> Result<String> {
        // `-a` with no match exits 0 and prints nothing.
        Ok(process::run_ok(
            SECURITY,
            &[
                "find-certificate",
                "-a",
                "-p",
                "-c",
                crate::ca::CA_CN_PREFIX,
                SYSTEM_KEYCHAIN,
            ],
        )?
        .stdout)
    }

    fn apply_user_extras(&self, _ca: &CaCert) -> Result<Vec<String>> {
        firefox::enable_enterprise_roots()
    }

    fn remove_user_extras(&self) -> Result<Vec<String>> {
        firefox::remove_pref()
    }

    fn user_extras_status(&self) -> Option<(bool, String)> {
        firefox::status()
    }

    fn system_roots_pem(&self) -> Result<String> {
        Ok(process::run_ok(SECURITY, &["find-certificate", "-a", "-p", ROOTS_KEYCHAIN])?.stdout)
    }
}

/// Evaluates a throwaway leaf for `TRUSTCHECK_NAME`, issued by `ca`, with
/// `policy` (the `-p …` arguments of `security verify-cert`). The root is
/// supplied only as a chain certificate (`-c`), so the leaf verifies only if
/// the keychain trusts the root for that policy. `-r <root>` (SPEC §6.3)
/// would make the root an anchor and pass even when untrusted (see
/// docs/history/PROGRESS.md, Decisions).
fn verify_trustcheck_leaf(ca: &CaCert, policy: &[&str]) -> Result<process::CmdOutput> {
    let issuer = ca.issuer()?;
    let leaf = issuer.issue_pem(TRUSTCHECK_NAME)?;
    let dir = std::env::temp_dir().join(format!("lokal-trust-{}", crate::util::random_hex(10)));
    crate::util::create_private_dir(&dir)?;
    let leaf_path = dir.join("leaf.pem");
    std::fs::write(&leaf_path, leaf).context("cannot write trust-check leaf")?;
    let mut args: Vec<&std::ffi::OsStr> = vec![
        "verify-cert".as_ref(),
        "-c".as_ref(),
        leaf_path.as_os_str(),
        "-c".as_ref(),
        ca.path.as_os_str(),
    ];
    args.extend(policy.iter().map(|a| std::ffi::OsStr::new(*a)));
    args.push("-L".as_ref());
    let out = process::run(SECURITY, &args);
    let _ = std::fs::remove_dir_all(&dir);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::Paths;

    /// Audit lead 1: trust must be limited to the SSL policy.
    #[test]
    fn plan_trust_is_ssl_only() {
        let dir = tempfile::tempdir().unwrap();
        let ca = CaCert::preview(&Paths::sandbox(dir.path().to_path_buf()), "u", "h").unwrap();
        let plan = Keychain.plan_trust(&ca);
        assert_eq!(plan.len(), 1);
        let args = &plan[0].args;
        assert_eq!(args[0], "add-trusted-cert");
        let p = args.iter().position(|a| a == "-p").expect("a -p policy");
        assert_eq!(args[p + 1], "ssl");
        assert_eq!(args.iter().filter(|a| *a == "-p").count(), 1);
        assert_eq!(args.last().unwrap(), &ca.path.display().to_string());
    }
}

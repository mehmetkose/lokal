//! The system trust store (SPEC §6.3). Trusting means a root-owned copy of
//! the CA in the distribution's anchors folder, then its update tool, which
//! rebuilds the bundle that OpenSSL, curl, rustls (rustls-native-certs) and
//! most other programs read. Trusting and untrusting need root and run only
//! from `setup`/`uninstall`; checking reads the bundle, unprivileged. Chrome
//! and Firefox keep their own NSS databases: `nss.rs`, as the user.
//!
//! An anchor in the bundle has no policy scope (no S/MIME or code-signing
//! bits to narrow), so `trust_too_broad` is always false; the CA's own name
//! constraints keep it to `.lokal` and `.local`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, anyhow};

use super::{nss, program};
use crate::ca::CaCert;
use crate::platform::{PrivilegedCommand, TrustStatus, TrustStore};

/// The throwaway leaf `is_trusted` verifies. Inside the CA's name constraints.
const TRUSTCHECK_NAME: &str = "lokal-trustcheck.lokal";
/// lokal's anchor files: `lokal-<sha1>.crt`, one per CA.
const FILE_PREFIX: &str = "lokal-";

/// Where a distribution keeps its anchors and how it rebuilds its bundle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemStore {
    /// The folder of locally added roots.
    anchors: PathBuf,
    /// The tool that rebuilds the bundle from it (no arguments).
    update: PathBuf,
    /// The bundle it writes, which programs read.
    bundle: PathBuf,
}

impl SystemStore {
    /// This machine's store, found by which tool exists, not by the
    /// distribution's name: Debian and Ubuntu's `update-ca-certificates`,
    /// else p11-kit's `update-ca-trust` (Fedora, RHEL, Arch, openSUSE's is
    /// `update-ca-certificates` again). `None` if neither is installed.
    pub fn detect() -> Option<Self> {
        let exists = |p: &str| Path::new(p).exists();
        let first = |ps: &[&str]| ps.iter().copied().find(|p| exists(p)).map(PathBuf::from);
        let debian = first(&[
            "/usr/sbin/update-ca-certificates",
            "/sbin/update-ca-certificates",
        ]);
        if let Some(update) = debian.filter(|_| exists("/usr/local/share/ca-certificates")) {
            return Some(Self {
                anchors: "/usr/local/share/ca-certificates".into(),
                update,
                bundle: first(&[
                    "/etc/ssl/certs/ca-certificates.crt",
                    "/etc/ssl/ca-bundle.pem",
                ])
                .unwrap_or_else(|| "/etc/ssl/certs/ca-certificates.crt".into()),
            });
        }
        let update = first(&["/usr/bin/update-ca-trust", "/usr/sbin/update-ca-trust"])?;
        Some(Self {
            anchors: first(&[
                "/etc/pki/ca-trust/source/anchors",
                "/etc/ca-certificates/trust-source/anchors",
            ])?,
            update,
            bundle: first(&[
                "/etc/pki/tls/certs/ca-bundle.crt",
                "/etc/ssl/certs/ca-certificates.crt",
            ])
            .unwrap_or_else(|| "/etc/pki/tls/certs/ca-bundle.crt".into()),
        })
    }

    /// The anchor file of `ca`. By its SHA-1, so an old CA and a new one
    /// can both be there while setup swaps them.
    fn anchor(&self, ca: &CaCert) -> PathBuf {
        self.anchors
            .join(format!("{FILE_PREFIX}{}.crt", ca.sha1.to_lowercase()))
    }

    fn update(&self) -> PrivilegedCommand {
        PrivilegedCommand {
            program: self.update.display().to_string(),
            args: vec![],
        }
    }

    /// Every anchor file lokal put there.
    fn lokal_files(&self) -> Vec<PathBuf> {
        let Ok(rd) = std::fs::read_dir(&self.anchors) else {
            return vec![];
        };
        let mut v: Vec<PathBuf> = rd
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with(FILE_PREFIX) && n.ends_with(".crt"))
            })
            .collect();
        v.sort();
        v
    }
}

/// The system store, or none: then setup stops before it changes anything.
pub struct SystemTrust(pub Option<SystemStore>);

impl SystemTrust {
    fn store(&self) -> Result<&SystemStore> {
        self.0.as_ref().ok_or_else(|| anyhow!(NO_STORE))
    }
}

const NO_STORE: &str = "no system certificate store: install the ca-certificates package";

impl TrustStore for SystemTrust {
    fn unavailable(&self) -> Option<String> {
        self.0.is_none().then(|| NO_STORE.into())
    }

    fn plan_trust(&self, ca: &CaCert) -> Vec<PrivilegedCommand> {
        let Some(s) = &self.0 else { return vec![] };
        vec![
            PrivilegedCommand {
                program: program(&["/usr/bin/install", "/bin/install"]),
                args: vec![
                    "-m".into(),
                    "0644".into(),
                    ca.path.display().to_string(),
                    s.anchor(ca).display().to_string(),
                ],
            },
            s.update(),
        ]
    }

    fn plan_untrust(&self, ca: &CaCert) -> Vec<PrivilegedCommand> {
        let Some(s) = &self.0 else { return vec![] };
        vec![
            PrivilegedCommand {
                program: program(&["/usr/bin/rm", "/bin/rm"]),
                args: vec!["-f".into(), s.anchor(ca).display().to_string()],
            },
            s.update(),
        ]
    }

    fn is_trusted(&self, ca: &CaCert) -> Result<TrustStatus> {
        let s = self.store()?;
        // The bundle itself, not SSL_CERT_FILE: `lokal env` may point that
        // at lokal's own bundle.pem, which always has the CA.
        let bundle = std::fs::read(&s.bundle)
            .with_context(|| format!("cannot read {}", s.bundle.display()))?;
        verify_leaf(&bundle, ca)
    }

    fn trust_too_broad(&self, _ca: &CaCert) -> Result<bool> {
        Ok(false)
    }

    fn is_present(&self, ca: &CaCert) -> Result<bool> {
        Ok(self.store()?.anchor(ca).is_file())
    }

    fn lokal_cas_pem(&self) -> Result<String> {
        let mut pem = String::new();
        for f in self.store()?.lokal_files() {
            pem.push_str(&std::fs::read_to_string(&f).unwrap_or_default());
            if !pem.ends_with('\n') {
                pem.push('\n');
            }
        }
        Ok(pem)
    }

    fn apply_user_extras(&self, ca: &CaCert) -> Result<Vec<String>> {
        nss::trust(ca)
    }

    fn remove_user_extras(&self) -> Result<Vec<String>> {
        nss::untrust()
    }

    fn user_extras_status(&self) -> Option<(bool, String)> {
        nss::status()
    }

    fn system_roots_pem(&self) -> Result<String> {
        let s = self.store()?;
        std::fs::read_to_string(&s.bundle)
            .with_context(|| format!("cannot read {}", s.bundle.display()))
    }
}

/// Does `bundle` (PEM) make a fresh leaf from `ca` verify? The CA is only
/// the leaf's issuer here, never an anchor of its own, as on macOS.
fn verify_leaf(bundle: &[u8], ca: &CaCert) -> Result<TrustStatus> {
    use rustls::client::WebPkiServerVerifier;
    use rustls::client::danger::ServerCertVerifier;
    use rustls::pki_types::{ServerName, UnixTime};

    let mut roots = rustls::RootCertStore::empty();
    roots.add_parsable_certificates(rustls_pemfile::certs(&mut &bundle[..]).filter_map(|c| c.ok()));
    if roots.is_empty() {
        return Ok(TrustStatus::NotTrusted(
            "the system bundle has no certificates".into(),
        ));
    }
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let verifier =
        WebPkiServerVerifier::builder_with_provider(Arc::new(roots), provider).build()?;
    let leaf = ca.issuer()?.issue(TRUSTCHECK_NAME)?;
    let name = ServerName::try_from(TRUSTCHECK_NAME)?;
    Ok(
        match verifier.verify_server_cert(&leaf.der, &[], &name, &[], UnixTime::now()) {
            Ok(_) => TrustStatus::Trusted,
            Err(e) => TrustStatus::NotTrusted(e.to_string()),
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::Paths;

    fn store(dir: &Path) -> SystemStore {
        SystemStore {
            anchors: dir.join("anchors"),
            update: "/usr/sbin/update-ca-certificates".into(),
            bundle: dir.join("bundle.crt"),
        }
    }

    #[test]
    fn trusts_by_copying_the_ca_and_rebuilding_the_bundle() {
        let dir = tempfile::tempdir().unwrap();
        let ca = CaCert::preview(&Paths::sandbox(dir.path().to_path_buf()), "u", "h").unwrap();
        let t = SystemTrust(Some(store(dir.path())));
        let anchor = dir
            .path()
            .join(format!("anchors/lokal-{}.crt", ca.sha1.to_lowercase()));
        let plan = t.plan_trust(&ca);
        assert_eq!(plan.len(), 2);
        assert!(plan[0].program.ends_with("/install"));
        assert_eq!(
            plan[0].args,
            [
                "-m".to_string(),
                "0644".into(),
                ca.path.display().to_string(),
                anchor.display().to_string()
            ]
        );
        assert_eq!(plan[1].program, "/usr/sbin/update-ca-certificates");
        let un = t.plan_untrust(&ca);
        assert_eq!(un[0].args, ["-f".to_string(), anchor.display().to_string()]);
        assert_eq!(un[1], plan[1]);
        // No store: nothing planned, and setup is told why.
        assert!(SystemTrust(None).plan_trust(&ca).is_empty());
        assert!(SystemTrust(None).unavailable().is_some());
    }

    /// The check passes only when the bundle holds the CA, and finds the
    /// CA files lokal put among the anchors.
    #[test]
    fn checks_trust_against_the_bundle() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::sandbox(dir.path().to_path_buf());
        let ca = CaCert::create(&paths, "u", "h").unwrap();
        let other = CaCert::preview(&paths, "v", "h").unwrap();
        let s = store(dir.path());
        std::fs::create_dir_all(&s.anchors).unwrap();
        let t = SystemTrust(Some(s.clone()));

        std::fs::write(&s.bundle, &other.pem).unwrap();
        assert!(matches!(
            t.is_trusted(&ca).unwrap(),
            TrustStatus::NotTrusted(_)
        ));
        assert!(!t.is_present(&ca).unwrap());

        std::fs::write(&s.bundle, format!("{}{}", other.pem, ca.pem)).unwrap();
        assert_eq!(t.is_trusted(&ca).unwrap(), TrustStatus::Trusted);

        std::fs::write(s.anchor(&ca), &ca.pem).unwrap();
        std::fs::write(s.anchors.join("someone-else.crt"), &other.pem).unwrap();
        assert!(t.is_present(&ca).unwrap());
        let found = CaCert::all_from_pem(&t.lokal_cas_pem().unwrap());
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].sha1, ca.sha1);
        assert!(!t.trust_too_broad(&ca).unwrap());
    }
}

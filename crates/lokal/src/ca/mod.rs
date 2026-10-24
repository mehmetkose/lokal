//! The per-Mac root CA (SPEC §6.1) and leaf issuance (§6.2).
//!
//! The CA is frozen once a human has trusted it: any change to its fields
//! means re-trusting (a sudo step). `tests::ca_fields_match_spec` pins
//! every field.

pub mod resolver;

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result, anyhow};
use rcgen::{
    BasicConstraints, CertificateParams, CidrSubnet, DistinguishedName, DnType,
    ExtendedKeyUsagePurpose, GeneralSubtree, IsCa, Issuer, KeyPair, KeyUsagePurpose,
    NameConstraints, SerialNumber,
};
use rustls::pki_types::{CertificateDer, PrivateKeyDer};

use crate::paths::Paths;
use crate::util;

pub const CA_CN_PREFIX: &str = "lokal development CA";
pub const CA_VALIDITY: Duration = Duration::from_secs(3652 * 86_400);
pub const LEAF_VALIDITY: Duration = Duration::from_secs(90 * 86_400);
pub const BACKDATE: Duration = Duration::from_secs(3600);
/// TLDs in the CA's name constraints: exactly the TLDs lokal serves, so a
/// leaked key can't sign for anything else (e.g. company `.internal` hosts).
pub const PERMITTED_TLDS: [&str; 2] = ["local", "lokal"];

/// Every IPv4 and IPv6 address, excluded. DNS-only constraints leave IP
/// addresses unconstrained (RFC 5280 §4.2.1.10): without this, a leaked key
/// could sign for `https://192.168.1.1`, and macOS and Chrome would trust it.
fn all_ip_addresses() -> Vec<GeneralSubtree> {
    vec![
        GeneralSubtree::IpAddress(CidrSubnet::V4([0; 4], [0; 4])),
        GeneralSubtree::IpAddress(CidrSubnet::V6([0; 16], [0; 16])),
    ]
}

/// What a CA's name constraints allow.
struct Scope {
    /// Permitted DNS subtrees.
    dns: Vec<String>,
    /// Both 0.0.0.0/0 and ::/0 are excluded.
    no_ips: bool,
}

fn random_serial() -> SerialNumber {
    let mut b: [u8; 16] = util::random_bytes();
    // Positive and 128 bits wide: top bit clear, next bit set.
    b[0] = (b[0] & 0x7f) | 0x40;
    SerialNumber::from_slice(&b)
}

fn offset(t: SystemTime) -> time::OffsetDateTime {
    time::OffsetDateTime::from(t)
}

/// The root CA, loaded from (or written to) `<base>/ca/`.
#[derive(Clone)]
pub struct CaCert {
    pub pem: String,
    pub der: CertificateDer<'static>,
    pub path: PathBuf,
    pub sha256: String,
    pub sha1: String,
    key_pem: String,
}

impl CaCert {
    pub fn exists(paths: &Paths) -> bool {
        paths.ca_cert.is_file() && paths.ca_key.is_file()
    }

    /// Creates a new CA for `user@host` and writes it. Key 0600, cert 0644.
    pub fn create(paths: &Paths, user: &str, host: &str) -> Result<Self> {
        let key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)?;
        let params = Self::params(user, host, SystemTime::now());
        let cert = params.self_signed(&key)?;
        util::create_private_dir(&paths.ca_dir)?;
        util::write_atomic(&paths.ca_key, key.serialize_pem().as_bytes(), 0o600)?;
        util::write_atomic(&paths.ca_cert, cert.pem().as_bytes(), 0o644)?;
        Self::load(paths)
    }

    /// An in-memory CA with the real path, never written (for planning).
    pub fn preview(paths: &Paths, user: &str, host: &str) -> Result<Self> {
        let key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)?;
        let cert = Self::params(user, host, SystemTime::now()).self_signed(&key)?;
        let der = cert.der().clone();
        Ok(Self {
            sha256: util::sha256_hex(&der),
            sha1: util::sha1_hex(&der),
            pem: cert.pem(),
            der,
            path: paths.ca_cert.clone(),
            key_pem: key.serialize_pem(),
        })
    }

    fn params(user: &str, host: &str, now: SystemTime) -> CertificateParams {
        let mut p = CertificateParams::default();
        let mut dn = DistinguishedName::new();
        dn.push(
            DnType::CommonName,
            format!("{CA_CN_PREFIX} ({user}@{host})"),
        );
        dn.push(DnType::OrganizationName, "lokal");
        p.distinguished_name = dn;
        p.not_before = offset(now - BACKDATE);
        p.not_after = offset(now + CA_VALIDITY);
        p.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
        p.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
        p.name_constraints = Some(NameConstraints {
            permitted_subtrees: PERMITTED_TLDS
                .iter()
                .map(|t| GeneralSubtree::DnsName((*t).to_string()))
                .collect(),
            excluded_subtrees: all_ip_addresses(),
        });
        p.serial_number = Some(random_serial());
        p
    }

    pub fn load(paths: &Paths) -> Result<Self> {
        let pem = fs::read_to_string(&paths.ca_cert)
            .with_context(|| format!("cannot read {}", paths.ca_cert.display()))?;
        let key_pem = fs::read_to_string(&paths.ca_key)
            .with_context(|| format!("cannot read {}", paths.ca_key.display()))?;
        let der = crate::net::read_pem_certs(&paths.ca_cert)?
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("no certificate in {}", paths.ca_cert.display()))?;
        Ok(Self {
            sha256: util::sha256_hex(&der),
            sha1: util::sha1_hex(&der),
            pem,
            der,
            path: paths.ca_cert.clone(),
            key_pem,
        })
    }

    /// A certificate without its key (a retired CA, kept only to untrust it).
    pub fn load_cert_only(path: &std::path::Path) -> Result<Self> {
        let pem =
            fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
        let mut c = Self::from_pem(&pem).with_context(|| format!("in {}", path.display()))?;
        c.path = path.to_path_buf();
        Ok(c)
    }

    /// The first certificate in `pem`, without a key or a path.
    pub fn from_pem(pem: &str) -> Result<Self> {
        let der = rustls_pemfile::certs(&mut pem.as_bytes())
            .next()
            .ok_or_else(|| anyhow!("no certificate"))??;
        Ok(Self {
            sha256: util::sha256_hex(&der),
            sha1: util::sha1_hex(&der),
            pem: pem.to_string(),
            der,
            path: PathBuf::new(),
            key_pem: String::new(),
        })
    }

    /// Every certificate in a PEM stream (such as `security find-certificate
    /// -p` output), each on its own, without keys or paths.
    pub fn all_from_pem(text: &str) -> Vec<Self> {
        const BEGIN: &str = "-----BEGIN CERTIFICATE-----";
        const END: &str = "-----END CERTIFICATE-----";
        text.split_inclusive(END)
            .filter_map(|chunk| {
                let block = &chunk[chunk.find(BEGIN)?..];
                block
                    .ends_with(END)
                    .then(|| Self::from_pem(&format!("{block}\n")).ok())
                    .flatten()
            })
            .collect()
    }

    /// Writes the certificate (never a key) to `self.path`, 0644: a CA
    /// recovered from the keychain needs a file for its untrust command.
    pub fn write_cert(&self) -> Result<()> {
        util::write_atomic(&self.path, self.pem.as_bytes(), 0o644)
    }

    /// Retired CAs (replaced, but maybe still in the keychain).
    pub fn load_retired(paths: &Paths) -> Vec<Self> {
        let Ok(rd) = fs::read_dir(&paths.ca_retired_dir) else {
            return vec![];
        };
        let mut v: Vec<Self> = rd
            .filter_map(|e| e.ok())
            .filter_map(|e| Self::load_cert_only(&e.path().join("root.pem")).ok())
            .collect();
        v.sort_by(|a, b| a.sha1.cmp(&b.sha1));
        v
    }

    /// Where this CA's certificate goes when it is retired.
    pub fn retired_path(&self, paths: &Paths) -> PathBuf {
        paths.ca_retired_dir.join(&self.sha1[..12]).join("root.pem")
    }

    /// Moves the current CA aside: its certificate to `ca/retired/<sha1>/`
    /// (needed to untrust it), and deletes its private key if it still has
    /// one. Returns the retired copy.
    pub fn retire(self, paths: &Paths) -> Result<Self> {
        let dst = self.retired_path(paths);
        if let Some(dir) = dst.parent() {
            util::create_private_dir(dir)?;
        }
        util::write_atomic(&dst, self.pem.as_bytes(), 0o644)?;
        match fs::remove_file(&paths.ca_key) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                return Err(e).context("cannot remove the old CA key");
            }
            _ => {}
        }
        fs::remove_file(&paths.ca_cert).context("cannot remove the old CA certificate")?;
        Self::load_cert_only(&dst)
    }

    /// A copy pointing at another file (for planning commands).
    pub fn with_path(&self, path: PathBuf) -> Self {
        Self {
            path,
            ..self.clone()
        }
    }

    pub fn load_or_create(paths: &Paths, user: &str, host: &str) -> Result<(Self, bool)> {
        if Self::exists(paths) {
            Ok((Self::load(paths)?, false))
        } else {
            Ok((Self::create(paths, user, host)?, true))
        }
    }

    /// The certificate's name constraints, or `None` if it has none (valid
    /// for every name).
    fn scope(&self) -> Option<Scope> {
        use x509_parser::extensions::{GeneralName, ParsedExtension};
        let (_, c) = x509_parser::parse_x509_certificate(&self.der).ok()?;
        c.extensions()
            .iter()
            .find_map(|e| match e.parsed_extension() {
                ParsedExtension::NameConstraints(nc) => {
                    let dns = nc
                        .permitted_subtrees
                        .iter()
                        .flatten()
                        .filter_map(|s| match &s.base {
                            GeneralName::DNSName(d) => Some(d.to_string()),
                            _ => None,
                        })
                        .collect();
                    // Address and mask, all zero: 8 bytes for IPv4, 32 for IPv6.
                    let excludes = |len: usize| {
                        nc.excluded_subtrees.iter().flatten().any(|s| {
                            matches!(&s.base, GeneralName::IPAddress(b)
                                if b.len() == len && b.iter().all(|x| *x == 0))
                        })
                    };
                    Some(Scope {
                        dns,
                        no_ips: excludes(8) && excludes(32),
                    })
                }
                _ => None,
            })
    }

    /// DNS names permitted by the certificate's name constraints.
    pub fn permitted_names(&self) -> Vec<String> {
        self.scope().map(|s| s.dns).unwrap_or_default()
    }

    /// Why this CA's name constraints differ from [`PERMITTED_TLDS`] with
    /// all IP addresses excluded, or `None` if they match. An older lokal
    /// created CAs without `.lokal`, later ones also allowed `.test` and
    /// `.internal`, and none excluded IP addresses before 2026-10-17.
    pub fn scope_problem(&self) -> Option<String> {
        let Some(have) = self.scope() else {
            return Some("it has no name constraints, so it is valid for every name".into());
        };
        let mut extra: Vec<String> = have
            .dns
            .iter()
            .filter(|h| !PERMITTED_TLDS.contains(&h.as_str()))
            .map(|t| format!(".{t}"))
            .collect();
        if !have.no_ips {
            extra.push("IP addresses".into());
        }
        let missing: Vec<String> = PERMITTED_TLDS
            .iter()
            .filter(|t| !have.dns.iter().any(|h| h == *t))
            .map(|t| format!(".{t}"))
            .collect();
        let mut why = vec![];
        if !extra.is_empty() {
            why.push(format!("it is also valid for {}", and_list(&extra)));
        }
        if !missing.is_empty() {
            why.push(format!("it doesn't cover {}", and_list(&missing)));
        }
        (!why.is_empty()).then(|| why.join("; "))
    }

    /// False when the name constraints differ from [`PERMITTED_TLDS`] with
    /// all IP addresses excluded: setup replaces such a CA.
    pub fn is_current(&self) -> bool {
        self.scope_problem().is_none()
    }

    /// The certificate's subject CN.
    pub fn common_name(&self) -> String {
        x509_parser::parse_x509_certificate(&self.der)
            .ok()
            .and_then(|(_, c)| {
                c.subject()
                    .iter_common_name()
                    .next()
                    .and_then(|cn| cn.as_str().ok().map(str::to_string))
            })
            .unwrap_or_default()
    }

    /// A leaf issuer with a fresh shared leaf key (one per daemon run).
    pub fn issuer(&self) -> Result<LeafIssuer> {
        let ca_key = KeyPair::from_pem(&self.key_pem).context("CA key is not valid")?;
        let issuer = Issuer::from_ca_cert_der(&self.der, ca_key)?;
        let leaf_key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)?;
        let key_der = PrivateKeyDer::Pkcs8(leaf_key.serialize_der().into());
        let signing_key = rustls::crypto::ring::sign::any_ecdsa_type(&key_der)
            .map_err(|e| anyhow!("leaf key: {e}"))?;
        Ok(LeafIssuer {
            issuer,
            leaf_key,
            signing_key,
        })
    }
}

/// Issues CA-signed leaves. All leaves of one daemon run share one key.
pub struct LeafIssuer {
    issuer: Issuer<'static, KeyPair>,
    leaf_key: KeyPair,
    signing_key: Arc<dyn rustls::sign::SigningKey>,
}

pub struct Leaf {
    pub der: CertificateDer<'static>,
    pub not_after: SystemTime,
}

impl LeafIssuer {
    fn leaf_params(fqdn: &str, now: SystemTime) -> Result<CertificateParams> {
        let mut p = CertificateParams::new(vec![fqdn.to_string()])?;
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, fqdn);
        p.distinguished_name = dn;
        p.is_ca = IsCa::ExplicitNoCa;
        p.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        p.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        p.not_before = offset(now - BACKDATE);
        p.not_after = offset(now + LEAF_VALIDITY);
        p.serial_number = Some(random_serial());
        p.use_authority_key_identifier_extension = true;
        Ok(p)
    }

    pub fn issue(&self, fqdn: &str) -> Result<Leaf> {
        let now = SystemTime::now();
        let cert = Self::leaf_params(fqdn, now)?.signed_by(&self.leaf_key, &self.issuer)?;
        Ok(Leaf {
            der: cert.der().clone(),
            not_after: now + LEAF_VALIDITY,
        })
    }

    /// A rustls certified key (leaf only; clients have the root).
    pub fn certified_key(
        &self,
        fqdn: &str,
    ) -> Result<(Arc<rustls::sign::CertifiedKey>, SystemTime)> {
        let leaf = self.issue(fqdn)?;
        let ck = rustls::sign::CertifiedKey::new(vec![leaf.der], self.signing_key.clone());
        Ok((Arc::new(ck), leaf.not_after))
    }

    /// PEM of a leaf for `fqdn` (for `security verify-cert`).
    pub fn issue_pem(&self, fqdn: &str) -> Result<String> {
        let leaf = self.issue(fqdn)?;
        Ok(pem_encode(&leaf.der))
    }
}

/// "a", "a and b", "a, b and c".
fn and_list(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

pub fn pem_encode(der: &[u8]) -> String {
    use std::fmt::Write;
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut b64 = String::new();
    for chunk in der.chunks(3) {
        let n = chunk.len();
        let v = (chunk[0] as u32) << 16
            | (*chunk.get(1).unwrap_or(&0) as u32) << 8
            | *chunk.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= n {
                b64.push(CHARS[((v >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                b64.push('=');
            }
        }
    }
    let mut out = String::from("-----BEGIN CERTIFICATE-----\n");
    for line in b64.as_bytes().chunks(64) {
        let _ = writeln!(out, "{}", std::str::from_utf8(line).unwrap_or_default());
    }
    out.push_str("-----END CERTIFICATE-----\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use x509_parser::extensions::{GeneralName, ParsedExtension};
    use x509_parser::prelude::*;

    fn sandbox() -> (tempdir::TempDir, Paths) {
        let d = tempdir::TempDir::new();
        let p = Paths::sandbox(d.0.clone());
        (d, p)
    }

    /// Minimal temp dir (no tempfile dependency in the library).
    mod tempdir {
        pub struct TempDir(pub std::path::PathBuf);
        impl TempDir {
            pub fn new() -> Self {
                let p = std::env::temp_dir()
                    .join(format!("lokal-ca-test-{}", crate::util::random_hex(12)));
                std::fs::create_dir_all(&p).unwrap();
                Self(p)
            }
        }
        impl Drop for TempDir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
    }

    /// SPEC §6.1, field by field. The CA freezes at human gate G1.
    #[test]
    fn ca_fields_match_spec() {
        let (_d, paths) = sandbox();
        let ca = CaCert::create(&paths, "alice", "mac").unwrap();
        let (_, c) = parse_x509_certificate(&ca.der).unwrap();

        // Key and signature
        assert_eq!(
            c.public_key().algorithm.algorithm,
            x509_parser::oid_registry::OID_KEY_TYPE_EC_PUBLIC_KEY
        );
        let curve = c
            .public_key()
            .algorithm
            .parameters
            .as_ref()
            .unwrap()
            .as_oid()
            .unwrap();
        assert_eq!(curve, x509_parser::oid_registry::OID_EC_P256);
        assert_eq!(
            c.signature_algorithm.algorithm,
            x509_parser::oid_registry::OID_SIG_ECDSA_WITH_SHA256
        );

        // Subject
        let cn = c
            .subject()
            .iter_common_name()
            .next()
            .unwrap()
            .as_str()
            .unwrap();
        assert_eq!(cn, "lokal development CA (alice@mac)");
        let o = c
            .subject()
            .iter_organization()
            .next()
            .unwrap()
            .as_str()
            .unwrap();
        assert_eq!(o, "lokal");
        assert_eq!(c.subject(), c.issuer(), "self-signed");

        // Validity: 10 years, notBefore = now − 1h
        let nb = c.validity().not_before.timestamp();
        let na = c.validity().not_after.timestamp();
        let now = ::time::OffsetDateTime::now_utc().unix_timestamp();
        assert!((now - 3600 - nb).abs() < 120, "notBefore {nb} now {now}");
        let years = (na - nb - 3600) as f64 / (365.25 * 86400.0);
        assert!((years - 10.0).abs() < 0.01, "validity {years} years");

        // Serial: 128-bit positive
        let serial = c.raw_serial();
        assert_eq!(serial.len(), 16, "serial {serial:?}");
        assert!(serial[0] & 0x80 == 0);

        let mut seen = (false, false, false);
        for ext in c.extensions() {
            match ext.parsed_extension() {
                ParsedExtension::BasicConstraints(bc) => {
                    seen.0 = true;
                    assert!(ext.critical, "basicConstraints critical");
                    assert!(bc.ca);
                    assert_eq!(bc.path_len_constraint, Some(0));
                }
                ParsedExtension::KeyUsage(ku) => {
                    seen.1 = true;
                    assert!(ext.critical, "keyUsage critical");
                    assert!(ku.key_cert_sign() && ku.crl_sign());
                    assert!(!ku.digital_signature());
                }
                ParsedExtension::NameConstraints(nc) => {
                    seen.2 = true;
                    assert!(ext.critical, "nameConstraints critical");
                    let permitted: Vec<String> = nc
                        .permitted_subtrees
                        .as_ref()
                        .unwrap()
                        .iter()
                        .map(|s| match &s.base {
                            GeneralName::DNSName(d) => d.to_string(),
                            o => panic!("unexpected subtree {o:?}"),
                        })
                        .collect();
                    assert_eq!(permitted, ["local", "lokal"]);
                    // Every IPv4 and IPv6 address (address + mask, all zero).
                    let excluded: Vec<Vec<u8>> = nc
                        .excluded_subtrees
                        .as_ref()
                        .unwrap()
                        .iter()
                        .map(|s| match &s.base {
                            GeneralName::IPAddress(b) => b.to_vec(),
                            o => panic!("unexpected excluded subtree {o:?}"),
                        })
                        .collect();
                    assert_eq!(excluded, [vec![0u8; 8], vec![0u8; 32]]);
                }
                _ => {}
            }
        }
        assert_eq!(seen, (true, true, true), "BC, KU, NC present");

        // File modes
        assert_eq!(util::file_mode(&paths.ca_key), Some(0o600));
        assert_eq!(util::file_mode(&paths.ca_cert), Some(0o644));
    }

    /// SPEC §6.2.
    #[test]
    fn leaf_fields_match_spec() {
        let (_d, paths) = sandbox();
        let ca = CaCert::create(&paths, "u", "h").unwrap();
        let iss = ca.issuer().unwrap();
        let leaf = iss.issue("api.myproject.local").unwrap();
        let (_, c) = parse_x509_certificate(&leaf.der).unwrap();
        let (_, root) = parse_x509_certificate(&ca.der).unwrap();
        assert_eq!(c.issuer(), root.subject());
        c.verify_signature(Some(root.public_key())).unwrap();
        let cn = c
            .subject()
            .iter_common_name()
            .next()
            .unwrap()
            .as_str()
            .unwrap();
        assert_eq!(cn, "api.myproject.local");
        let mut seen = 0;
        for ext in c.extensions() {
            match ext.parsed_extension() {
                ParsedExtension::SubjectAlternativeName(san) => {
                    seen += 1;
                    assert_eq!(san.general_names.len(), 1);
                    assert!(matches!(
                        san.general_names[0],
                        GeneralName::DNSName("api.myproject.local")
                    ));
                }
                ParsedExtension::ExtendedKeyUsage(eku) => {
                    seen += 1;
                    assert!(eku.server_auth);
                }
                ParsedExtension::KeyUsage(ku) => {
                    seen += 1;
                    assert!(ext.critical);
                    assert!(ku.digital_signature() && !ku.key_cert_sign());
                }
                ParsedExtension::BasicConstraints(bc) => {
                    seen += 1;
                    assert!(!bc.ca);
                }
                _ => {}
            }
        }
        assert_eq!(seen, 4);
        let days =
            (c.validity().not_after.timestamp() - c.validity().not_before.timestamp() - 3600)
                / 86400;
        assert_eq!(days, 90);
    }

    #[test]
    fn load_round_trip_and_pem() {
        let (_d, paths) = sandbox();
        let ca = CaCert::create(&paths, "u", "h").unwrap();
        let again = CaCert::load(&paths).unwrap();
        assert_eq!(ca.sha256, again.sha256);
        assert_eq!(pem_encode(&ca.der).trim(), ca.pem.trim());
        assert_eq!(ca.common_name(), "lokal development CA (u@h)");
        assert!(ca.is_current());
        assert_eq!(ca.scope_problem(), None);
        assert_eq!(ca.permitted_names(), ["local", "lokal"]);
    }

    /// A self-signed CA with the given name constraints (`None`: none), and
    /// IP addresses excluded or not.
    fn ca_with(permitted: Option<&[&str]>, excluded: Vec<rcgen::GeneralSubtree>) -> CaCert {
        let key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
        let mut p = CaCert::params("u", "h", SystemTime::now());
        p.name_constraints = permitted.map(|tlds| rcgen::NameConstraints {
            permitted_subtrees: tlds
                .iter()
                .map(|t| rcgen::GeneralSubtree::DnsName((*t).to_string()))
                .collect(),
            excluded_subtrees: excluded,
        });
        let cert = p.self_signed(&key).unwrap();
        let der = cert.der().clone();
        CaCert {
            sha256: util::sha256_hex(&der),
            sha1: util::sha1_hex(&der),
            pem: cert.pem(),
            der,
            path: PathBuf::from("root.pem"),
            key_pem: key.serialize_pem(),
        }
    }

    /// The CA trusted before the narrowing (local, lokal, test, internal; IP
    /// addresses unconstrained) is outdated, so setup replaces it. So is one
    /// from before `.lokal`.
    #[test]
    fn ca_with_other_tlds_is_outdated() {
        let wide = ca_with(Some(&["local", "lokal", "test", "internal"]), vec![]);
        assert!(!wide.is_current());
        assert_eq!(
            wide.scope_problem().unwrap(),
            "it is also valid for .test, .internal and IP addresses"
        );

        let old = ca_with(Some(&["local", "test", "internal"]), vec![]);
        assert!(!old.is_current());
        assert_eq!(
            old.scope_problem().unwrap(),
            "it is also valid for .test, .internal and IP addresses; it doesn't cover .lokal"
        );

        let none = ca_with(None, vec![]);
        assert!(!none.is_current());
        assert!(none.scope_problem().unwrap().contains("every name"));

        // Order doesn't matter.
        assert!(ca_with(Some(&["lokal", "local"]), all_ip_addresses()).is_current());
    }

    /// Regression: DNS-only constraints leave IP addresses unconstrained, and
    /// macOS and Chrome trusted a 127.0.0.1 leaf from such a CA (2026-10-17).
    #[test]
    fn ca_must_exclude_every_ip_address() {
        use rcgen::GeneralSubtree::IpAddress;
        let tlds: &[&str] = &["local", "lokal"];
        let no_ex = ca_with(Some(tlds), vec![]);
        assert_eq!(
            no_ex.scope_problem().unwrap(),
            "it is also valid for IP addresses"
        );
        // Excluding only one address family, or only part of one, isn't enough.
        let v4_only = ca_with(Some(tlds), vec![IpAddress(CidrSubnet::V4([0; 4], [0; 4]))]);
        assert!(!v4_only.is_current());
        let v6_only = ca_with(
            Some(tlds),
            vec![IpAddress(CidrSubnet::V6([0; 16], [0; 16]))],
        );
        assert!(!v6_only.is_current());
        let partial = ca_with(
            Some(tlds),
            vec![
                IpAddress(CidrSubnet::V4([10, 0, 0, 0], [255, 0, 0, 0])),
                IpAddress(CidrSubnet::V6([0; 16], [0; 16])),
            ],
        );
        assert!(!partial.is_current());
        assert!(ca_with(Some(tlds), all_ip_addresses()).is_current());
    }

    #[test]
    fn and_lists() {
        let s = |v: &[&str]| and_list(&v.iter().map(|x| x.to_string()).collect::<Vec<_>>());
        assert_eq!(s(&[]), "");
        assert_eq!(s(&["a"]), "a");
        assert_eq!(s(&["a", "b"]), "a and b");
        assert_eq!(s(&["a", "b", "c"]), "a, b and c");
    }

    /// `security find-certificate -a -Z -p` output: hash lines, then a PEM
    /// block, for each match. Every block parses on its own.
    #[test]
    fn all_from_pem_reads_find_certificate_output() {
        let (_d, paths) = sandbox();
        let a = CaCert::preview(&paths, "a", "mac").unwrap();
        let b = CaCert::preview(&paths, "b", "mac").unwrap();
        let text = format!(
            "SHA-256 hash: {}\nSHA-1 hash: {}\n{}SHA-1 hash: {}\n{}",
            a.sha256.to_uppercase(),
            a.sha1.to_uppercase(),
            a.pem,
            b.sha1.to_uppercase(),
            b.pem.trim_end()
        );
        let got: Vec<String> = CaCert::all_from_pem(&text)
            .into_iter()
            .map(|c| c.sha1)
            .collect();
        assert_eq!(got, [a.sha1.clone(), b.sha1.clone()]);
        assert!(CaCert::all_from_pem("").is_empty());
        let one = CaCert::from_pem(&a.pem).unwrap();
        assert_eq!(one.sha256, a.sha256);
        assert_eq!(one.common_name(), a.common_name());
    }

    /// A CA whose key file is gone can still be retired.
    #[test]
    fn retire_without_a_key() {
        let (_d, paths) = sandbox();
        let ca = CaCert::create(&paths, "alice", "mac").unwrap();
        fs::remove_file(&paths.ca_key).unwrap();
        let retired = CaCert::load_cert_only(&paths.ca_cert)
            .unwrap()
            .retire(&paths)
            .unwrap();
        assert_eq!(retired.sha1, ca.sha1);
        assert_eq!(retired.path, ca.retired_path(&paths));
        assert!(!paths.ca_cert.exists());
    }
}

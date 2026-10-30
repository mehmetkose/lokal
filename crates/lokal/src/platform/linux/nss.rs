//! Browsers (SPEC §4.6): Chrome, Chromium and Firefox on Linux don't read the
//! system bundle; they trust what their NSS databases hold. lokal adds its
//! CA to each one with `certutil` (libnss3-tools on Debian and Ubuntu,
//! nss-tools on Fedora), as the user and never as root, trusted for TLS
//! only (`C,,`). Its nickname starts with the CA's common-name prefix, so
//! uninstall removes exactly lokal's.

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

use crate::ca::{CA_CN_PREFIX, CaCert};
use crate::process;

const CERTUTIL: [&str; 2] = ["/usr/bin/certutil", "/bin/certutil"];
/// Chrome and Chromium create `~/.pki/nssdb` on first start; setup creates
/// it when one of them is installed but hasn't run yet.
const CHROMES: [&str; 6] = [
    "/usr/bin/google-chrome",
    "/usr/bin/google-chrome-stable",
    "/usr/bin/chromium",
    "/usr/bin/chromium-browser",
    "/opt/google/chrome/chrome",
    "/snap/bin/chromium",
];
const NO_CERTUTIL: &str = "certutil is missing, so browsers don't trust lokal yet: install libnss3-tools (Debian, Ubuntu) or nss-tools (Fedora), then run 'lokal setup' again";

fn certutil() -> Option<&'static str> {
    CERTUTIL.into_iter().find(|p| Path::new(p).is_file())
}

fn home() -> Option<PathBuf> {
    directories::BaseDirs::new().map(|d| d.home_dir().to_path_buf())
}

fn chrome_installed() -> bool {
    CHROMES.iter().any(|p| Path::new(p).exists())
}

/// Chrome's and Chromium's shared database.
fn chrome_db(home: &Path) -> PathBuf {
    home.join(".pki/nssdb")
}

/// Every NSS database of the user's: Chrome's, snap Chromium's, and each
/// Firefox profile's (deb, snap, flatpak). Only ones that exist.
fn databases() -> Vec<PathBuf> {
    let Some(home) = home() else { return vec![] };
    let mut dbs: Vec<PathBuf> = [
        chrome_db(&home),
        home.join("snap/chromium/current/.pki/nssdb"),
    ]
    .into_iter()
    .filter(|d| d.join("cert9.db").is_file())
    .collect();
    for dir in [
        ".mozilla/firefox",
        "snap/firefox/common/.mozilla/firefox",
        ".var/app/org.mozilla.firefox/.mozilla/firefox",
    ] {
        let Ok(rd) = std::fs::read_dir(home.join(dir)) else {
            continue;
        };
        let mut profiles: Vec<PathBuf> = rd
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.join("cert9.db").is_file())
            .collect();
        profiles.sort();
        dbs.extend(profiles);
    }
    dbs
}

/// For messages: `~/.pki/nssdb`.
fn short(db: &Path) -> String {
    match home().and_then(|h| db.strip_prefix(h).ok().map(Path::to_path_buf)) {
        Some(rel) => format!("~/{}", rel.display()),
        None => db.display().to_string(),
    }
}

fn nickname(ca: &CaCert) -> String {
    format!("{CA_CN_PREFIX} {}", &ca.sha1.to_lowercase()[..12])
}

fn sql(db: &Path) -> String {
    format!("sql:{}", db.display())
}

/// The nicknames in `certutil -L`'s table: the name, then spaces, then the
/// trust flags (`C,,` or `CT,C,C`), after two header lines.
fn nicknames(listing: &str) -> Vec<String> {
    listing
        .lines()
        .filter_map(|l| {
            let l = l.trim_end();
            let (name, flags) = l.rsplit_once(char::is_whitespace)?;
            (flags.matches(',').count() == 2 && !name.trim().is_empty())
                .then(|| name.trim().to_string())
        })
        .collect()
}

fn lokal_nicknames(certutil: &str, db: &Path) -> Vec<String> {
    process::run(certutil, &["-L", "-d", &sql(db)])
        .map(|o| nicknames(&o.stdout))
        .unwrap_or_default()
        .into_iter()
        .filter(|n| n.starts_with(CA_CN_PREFIX))
        .collect()
}

/// Removes every certificate called `nick` (`-D` takes one at a time).
fn delete(certutil: &str, db: &Path, nick: &str) {
    for _ in 0..8 {
        match process::run(certutil, &["-D", "-d", &sql(db), "-n", nick]) {
            Ok(o) if o.success() => {}
            _ => break,
        }
    }
}

pub fn trust(ca: &CaCert) -> Result<Vec<String>> {
    let tool = certutil();
    let mut done = vec![];
    if let (Some(home), Some(tool)) = (home(), tool) {
        let db = chrome_db(&home);
        if chrome_installed() && !db.join("cert9.db").is_file() {
            std::fs::create_dir_all(&db)?;
            process::run_ok(tool, &["-N", "--empty-password", "-d", &sql(&db)])?;
        }
    }
    let dbs = databases();
    if dbs.is_empty() && !chrome_installed() {
        return Ok(done);
    }
    let Some(tool) = tool else { bail!(NO_CERTUTIL) };
    let nick = nickname(ca);
    for db in dbs {
        let have = lokal_nicknames(tool, &db);
        if have == [nick.clone()] {
            continue;
        }
        // An older CA of lokal's goes, as from the system store.
        for old in have.iter().filter(|n| **n != nick) {
            delete(tool, &db, old);
        }
        if !have.contains(&nick) {
            let path = ca.path.display().to_string();
            process::run_ok(
                tool,
                &["-A", "-d", &sql(&db), "-t", "C,,", "-n", &nick, "-i", &path],
            )?;
        }
        done.push(format!(
            "Browser certificates {}: trusted lokal's CA",
            short(&db)
        ));
    }
    Ok(done)
}

pub fn untrust() -> Result<Vec<String>> {
    let dbs = databases();
    let Some(tool) = certutil() else {
        return Ok(vec![]);
    };
    let mut done = vec![];
    for db in dbs {
        let have = lokal_nicknames(tool, &db);
        if have.is_empty() {
            continue;
        }
        for n in &have {
            delete(tool, &db, n);
        }
        done.push(format!(
            "Browser certificates {}: removed lokal's CA",
            short(&db)
        ));
    }
    Ok(done)
}

pub fn status() -> Option<(bool, String)> {
    let dbs = databases();
    if dbs.is_empty() && !chrome_installed() {
        return None;
    }
    let Some(tool) = certutil() else {
        return Some((false, NO_CERTUTIL.into()));
    };
    if dbs.is_empty() {
        return Some((
            false,
            "Chrome has no certificate database yet: run 'lokal setup' again".into(),
        ));
    }
    let missing: Vec<String> = dbs
        .iter()
        .filter(|db| lokal_nicknames(tool, db).is_empty())
        .map(|db| short(db))
        .collect();
    if missing.is_empty() {
        Some((
            true,
            format!(
                "lokal's CA is in every browser certificate database ({})",
                dbs.len()
            ),
        ))
    } else {
        Some((
            false,
            format!(
                "{} don't trust lokal's CA; run 'lokal setup'",
                missing.join(", ")
            ),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_certutils_nicknames() {
        let listing = "\n\
Certificate Nickname                                         Trust Attributes\n\
                                                             SSL,S/MIME,JAR/XPI\n\
\n\
lokal development CA 0123456789ab                            C,,\n\
Some Corp Root CA (2024)                                     CT,C,C\n";
        assert_eq!(
            nicknames(listing),
            [
                "lokal development CA 0123456789ab",
                "Some Corp Root CA (2024)"
            ]
        );
    }
}

//! A person's approval of a `lokal.toml` (SPEC §16.11): `lokal up` runs a
//! file's commands only after a person has seen them and said yes, or with
//! `--yes`, which records nothing.
//!
//! The record is `<base>/trusted/<SHA-256 of the file's canonical path>`
//! ([`Paths::trust_record`]), mode 0600 in a 0700 directory, and holds the
//! SHA-256 of the file's bytes and a newline, nothing else. Any change to
//! the file, even a comment, asks again.
//!
//! Not called `trust`: in this codebase that is the CA's trust in the
//! keychain (`TrustStore`).

use std::path::Path;

use anyhow::Result;

use crate::paths::Paths;
use crate::util;

/// What the record says about a file's bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// The record holds this digest: a person approved these bytes.
    Approved,
    /// The record holds another digest: the file changed since then.
    Changed,
    /// No record, or one that can't be read.
    Unapproved,
}

/// Looks up the approval of the file at `canonical` whose bytes have
/// `digest`. A missing or unreadable record means not approved.
pub fn status(paths: &Paths, canonical: &Path, digest: &str) -> Status {
    let Ok(text) = std::fs::read_to_string(paths.trust_record(canonical)) else {
        return Status::Unapproved;
    };
    match text.lines().next().map(str::trim) {
        Some(d) if d == digest => Status::Approved,
        Some(d) if !d.is_empty() => Status::Changed,
        _ => Status::Unapproved,
    }
}

/// Records that a person approved the file at `canonical` whose bytes have
/// `digest`. Only `lokal up` calls it, after a `y` on a terminal.
pub fn record(paths: &Paths, canonical: &Path, digest: &str) -> Result<()> {
    // write_atomic creates the directory with mode 0700.
    util::write_atomic(
        &paths.trust_record(canonical),
        format!("{digest}\n").as_bytes(),
        0o600,
    )
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    const A: &str = "a3ec7a0d38f5d4d2c3ebdc8a01ae1dc3e0c41cb3c8ad7cbd5e2d47aa0f3f9b44";
    const B: &str = "290ce42f1ac7768c54b08b954490c02032977dfa73d851dabd5935f82943fdea";

    fn sandbox() -> (tempfile::TempDir, Paths) {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::sandbox(dir.path().to_path_buf());
        (dir, paths)
    }

    #[test]
    fn a_missing_record_is_not_an_approval() {
        let (_d, p) = sandbox();
        let file = PathBuf::from("/r/shop/lokal.toml");
        assert_eq!(status(&p, &file, A), Status::Unapproved);
        assert!(!p.trusted_dir.exists());
    }

    #[test]
    fn a_matching_record_approves_and_another_digest_is_a_change() {
        let (_d, p) = sandbox();
        let file = PathBuf::from("/r/shop/lokal.toml");
        record(&p, &file, A).unwrap();
        assert_eq!(status(&p, &file, A), Status::Approved);
        assert_eq!(status(&p, &file, B), Status::Changed);
        // Another path has its own record.
        assert_eq!(
            status(&p, Path::new("/r/copy/lokal.toml"), A),
            Status::Unapproved
        );
        // Recording other bytes replaces the old approval.
        record(&p, &file, B).unwrap();
        assert_eq!(status(&p, &file, A), Status::Changed);
        assert_eq!(status(&p, &file, B), Status::Approved);
    }

    #[test]
    fn the_record_is_private_and_holds_only_the_digest() {
        let (_d, p) = sandbox();
        let file = PathBuf::from("/r/shop/lokal.toml");
        record(&p, &file, A).unwrap();
        let rec = p.trust_record(&file);
        assert_eq!(std::fs::read_to_string(&rec).unwrap(), format!("{A}\n"));
        assert_eq!(util::file_mode(&rec), Some(0o600));
        assert_eq!(util::file_mode(&p.trusted_dir), Some(0o700));
        // Nothing else is left in the directory (no temp file).
        assert_eq!(std::fs::read_dir(&p.trusted_dir).unwrap().count(), 1);
    }

    #[test]
    fn an_unreadable_or_empty_record_is_not_an_approval() {
        let (_d, p) = sandbox();
        let file = PathBuf::from("/r/shop/lokal.toml");
        let rec = p.trust_record(&file);
        // A directory where the record should be can't be read.
        std::fs::create_dir_all(&rec).unwrap();
        assert_eq!(status(&p, &file, A), Status::Unapproved);
        std::fs::remove_dir(&rec).unwrap();
        // Not UTF-8.
        std::fs::write(&rec, b"\xff\xfe\n").unwrap();
        assert_eq!(status(&p, &file, A), Status::Unapproved);
        // Empty.
        std::fs::write(&rec, "").unwrap();
        assert_eq!(status(&p, &file, A), Status::Unapproved);
        // Only the first line counts, trimmed.
        std::fs::write(&rec, format!("  {A} \nanything\n")).unwrap();
        assert_eq!(status(&p, &file, A), Status::Approved);
    }
}

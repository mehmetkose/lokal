//! Small helpers: atomic/private file writes, randomness, digests, time.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
// PORTING(windows): unix permission bits; Windows needs ACLs (owner-only DACL).
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};

/// Creates `dir` (and parents) with mode 0700 if missing.
pub fn create_private_dir(dir: &Path) -> Result<()> {
    if dir.is_dir() {
        return Ok(());
    }
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .with_context(|| format!("cannot create {}", dir.display()))
}

/// Atomically replaces `path` with `data`: temp file, fsync, rename.
pub fn write_atomic(path: &Path, data: &[u8], mode: u32) -> Result<()> {
    if let Some(parent) = path.parent() {
        create_private_dir(parent)?;
    }
    let tmp = crate::paths::Paths::tmp_sibling(path);
    {
        let mut f = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(mode)
            .open(&tmp)
            .with_context(|| format!("cannot write {}", tmp.display()))?;
        f.write_all(data)?;
        f.set_permissions(fs::Permissions::from_mode(mode))?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path).with_context(|| format!("cannot replace {}", path.display()))?;
    Ok(())
}

/// Atomically copies `src` to `dst` with `mode`. Returns true if the
/// content changed.
pub fn copy_atomic(src: &Path, dst: &Path, mode: u32) -> Result<bool> {
    let data = fs::read(src).with_context(|| format!("cannot read {}", src.display()))?;
    if fs::read(dst).ok().as_deref() == Some(&data[..]) {
        // Keep the mode right even when the content matches.
        let _ = fs::set_permissions(dst, fs::Permissions::from_mode(mode));
        return Ok(false);
    }
    write_atomic(dst, &data, mode)?;
    Ok(true)
}

pub fn file_mode(path: &Path) -> Option<u32> {
    fs::metadata(path)
        .ok()
        .map(|m| m.permissions().mode() & 0o777)
}

/// Owner uid of a file.
pub fn file_uid(path: &Path) -> Option<u32> {
    use std::os::unix::fs::MetadataExt;
    fs::metadata(path).ok().map(|m| m.uid())
}

pub fn current_uid() -> u32 {
    // SAFETY: getuid has no preconditions.
    unsafe { libc::getuid() }
}

pub fn stdin_is_tty() -> bool {
    // SAFETY: isatty only inspects the fd.
    unsafe { libc::isatty(0) == 1 }
}

pub fn stdout_is_tty() -> bool {
    // SAFETY: isatty only inspects the fd.
    unsafe { libc::isatty(1) == 1 }
}

pub fn random_bytes<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut b)
        .expect("system randomness");
    b
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// `n` random hex characters (n even).
pub fn random_hex(n: usize) -> String {
    let b: [u8; 16] = random_bytes();
    hex(&b)[..n.min(32)].to_string()
}

pub fn sha256_hex(data: &[u8]) -> String {
    hex(ring::digest::digest(&ring::digest::SHA256, data).as_ref())
}

pub fn sha1_hex(data: &[u8]) -> String {
    hex(ring::digest::digest(&ring::digest::SHA1_FOR_LEGACY_USE_ONLY, data).as_ref())
}

pub fn now_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .replace_nanosecond(0)
        .ok()
        .and_then(|t| {
            t.format(&time::format_description::well_known::Rfc3339)
                .ok()
        })
        .unwrap_or_default()
}

/// `38ms`, `1.2s`.
pub fn fmt_duration(d: Duration) -> String {
    let ms = d.as_secs_f64() * 1000.0;
    if ms < 1000.0 {
        format!("{}ms", ms.round() as u64)
    } else {
        format!("{:.1}s", ms / 1000.0)
    }
}

/// Opens (creating) a lock file and takes an exclusive lock, blocking.
pub fn lock_exclusive(path: &Path) -> Result<File> {
    if let Some(parent) = path.parent() {
        create_private_dir(parent)?;
    }
    let f = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("cannot open {}", path.display()))?;
    f.lock()
        .with_context(|| format!("cannot lock {}", path.display()))?;
    Ok(f)
}

/// Last `n` lines of a file (whole file read; logs are small and rotated).
pub fn tail_lines(path: &Path, n: usize) -> Vec<String> {
    let Ok(data) = fs::read_to_string(path) else {
        return vec![];
    };
    let lines: Vec<&str> = data.lines().collect();
    let start = lines.len().saturating_sub(n);
    lines[start..].iter().map(|s| s.to_string()).collect()
}

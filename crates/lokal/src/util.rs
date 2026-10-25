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

/// Opens `path` for writing, created or truncated, with mode 0600 (also
/// when it existed with another mode). Writes go straight to the file:
/// `File` doesn't buffer.
pub fn create_private_file(path: &Path) -> std::io::Result<File> {
    let f = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    f.set_permissions(fs::Permissions::from_mode(0o600))?;
    Ok(f)
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

/// The time now as RFC 3339 UTC with microseconds,
/// `2026-10-25T10:44:34.635510Z`: the format `daemon.log`'s lines start
/// with (tracing-subscriber's default timer), so a service's `lokal up` log
/// lines up with it (SPEC §4.11, "Logs").
pub fn now_rfc3339_micros() -> String {
    rfc3339_micros(time::OffsetDateTime::now_utc())
}

fn rfc3339_micros(t: time::OffsetDateTime) -> String {
    let t = t.to_offset(time::UtcOffset::UTC);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:06}Z",
        t.year(),
        u8::from(t.month()),
        t.day(),
        t.hour(),
        t.minute(),
        t.second(),
        t.microsecond()
    )
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

/// Opens a lock file and tries to take an exclusive lock without waiting
/// (`File::try_lock`). With `create`, a missing file is created (mode 0600,
/// its directory 0700); without, a missing file is `Ok(None)`. Otherwise
/// the file, and whether this process now holds the lock (false: another
/// one does). The kernel releases the lock when the file is closed, also
/// when the process dies.
pub fn try_lock_exclusive(path: &Path, create: bool) -> Result<Option<(File, bool)>> {
    if create && let Some(parent) = path.parent() {
        create_private_dir(parent)?;
    }
    let f = match OpenOptions::new()
        .read(true)
        .write(true)
        .create(create)
        .truncate(false)
        .mode(0o600)
        .open(path)
    {
        Ok(f) => f,
        Err(e) if !create && e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("cannot open {}", path.display())),
    };
    match f.try_lock() {
        Ok(()) => Ok(Some((f, true))),
        Err(fs::TryLockError::WouldBlock) => Ok(Some((f, false))),
        Err(fs::TryLockError::Error(e)) => {
            Err(e).with_context(|| format!("cannot lock {}", path.display()))
        }
    }
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

/// The last `n` lines of `files` read as one, oldest file first (a log's
/// `.log.1`, then its `.log`). A missing file has no lines. Each file is
/// read whole, which is fine for logs capped at a few MB.
pub fn tail_lines_across(files: &[&Path], n: usize) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for f in files.iter().rev() {
        if out.len() >= n {
            break;
        }
        let mut earlier = tail_lines(f, n - out.len());
        earlier.append(&mut out);
        out = earlier;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tail_lines_across_counts_back_through_the_older_file() {
        let dir = tempfile::tempdir().unwrap();
        let (old, new) = (dir.path().join("a.log.1"), dir.path().join("a.log"));
        let missing = dir.path().join("none.log");
        fs::write(&old, "o1\no2\no3\n").unwrap();
        fs::write(&new, "n1\nn2\n").unwrap();
        let tail = |files: &[&Path], n| tail_lines_across(files, n);
        // Enough lines in the newer file: the older one isn't needed.
        assert_eq!(tail(&[&old, &new], 2), ["n1", "n2"]);
        assert_eq!(tail(&[&old, &new], 1), ["n2"]);
        // More than the newer file has: the rest from the end of the older.
        assert_eq!(tail(&[&old, &new], 3), ["o3", "n1", "n2"]);
        assert_eq!(tail(&[&old, &new], 4), ["o2", "o3", "n1", "n2"]);
        // More than both have: all of them, in order.
        assert_eq!(tail(&[&old, &new], 50), ["o1", "o2", "o3", "n1", "n2"]);
        assert!(tail(&[&old, &new], 0).is_empty());
        // A missing file has no lines, on either side.
        assert_eq!(tail(&[&missing, &new], 5), ["n1", "n2"]);
        assert_eq!(tail(&[&old, &missing], 2), ["o2", "o3"]);
        assert!(tail(&[&missing, &missing], 5).is_empty());
        // A file without a final newline still counts its last line.
        fs::write(&new, "n1\nn2").unwrap();
        assert_eq!(tail(&[&old, &new], 3), ["o3", "n1", "n2"]);
    }

    #[test]
    fn the_log_timestamp_is_rfc3339_utc_with_microseconds() {
        let t = time::OffsetDateTime::from_unix_timestamp_nanos(1_792_925_074_635_510_123).unwrap();
        assert_eq!(rfc3339_micros(t), "2026-10-25T10:44:34.635510Z");
        // Another offset is shown in UTC, and zeros keep their width.
        let t = time::OffsetDateTime::from_unix_timestamp(1_767_225_600)
            .unwrap()
            .to_offset(time::UtcOffset::from_hms(2, 0, 0).unwrap());
        assert_eq!(rfc3339_micros(t), "2026-01-01T00:00:00.000000Z");
        let now = now_rfc3339_micros();
        assert_eq!(now.len(), "2026-10-25T10:44:34.635510Z".len(), "{now}");
        assert!(now.ends_with('Z'), "{now}");
    }
}

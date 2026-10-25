//! The log of each service that `lokal up` starts (SPEC §4.11, "Logs").
//!
//! `<log dir>/up/<project>/<sub>.log` holds every line lokal prints for the
//! service, from either stream and in order, as `<timestamp> <line>`, and
//! lokal's own lines about it as `<timestamp> lokal: …`, so an agent that
//! reads only the file still sees what happened. The supervisor's main loop
//! is the only writer, and the project's lock keeps a second `up` out, so no
//! two writers ever share a file. Each line is one write, unbuffered, so
//! `lokal logs -f` sees it at once. Before a write would take the file past
//! [`CAP`], it becomes `<sub>.log.1` and a new file starts.

use std::fs::File;
use std::io::Write;
use std::path::PathBuf;

use anyhow::{Context, Result};

use crate::paths::Paths;
use crate::util;

/// A log file never grows past this (5 MB); with its `.log.1`, a service
/// keeps at most about 10 MB.
pub(crate) const CAP: u64 = 5 * 1024 * 1024;

/// One service's log file and its rotated copy.
struct LogFile {
    path: PathBuf,
    rotated: PathBuf,
    file: File,
    /// Bytes in `file`.
    len: u64,
    cap: u64,
}

impl LogFile {
    /// Truncates `path`, and removes an older `rotated`: the log of a run
    /// holds only that run.
    fn start(path: PathBuf, rotated: PathBuf, cap: u64) -> Result<Self> {
        if let Some(dir) = path.parent() {
            util::create_private_dir(dir)?;
        }
        match std::fs::remove_file(&rotated) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                return Err(e).with_context(|| format!("cannot remove {}", rotated.display()));
            }
            _ => {}
        }
        let file = util::create_private_file(&path)
            .with_context(|| format!("cannot write {}", path.display()))?;
        Ok(Self {
            path,
            rotated,
            file,
            len: 0,
            cap,
        })
    }

    /// Writes `line` (with its newline) in one write. If that would take
    /// the file past the cap, the file first becomes `rotated`, replacing
    /// an older one, and a new file starts.
    fn write(&mut self, line: &str) -> Result<()> {
        let n = line.len() as u64;
        if self.len > 0 && self.len + n > self.cap {
            std::fs::rename(&self.path, &self.rotated).with_context(|| {
                format!(
                    "cannot rename {} to {}",
                    self.path.display(),
                    self.rotated.display()
                )
            })?;
            self.file = util::create_private_file(&self.path)
                .with_context(|| format!("cannot write {}", self.path.display()))?;
            self.len = 0;
        }
        self.file
            .write_all(line.as_bytes())
            .with_context(|| format!("cannot write {}", self.path.display()))?;
        self.len += n;
        Ok(())
    }
}

/// A service's log as the supervisor uses it. The first error stops it:
/// the caller prints the one warning it returns, nothing more is logged
/// for that service, and the service keeps running.
pub(crate) struct ServiceLog {
    sub: String,
    file: Option<LogFile>,
}

impl ServiceLog {
    /// Starts the log of `sub`, a service this `up` starts. Returns the
    /// warning to print if it can't.
    pub fn start(paths: &Paths, project: &str, sub: &str) -> (Self, Option<String>) {
        Self::start_capped(paths, project, sub, CAP)
    }

    fn start_capped(paths: &Paths, project: &str, sub: &str, cap: u64) -> (Self, Option<String>) {
        let path = paths.up_log(project, sub);
        let rotated = paths.up_log_rotated(project, sub);
        let mut log = Self {
            sub: sub.to_string(),
            file: None,
        };
        match LogFile::start(path, rotated, cap) {
            Ok(f) => {
                log.file = Some(f);
                (log, None)
            }
            Err(e) => {
                let w = log.stopped(&e);
                (log, Some(w))
            }
        }
    }

    /// `<timestamp> <text>`: a line the service wrote, without the
    /// `name | ` prefix.
    pub fn line(&mut self, text: &str) -> Option<String> {
        self.write(text)
    }

    /// `<timestamp> lokal: <text>`: lokal's own line about the service.
    pub fn lokal(&mut self, text: &str) -> Option<String> {
        self.write(&format!("lokal: {text}"))
    }

    fn write(&mut self, text: &str) -> Option<String> {
        let f = self.file.as_mut()?;
        match f.write(&format!("{} {text}\n", util::now_rfc3339_micros())) {
            Ok(()) => None,
            Err(e) => {
                self.file = None;
                Some(self.stopped(&e))
            }
        }
    }

    fn stopped(&self, e: &anyhow::Error) -> String {
        format!(
            "{}: {e:#}; not logging it any more (it keeps running)",
            self.sub
        )
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    use super::*;

    /// `<timestamp> ` at the start of a line: RFC 3339 UTC with
    /// microseconds, then a space.
    fn stamped(line: &str) -> &str {
        let (ts, rest) = line.split_at(28);
        let b = ts.as_bytes();
        for (i, c) in b.iter().enumerate() {
            let want_digit = !matches!(i, 4 | 7 | 10 | 13 | 16 | 19 | 26 | 27);
            assert_eq!(c.is_ascii_digit(), want_digit, "{line:?} at {i}");
        }
        assert_eq!(&ts[4..5], "-");
        assert_eq!(&ts[10..11], "T");
        assert_eq!(&ts[19..20], ".");
        assert_eq!(&ts[26..], "Z ");
        rest
    }

    fn lines(path: &Path) -> Vec<String> {
        std::fs::read_to_string(path)
            .unwrap()
            .lines()
            .map(|l| stamped(l).to_string())
            .collect()
    }

    #[test]
    fn each_line_gets_a_timestamp_and_lokals_lines_are_marked() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::sandbox(dir.path().to_path_buf());
        let (mut log, w) = ServiceLog::start(&paths, "shop", "v1.api");
        assert_eq!(w, None);
        assert_eq!(log.lokal("started on port 51234: cargo run"), None);
        assert_eq!(log.line("hello"), None);
        assert_eq!(log.line(""), None);
        let path = paths.up_log("shop", "v1.api");
        assert_eq!(
            lines(&path),
            ["lokal: started on port 51234: cargo run", "hello", ""]
        );
        // Private, in a private folder.
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&path), 0o600);
        assert_eq!(mode(&paths.up_dir("shop")), 0o700);
    }

    #[test]
    fn a_new_run_truncates_the_log_and_drops_the_old_rotated_one() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::sandbox(dir.path().to_path_buf());
        let (path, rotated) = (paths.up_log("shop", "@"), paths.up_log_rotated("shop", "@"));
        let (mut log, _) = ServiceLog::start(&paths, "shop", "@");
        log.line("first run");
        std::fs::write(&rotated, "older\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let (mut log, w) = ServiceLog::start(&paths, "shop", "@");
        assert_eq!(w, None);
        log.line("second run");
        assert_eq!(lines(&path), ["second run"]);
        assert!(!rotated.exists());
        // An existing file is made private again.
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn a_line_that_would_pass_the_cap_starts_a_new_file() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::sandbox(dir.path().to_path_buf());
        let (path, rotated) = (paths.up_log("p", "a"), paths.up_log_rotated("p", "a"));
        // A stamped line of `xx` is 28 + 2 + 1 = 31 bytes: 3 fit in 100.
        let (mut log, _) = ServiceLog::start_capped(&paths, "p", "a", 100);
        for l in ["l1", "l2", "l3"] {
            assert_eq!(log.line(l), None);
        }
        assert_eq!(std::fs::metadata(&path).unwrap().len(), 93);
        assert!(!rotated.exists());
        assert_eq!(log.line("l4"), None);
        assert_eq!(lines(&rotated), ["l1", "l2", "l3"]);
        assert_eq!(lines(&path), ["l4"]);
        for l in ["l5", "l6", "l7"] {
            assert_eq!(log.line(l), None);
        }
        // The next rotation replaces the older `.log.1`.
        assert_eq!(lines(&rotated), ["l4", "l5", "l6"]);
        assert_eq!(lines(&path), ["l7"]);
        for p in [&path, &rotated] {
            assert!(std::fs::metadata(p).unwrap().len() <= 100);
        }
        // As one log, nothing is lost or reordered.
        let all = util::tail_lines_across(&[&rotated, &path], 50);
        let all: Vec<&str> = all.iter().map(|l| stamped(l)).collect();
        assert_eq!(all, ["l4", "l5", "l6", "l7"]);
    }

    #[test]
    fn the_first_error_is_reported_once_and_stops_the_log() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::sandbox(dir.path().to_path_buf());
        let up_dir = paths.up_dir("p");
        let (path, rotated) = (paths.up_log("p", "a"), paths.up_log_rotated("p", "a"));
        let (mut log, _) = ServiceLog::start_capped(&paths, "p", "a", 40);
        assert_eq!(log.line("l1"), None);
        // A folder lokal can't write to: the rotation's rename fails.
        std::fs::set_permissions(&up_dir, std::fs::Permissions::from_mode(0o500)).unwrap();
        let w = log.line("l2");
        let after = log.line("l3");
        std::fs::set_permissions(&up_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        let w = w.expect("a warning");
        assert!(w.starts_with("a: cannot rename "), "{w}");
        assert!(
            w.ends_with("; not logging it any more (it keeps running)"),
            "{w}"
        );
        assert_eq!(after, None, "only one warning");
        assert_eq!(log.lokal("exited 0"), None);
        assert_eq!(lines(&path), ["l1"]);
        assert!(!rotated.exists());
        // A log that can't start says so once, and logs nothing.
        std::fs::set_permissions(&up_dir, std::fs::Permissions::from_mode(0o500)).unwrap();
        let (mut log, w) = ServiceLog::start(&paths, "p", "b");
        std::fs::set_permissions(&up_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        let w = w.expect("a warning");
        assert!(w.starts_with("b: cannot write "), "{w}");
        assert_eq!(log.line("x"), None);
        assert!(!paths.up_log("p", "b").exists());
    }
}

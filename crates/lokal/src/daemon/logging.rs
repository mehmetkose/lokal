//! Daemon logging: `daemon.log`, rotated daily to `daemon.log.1`/`.2`
//! (3 files kept), written through a non-blocking appender so logging never
//! blocks the request path.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use anyhow::Result;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

use crate::paths::Paths;

const KEEP_ROTATED: u32 = 2;

fn today() -> String {
    let d = time::OffsetDateTime::now_utc().date();
    format!("{}-{:02}-{:02}", d.year(), d.month() as u8, d.day())
}

fn file_date(path: &Path) -> Option<String> {
    let m = fs::metadata(path).ok()?.modified().ok()?;
    let d = time::OffsetDateTime::from(m).date();
    Some(format!(
        "{}-{:02}-{:02}",
        d.year(),
        d.month() as u8,
        d.day()
    ))
}

struct DailyFile {
    paths: Paths,
    path: PathBuf,
    date: String,
    file: File,
}

impl DailyFile {
    fn open(paths: &Paths) -> io::Result<Self> {
        fs::create_dir_all(&paths.log_dir)?;
        let path = paths.daemon_log.clone();
        if let Some(d) = file_date(&path)
            && d != today()
        {
            rotate(paths);
        }
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        Ok(Self {
            paths: paths.clone(),
            path,
            date: today(),
            file,
        })
    }
}

fn rotate(paths: &Paths) {
    let _ = fs::remove_file(paths.rotated_log(KEEP_ROTATED));
    for n in (1..KEEP_ROTATED).rev() {
        let _ = fs::rename(paths.rotated_log(n), paths.rotated_log(n + 1));
    }
    let _ = fs::rename(&paths.daemon_log, paths.rotated_log(1));
}

impl Write for DailyFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let t = today();
        if t != self.date {
            rotate(&self.paths);
            self.file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.path)?;
            self.date = t;
        }
        self.file.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

/// Installs the global subscriber. Keep the guard alive for the process
/// lifetime (it flushes on drop).
pub fn init(
    paths: &Paths,
    foreground: bool,
) -> Result<tracing_appender::non_blocking::WorkerGuard> {
    let file = DailyFile::open(paths)?;
    let (writer, guard) = tracing_appender::non_blocking::NonBlockingBuilder::default()
        .lossy(true)
        .finish(file);
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let file_layer = tracing_subscriber::fmt::layer()
        .with_writer(writer)
        .with_ansi(false)
        .with_target(false);
    let stderr_layer = foreground.then(|| {
        tracing_subscriber::fmt::layer()
            .with_writer(io::stderr)
            .with_target(false)
    });
    tracing_subscriber::registry()
        .with(filter)
        .with(file_layer)
        .with(stderr_layer)
        .try_init()
        .ok();
    Ok(guard)
}

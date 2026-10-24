//! The one place lokal runs other programs (SPEC §10.1, §16.7): argv only,
//! never `sh -c`, and the exact argv is logged at debug level. lokal's own
//! shell-outs capture their output (only `sudo` gets the terminal). The one
//! other kind is [`spawn_inherited`]: the user's own command for `lokal run`
//! (SPEC §4.10), also argv-only, with the terminal attached.

use std::ffi::OsStr;
use std::process::{Command, ExitStatus, Stdio};

use anyhow::{Context, Result, bail};

#[derive(Debug)]
pub struct CmdOutput {
    pub status: ExitStatus,
    pub stdout: String,
    pub stderr: String,
}

impl CmdOutput {
    pub fn success(&self) -> bool {
        self.status.success()
    }
}

fn describe<S: AsRef<OsStr>>(cmd: &str, args: &[S]) -> String {
    let mut s = cmd.to_string();
    for a in args {
        s.push(' ');
        let a = a.as_ref().to_string_lossy();
        if a.is_empty() || a.contains([' ', '"', '\'']) {
            s.push_str(&format!("{a:?}"));
        } else {
            s.push_str(&a);
        }
    }
    s
}

/// Runs `cmd args…` to completion and captures its output, whatever the
/// exit status.
pub fn run<S: AsRef<OsStr>>(cmd: &str, args: &[S]) -> Result<CmdOutput> {
    let line = describe(cmd, args);
    tracing::debug!(argv = %line, "run");
    let out = Command::new(cmd)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("failed to run `{line}`"))?;
    let res = CmdOutput {
        status: out.status,
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    };
    tracing::debug!(argv = %line, status = %res.status, "done");
    Ok(res)
}

/// Like [`run`], but a non-zero exit is an error that includes the argv,
/// the status, stdout and stderr.
pub fn run_ok<S: AsRef<OsStr>>(cmd: &str, args: &[S]) -> Result<CmdOutput> {
    let out = run(cmd, args)?;
    if !out.success() {
        bail!(
            "`{}` failed ({}){}{}",
            describe(cmd, args),
            out.status,
            fmt_stream("stdout", &out.stdout),
            fmt_stream("stderr", &out.stderr)
        );
    }
    Ok(out)
}

/// Runs with the terminal attached (stdin/stdout/stderr inherited), for
/// `sudo` password prompts.
pub fn run_interactive<S: AsRef<OsStr>>(cmd: &str, args: &[S]) -> Result<ExitStatus> {
    let line = describe(cmd, args);
    tracing::debug!(argv = %line, "run (interactive)");
    Command::new(cmd)
        .args(args)
        .status()
        .with_context(|| format!("failed to run `{line}`"))
}

/// Starts the user's own command for `lokal run` (SPEC §4.10): `argv` as
/// given, never through a shell, with stdin, stdout and stderr inherited (so
/// a dev server keeps its terminal) and in lokal's process group (so the
/// terminal's Ctrl-C reaches it directly). `env` is added to lokal's
/// environment. Must be called inside a tokio runtime. The caller maps
/// `NotFound` to exit 127 and `PermissionDenied` to 126, as shells do.
pub fn spawn_inherited(
    argv: &[String],
    env: &[(String, String)],
) -> std::io::Result<tokio::process::Child> {
    let Some((cmd, args)) = argv.split_first() else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "empty command",
        ));
    };
    let line = describe(cmd, args);
    tracing::debug!(argv = %line, "spawn (terminal attached)");
    tokio::process::Command::new(cmd)
        .args(args)
        .envs(env.iter().map(|(k, v)| (k, v)))
        .spawn()
}

fn fmt_stream(name: &str, s: &str) -> String {
    let s = s.trim();
    if s.is_empty() {
        String::new()
    } else {
        format!("\n{name}: {s}")
    }
}

/// Human-readable command line, for showing the user what will run.
pub fn display_command<S: AsRef<OsStr>>(cmd: &str, args: &[S]) -> String {
    describe(cmd, args)
}

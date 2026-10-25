//! The one place lokal runs other programs (SPEC §10.1, §16.7): an argument
//! vector, never a string-built command, and the exact argv is logged at
//! debug level. lokal's own shell-outs capture their output (only `sudo`
//! gets the terminal). Two kinds start the user's own commands:
//! [`spawn_inherited`], for `lokal run` (SPEC §4.10), argv-only with the
//! terminal attached; and [`spawn_shell`], for `lokal up` (SPEC §4.11).
//!
//! lokal never uses `sh -c`, with one exception (SPEC §16.7):
//! [`spawn_shell`] runs a `run` string from a `lokal.toml` as `/bin/sh`,
//! `-c`, `<run>`, and `lokal up` calls it only after the trust check
//! (§16.11) has passed.

use std::ffi::OsStr;
use std::path::Path;
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

/// Starts one `lokal.toml` service for `lokal up` (SPEC §4.11): the argument
/// vector `/bin/sh`, `-c`, `run`, the one shell lokal starts (SPEC §16.7).
/// `run` is the file's string with `{port}` already replaced, and nothing
/// else is added to it. The caller must have passed the trust check
/// (§16.11) first.
///
/// The shell runs in `cwd`, as the user, with `env` added to lokal's
/// environment, stdin `/dev/null`, stdout and stderr piped to lokal, and in
/// a new process group that it leads (pgid = its pid). lokal signals that
/// group, which also reaches what the shell started (`npm` → `node`), and
/// the terminal's Ctrl-C reaches only lokal. Dropping the child doesn't
/// kill it. Must be called inside a tokio runtime.
// PORTING(windows): `cmd.exe /C <run>` (argv: "cmd.exe", "/C", run), in a job
// object instead of a process group, started with CREATE_NEW_PROCESS_GROUP.
pub fn spawn_shell(
    run: &str,
    env: &[(String, String)],
    cwd: &Path,
) -> std::io::Result<tokio::process::Child> {
    let args = ["-c", run];
    tracing::debug!(
        argv = %describe("/bin/sh", &args),
        cwd = %cwd.display(),
        "spawn (own process group, output piped)"
    );
    tokio::process::Command::new("/bin/sh")
        .args(args)
        .current_dir(cwd)
        .envs(env.iter().map(|(k, v)| (k, v)))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
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

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use tokio::io::AsyncBufReadExt;

    use super::*;
    use crate::platform::{self, GroupSignal};

    /// Kills the test's process group if a failed assertion leaves it. Only
    /// while it exists, so its pgid can't belong to anyone else.
    struct KillGroup(u32);

    impl Drop for KillGroup {
        fn drop(&mut self) {
            if platform::group_alive(self.0) {
                platform::signal_group(self.0, GroupSignal::Kill);
            }
        }
    }

    /// The shell leads a new process group, which holds what it starts, gets
    /// `env` and `cwd`, and outlives its leader while a background job is
    /// left (VERIFY V10).
    #[test]
    fn a_shell_leads_its_own_process_group() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let job = format!("sleep 60.{}{}", std::process::id(), line!());
            let run = format!("{job} & echo \"$$ $X $(pwd)\"; exec sleep 31");
            let env = [("X".to_string(), "from-env".to_string())];
            let mut child = spawn_shell(&run, &env, Path::new("/")).unwrap();
            let pgid = child.id().unwrap();
            let _guard = KillGroup(pgid);
            let mut out = tokio::io::BufReader::new(child.stdout.take().unwrap()).lines();
            let line = out.next_line().await.unwrap().unwrap();
            assert_eq!(line, format!("{pgid} from-env /"), "$$ is the group's id");
            // SAFETY: getpgrp has no preconditions.
            assert_ne!(pgid as i32, unsafe { libc::getpgrp() });
            // The background job is running (and has SIGINT ignored) once
            // it has exec'd: wait for it in the group.
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                let ps = run_ok("/bin/ps", &["-A", "-ww", "-o", "pgid=,command="]).unwrap();
                let want = format!("{pgid} {job}");
                if ps.stdout.lines().any(|l| l.trim() == want) {
                    break;
                }
                assert!(Instant::now() < deadline, "no `{job}`:\n{}", ps.stdout);
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            assert!(platform::group_alive(pgid));
            assert!(platform::signal_group(pgid, GroupSignal::Interrupt));
            let st = child.wait().await.unwrap();
            assert_eq!(platform::shell_exit_code(&st), 130);
            assert!(platform::group_alive(pgid), "the background job is left");
            assert!(platform::signal_group(pgid, GroupSignal::Terminate));
            let deadline = Instant::now() + Duration::from_secs(2);
            while platform::group_alive(pgid) {
                assert!(Instant::now() < deadline, "group {pgid} didn't empty");
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            assert!(!platform::signal_group(pgid, GroupSignal::Kill));
        });
        // Never a group lokal mustn't signal.
        assert!(!platform::group_alive(0) && !platform::group_alive(1));
    }
}

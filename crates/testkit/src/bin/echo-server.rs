//! `echo-server --bind <ip> --port <p> --marker <m>`: prints `LISTENING <addr>`.
//!
//! `--detach` starts the server the way some dev commands do (`astro dev`
//! under a coding agent, `pg_ctl start`, `docker compose up -d`), for `lokal
//! up`'s tests (SPEC §4.11, "A server that detaches"): the server runs in a
//! session of its own (`setsid`), out of its caller's process group, with
//! stdin, stdout and stderr on /dev/null, and the process that started it
//! prints `DETACHED <server pid>` and exits 0.
//! - By default the server binds first, as `astro dev` does: the line is
//!   `DETACHED <pid> LISTENING <addr>`, and the exit comes `--exit-after-ms`
//!   later (default 0).
//! - With `--bind-after-ms <ms>`, the exit comes at once, and the server
//!   binds that long after it.

use std::io::Write;
use std::os::fd::AsRawFd;
use std::time::Duration;

use anyhow::{Context, bail};
use clap::Parser;

#[derive(Parser)]
struct Args {
    #[arg(long, default_value = "127.0.0.1")]
    bind: std::net::IpAddr,
    #[arg(long, default_value_t = 0)]
    port: u16,
    #[arg(long, default_value = "echo")]
    marker: String,
    /// Run the server in a new session and exit 0.
    #[arg(long)]
    detach: bool,
    /// With --detach: exit this long after the server listens.
    #[arg(long, default_value_t = 0, requires = "detach")]
    exit_after_ms: u64,
    /// With --detach: exit at once, and bind this long after.
    #[arg(long, requires = "detach", conflicts_with = "exit_after_ms")]
    bind_after_ms: Option<u64>,
}

fn main() -> anyhow::Result<()> {
    let a = Args::parse();
    if !a.detach {
        return runtime()?.block_on(async move {
            let l = tokio::net::TcpListener::bind((a.bind, a.port)).await?;
            println!("LISTENING {}", l.local_addr()?);
            std::io::stdout().flush()?;
            testkit::echo::serve(l, a.marker).await;
            Ok(())
        });
    }
    match a.bind_after_ms {
        None => {
            let l = std::net::TcpListener::bind((a.bind, a.port))?;
            let addr = l.local_addr()?;
            match detach()? {
                None => serve(l, a.marker),
                Some(pid) => {
                    println!("DETACHED {pid} LISTENING {addr}");
                    std::io::stdout().flush()?;
                    std::thread::sleep(Duration::from_millis(a.exit_after_ms));
                    Ok(())
                }
            }
        }
        Some(ms) => match detach()? {
            None => {
                std::thread::sleep(Duration::from_millis(ms));
                serve(std::net::TcpListener::bind((a.bind, a.port))?, a.marker)
            }
            Some(pid) => {
                println!("DETACHED {pid}");
                std::io::stdout().flush()?;
                Ok(())
            }
        },
    }
}

/// Forks. Returns the child's pid in the parent, and `None` in the child,
/// which is then in a new session with its standard streams on /dev/null.
/// Called while this process has one thread, before any runtime starts.
fn detach() -> anyhow::Result<Option<i32>> {
    // SAFETY: the process is single-threaded here, so the child is a
    // complete copy of it.
    match unsafe { libc::fork() } {
        -1 => bail!("fork: {}", std::io::Error::last_os_error()),
        0 => {
            // SAFETY: setsid has no preconditions; the child of a fork isn't
            // a process group leader, so it succeeds.
            if unsafe { libc::setsid() } == -1 {
                bail!("setsid: {}", std::io::Error::last_os_error());
            }
            let null = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open("/dev/null")
                .context("/dev/null")?;
            for fd in 0..=2 {
                // SAFETY: both are open descriptors of this process.
                if unsafe { libc::dup2(null.as_raw_fd(), fd) } == -1 {
                    bail!("dup2: {}", std::io::Error::last_os_error());
                }
            }
            Ok(None)
        }
        pid => Ok(Some(pid)),
    }
}

/// The detached server, on a listener bound before the runtime starts.
fn serve(l: std::net::TcpListener, marker: String) -> anyhow::Result<()> {
    l.set_nonblocking(true)?;
    runtime()?.block_on(async move {
        testkit::echo::serve(tokio::net::TcpListener::from_std(l)?, marker).await;
        Ok(())
    })
}

fn runtime() -> std::io::Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
}

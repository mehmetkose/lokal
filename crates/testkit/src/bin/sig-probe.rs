//! `sig-probe --port <p>`: a stand-in dev server for `lokal run`'s signal
//! tests (dev-only). It listens on 127.0.0.1:<p>. After the first SIGINT,
//! SIGTERM or SIGHUP it keeps counting those signals for 300ms, prints
//! `signals: <count>` and exits with 40 + count, so a test can tell how many
//! signals reached it: 41 means exactly one.
//!
//! It counts in a plain `sigaction` handler. tokio's signal streams merge
//! signals that arrive before the task runs, which would hide a signal that
//! lokal relays right after the terminal sent it.
//!
//! It first checks that it didn't start with any of SIGINT, SIGTERM, SIGHUP
//! or SIGQUIT ignored: an ignored disposition survives `exec`, so a wrapper
//! that set `SIG_IGN` before starting it would leave a dev server deaf to
//! Ctrl-C. Then it says so and exits 3 without listening.

use std::sync::atomic::{AtomicI32, Ordering};
use std::time::Duration;

use clap::Parser;

#[derive(Parser)]
struct Args {
    #[arg(long, default_value_t = 0)]
    port: u16,
}

static COUNT: AtomicI32 = AtomicI32::new(0);

extern "C" fn on_signal(_: libc::c_int) {
    COUNT.fetch_add(1, Ordering::SeqCst);
}

fn main() -> anyhow::Result<()> {
    let a = Args::parse();
    for (name, sig) in [
        ("SIGINT", libc::SIGINT),
        ("SIGTERM", libc::SIGTERM),
        ("SIGHUP", libc::SIGHUP),
        ("SIGQUIT", libc::SIGQUIT),
    ] {
        if started_ignored(sig) {
            eprintln!("sig-probe: started with {name} ignored (inherited SIG_IGN)");
            std::process::exit(3);
        }
    }
    // Handlers first: once the port accepts, a signal must be counted.
    for sig in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
        // SAFETY: installs a handler that only touches an atomic.
        unsafe {
            let mut sa: libc::sigaction = std::mem::zeroed();
            sa.sa_sigaction = on_signal as *const () as libc::sighandler_t;
            sa.sa_flags = libc::SA_RESTART;
            libc::sigemptyset(&mut sa.sa_mask);
            if libc::sigaction(sig, &sa, std::ptr::null_mut()) != 0 {
                anyhow::bail!("sigaction: {}", std::io::Error::last_os_error());
            }
        }
    }
    let l = std::net::TcpListener::bind(("127.0.0.1", a.port))?;
    std::thread::spawn(move || {
        for s in l.incoming() {
            drop(s);
        }
    });
    while COUNT.load(Ordering::SeqCst) == 0 {
        std::thread::sleep(Duration::from_millis(5));
    }
    std::thread::sleep(Duration::from_millis(300));
    let count = COUNT.load(Ordering::SeqCst);
    println!("signals: {count}");
    std::process::exit(40 + count);
}

/// Was `sig` ignored when this process started?
fn started_ignored(sig: libc::c_int) -> bool {
    // SAFETY: a null new action only reads the current one into `old`.
    unsafe {
        let mut old: libc::sigaction = std::mem::zeroed();
        libc::sigaction(sig, std::ptr::null(), &mut old) == 0 && old.sa_sigaction == libc::SIG_IGN
    }
}

//! VERIFY V1 (SPEC §12): can a user LaunchAgent declare `Sockets` on
//! 127.0.0.1:443 / ::1:443 / :80 and receive them via
//! `launch_activate_socket`? Throwaway experiment, kept as documentation.
//!
//! verify_sockets serve          run under launchd: serve "ok <name>" on every
//!                               handed-over socket (TLS on https4/https6)
//! verify_sockets bind <ip:port> try to bind directly as the user (fallback a)
//!
//! It calls launchd's launch_activate_socket, so it runs on macOS only; elsewhere it says so and
//! exits 2.

#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "macos")]
fn main() {
    macos::main()
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("verify_sockets: VERIFY V1 needs macOS");
    std::process::exit(2);
}

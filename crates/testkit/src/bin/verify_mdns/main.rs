//! VERIFY V2 (SPEC §12): are `dns_sd` LocalOnly A+AAAA records visible to
//! `getaddrinfo`, and how fast? Throwaway experiment, kept as documentation.
//!
//! Usage: verify_mdns <unique|shared> [hold-secs] [name]
//!   Registers `<name>.local` and `api.<name>.local` (default name v2-<rand>),
//!   reports callback latency and getaddrinfo timings, holds the records for
//!   `hold-secs` so curl/dscacheutil/Chrome can be pointed at them, then exits
//!   (which removes the records).
//!
//! It calls dns_sd (mDNSResponder), so it runs on macOS only; elsewhere it says so and
//! exits 2.

#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "macos")]
fn main() {
    macos::main()
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("verify_mdns: VERIFY V2 needs macOS");
    std::process::exit(2);
}

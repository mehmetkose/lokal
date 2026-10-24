//! Human output: colour (off with `--no-color`, `NO_COLOR`, or a non-TTY
//! stdout), marks, aligned step lines, and `error:`/`hint:` on stderr.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::error::UserError;

static COLOR: AtomicBool = AtomicBool::new(false);
static VERBOSE: AtomicBool = AtomicBool::new(false);

pub fn init(no_color: bool, verbose: u8) {
    let env_off = std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty());
    COLOR.store(
        !no_color && !env_off && crate::util::stdout_is_tty(),
        Ordering::Relaxed,
    );
    VERBOSE.store(verbose > 0, Ordering::Relaxed);
}

pub fn color() -> bool {
    COLOR.load(Ordering::Relaxed)
}

pub fn verbose() -> bool {
    VERBOSE.load(Ordering::Relaxed)
}

fn paint(code: &str, s: &str) -> String {
    if color() {
        format!("\x1b[{code}m{s}\x1b[0m")
    } else {
        s.to_string()
    }
}

pub fn green(s: &str) -> String {
    paint("32", s)
}
pub fn red(s: &str) -> String {
    paint("31", s)
}
pub fn yellow(s: &str) -> String {
    paint("33", s)
}
pub fn dim(s: &str) -> String {
    paint("2", s)
}
pub fn bold(s: &str) -> String {
    paint("1", s)
}

pub fn ok_mark() -> String {
    green("✓")
}
pub fn fail_mark() -> String {
    red("✗")
}
pub fn warn_mark() -> String {
    yellow("!")
}

/// `  ✓ Created certificate authority                    12ms`
pub fn step(mark: &str, label: &str, d: Option<Duration>) {
    let t = d.map(crate::util::fmt_duration).unwrap_or_default();
    let width = 62usize;
    let pad = width.saturating_sub(label.chars().count());
    println!("  {mark} {label}{}{}", " ".repeat(pad.max(1)), dim(&t));
}

pub fn note(msg: &str) {
    println!("  {} {msg}", dim("note:"));
}

/// [`note`] on stderr, for commands whose stdout belongs to someone else
/// (`lokal run`).
pub fn enote(msg: &str) {
    eprintln!("  {} {msg}", dim("note:"));
}

pub fn debug(msg: impl AsRef<str>) {
    if verbose() {
        eprintln!("{}", dim(&format!("debug: {}", msg.as_ref())));
    }
}

/// Prints an error as `error: …` (+ `hint: …`) and returns its exit code.
pub fn report_error(e: &anyhow::Error) -> i32 {
    if let Some(u) = e.downcast_ref::<UserError>() {
        eprintln!("{} {}", red("error:"), u.message);
        if let Some(h) = &u.hint {
            eprintln!("{} {h}", yellow("hint:"));
        }
        return u.code;
    }
    eprintln!("{} {e:#}", red("error:"));
    crate::error::EXIT_ERROR
}

/// Pretty JSON with `"schema": 1` already inside `v`.
pub fn print_json(v: &serde_json::Value) {
    println!("{}", serde_json::to_string_pretty(v).unwrap_or_default());
}

//! User-facing errors with an exit code and an optional hint (SPEC §4.5, §4.9).

use std::fmt;

pub const EXIT_OK: i32 = 0;
pub const EXIT_ERROR: i32 = 1;
pub const EXIT_USAGE: i32 = 2;
pub const EXIT_NOT_SET_UP: i32 = 3;
pub const EXIT_SUDO_FORBIDDEN: i32 = 4;

pub const NOT_SET_UP: &str =
    "lokal isn't set up yet — run 'lokal setup' (one time, takes about 10 seconds)";

#[derive(Debug, Clone)]
pub struct UserError {
    pub code: i32,
    pub message: String,
    pub hint: Option<String>,
}

impl UserError {
    pub fn new(code: i32, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            hint: None,
        }
    }

    /// A validation error (bad argument value).
    pub fn usage(message: impl Into<String>) -> Self {
        Self::new(EXIT_USAGE, message)
    }

    pub fn runtime(message: impl Into<String>) -> Self {
        Self::new(EXIT_ERROR, message)
    }

    pub fn not_set_up() -> Self {
        Self::new(EXIT_NOT_SET_UP, NOT_SET_UP)
    }

    pub fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
}

impl fmt::Display for UserError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for UserError {}

/// Shorthand: `Err(user(...))?`.
pub fn usage(message: impl Into<String>) -> anyhow::Error {
    UserError::usage(message).into()
}

//! lokal: local `.lokal` (and `.local`) domains with valid HTTPS for every service you run
//! in development. The specification is `docs/SPEC.md`; `docs/ARCHITECTURE.md` maps the code.

pub mod ca;
pub mod cli;
pub mod daemon;
pub mod dns;
pub mod doctor;
pub mod error;
pub mod ipc;
pub mod net;
pub mod paths;
pub mod platform;
pub mod process;
pub mod proxy;
pub mod runner;
pub mod setup;
pub mod state;
pub mod util;
pub mod version;

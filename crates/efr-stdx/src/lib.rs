//! Small extensions of `std` that every efr crate may use.
//!
//! This crate is the one place where the workspace touches ambient process state: the
//! wall clock, timers, OS randomness, environment variables, child processes and the
//! XDG directories. `clippy.toml` denies the `std` and tokio entry points for these
//! everywhere else and names the replacement in this crate, so other crates receive
//! time and randomness by injection and their tests never wait on real time.
//!
//! Allowed dependencies: no workspace crate, ever, because every crate depends on this
//! one. What does not belong here: anything that knows about the protocol, the
//! database, screens, shells or providers.
//!
//! The modules are public because their paths are the replacements that `clippy.toml`
//! names, such as `efr_stdx::process::command` and `efr_stdx::env::var`.

// NOTE: missing_docs is set here and not in Cargo.toml, because Cargo rejects a
// `[lints]` table that both inherits the workspace lints and adds its own.
#![warn(missing_docs)]

pub mod env;
mod error;
pub mod fs;
pub mod id;
pub mod paths;
pub mod process;
pub mod rng;
pub mod thread;
pub mod time;

pub use error::StdxError;

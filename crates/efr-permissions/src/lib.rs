//! Pure permission policy: may this tool call run, must the user approve it, or is it
//! refused?
//!
//! - [`PathClass`] and [`Locations`]: every path a call touches falls into one of five
//!   classes (Scratch, UserConfig, UserData, System, Secrets), decided by the target
//!   path and never by the shell's working directory.
//!
//! Allowed dependencies: `efr-protocol` only, for `Scope`, `Origin` and `ProjectId`.
//! What does not belong here: the file system, git, the clock, the project registry
//! file (`efr-scope`) and the approval flow (`efr-conversation`).

// NOTE: missing_docs is set here and not in Cargo.toml, because Cargo rejects a
// `[lints]` table that both inherits the workspace lints and adds its own.
#![warn(missing_docs)]

mod error;
mod path_class;

pub use error::PermissionsError;
pub use path_class::{Locations, PathClass};

//! Where a turn stands: the shell's working directory turned into an
//! `efr_protocol::Scope`, again on every turn.
//!
//! - [`Registry`]: the explicit project registry, `$XDG_CONFIG_HOME/efr/projects.toml`.
//! - [`Home`]: the home directory, always passed in, never read from the environment.
//!
//! Allowed dependencies: `efr-protocol` (`Scope`, `ProjectId`) and `efr-stdx` (the
//! process constructor, the clock for git timeouts, atomic writes). What does not
//! belong here: permissions (`efr-permissions` decides with the `Scope` this crate
//! derives), memory, and the events that record a scope change.

// NOTE: missing_docs is set here and not in Cargo.toml, because Cargo rejects a
// `[lints]` table that both inherits the workspace lints and adds its own.
#![warn(missing_docs)]

mod error;
mod home;
mod registry;
#[cfg(test)]
mod testing;

pub use error::ScopeError;
pub use home::Home;
pub use registry::{Project, REGISTRY_FILE, Registry, RegistryProblem};

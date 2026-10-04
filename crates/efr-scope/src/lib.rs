//! Where a turn stands: the shell's working directory turned into an
//! `efr_protocol::Scope`, again on every turn.
//!
//! - [`Registry`]: the explicit project registry, `$XDG_CONFIG_HOME/efr/projects.toml`.
//! - [`Git`]: guarded git discovery with `GIT_CEILING_DIRECTORIES`, so a dotfiles
//!   `~/.git` never turns every directory under `$HOME` into one repository.
//! - [`detect_dotfiles`]: the dotfiles layouts that make `$HOME` a work tree (a
//!   `~/.git`, yadm, a bare repository with `core.worktree=$HOME`), which plain git
//!   discovery mostly cannot see.
//! - [`Home`]: the home directory, always passed in, never read from the environment.
//!
//! Allowed dependencies: `efr-protocol` (`Scope`, `ProjectId`) and `efr-stdx` (the
//! process constructor, the clock for git timeouts, atomic writes). What does not
//! belong here: permissions (`efr-permissions` decides with the `Scope` this crate
//! derives), memory, and the events that record a scope change.

// NOTE: missing_docs is set here and not in Cargo.toml, because Cargo rejects a
// `[lints]` table that both inherits the workspace lints and adds its own.
#![warn(missing_docs)]

mod dotfiles;
mod error;
mod git;
mod home;
mod registry;
#[cfg(test)]
mod testing;

pub use dotfiles::{Dotfiles, detect_dotfiles};
pub use error::ScopeError;
pub use git::{DEFAULT_GIT_TIMEOUT, Discovery, Git, Repo};
pub use home::Home;
pub use registry::{Project, REGISTRY_FILE, Registry, RegistryProblem};

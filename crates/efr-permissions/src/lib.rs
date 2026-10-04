//! Pure permission policy: may this tool call run, must the user approve it, or is it
//! refused?
//!
//! - [`PathClass`] and [`Locations`]: every path a call touches falls into one of five
//!   classes (Scratch, UserConfig, UserData, System, Secrets), decided by the target
//!   path and never by the shell's working directory.
//! - [`DecisionInput`]: the call's declared [`Requirements`], the turn's `Scope` and
//!   `Origin`, and the [`ConversationPolicy`] (its `$SCRATCH` and its own rules).
//! - [`Policy`]: an ordered list of [`Rule`]s, each an [`Action`], a [`Resource`] and
//!   an [`Effect`]; the last rule that matches wins.
//! - [`Engine::decide`]: the [`Decision`], `Allow`, `Ask` or `Deny`, with a [`Reason`]
//!   for every requirement.
//!
//! `efr-tools` declares what a call needs, this crate decides, and
//! `efr-conversation/src/turn.rs` enforces; `xtask/src/deps.rs` forbids
//! `efr-tools -> efr-permissions`, so a tool can never grant itself anything.
//!
//! Allowed dependencies: `efr-protocol` only, for `Scope`, `Origin` and `ProjectId`.
//! What does not belong here: the file system, git, the clock, the project registry
//! file (`efr-scope`) and the approval flow (`efr-conversation`). Every test is a table
//! of inputs and decisions.

// NOTE: missing_docs is set here and not in Cargo.toml, because Cargo rejects a
// `[lints]` table that both inherits the workspace lints and adds its own.
#![warn(missing_docs)]

mod decision;
mod engine;
mod error;
mod path_class;
mod policy;
mod request;

pub use decision::{Cause, Decision, Effect, Layer, Reason, Subject};
pub use engine::Engine;
pub use error::PermissionsError;
pub use path_class::{Locations, PathClass};
pub use policy::{Action, CommandPattern, Policy, Resource, Rule};
pub use request::{Access, ConversationPolicy, DecisionInput, PathAccess, Requirements};

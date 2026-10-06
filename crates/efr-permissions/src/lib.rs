//! Pure permission policy: may this tool call run, must the user approve it, or is it
//! refused?
//!
//! - [`PathClass`] and [`Locations`]: every path a call touches falls into one of five
//!   classes (Scratch, UserConfig, UserData, System, Secrets), decided by the target
//!   path and never by the shell's working directory.
//! - [`DecisionInput`]: the call's declared [`Requirements`], the turn's `Scope`,
//!   `Origin` and permission `Mode`, and the [`ConversationPolicy`] (its `$SCRATCH`
//!   and its own rules).
//! - [`Policy`]: an ordered list of [`Rule`]s, each an [`Action`], a [`Resource`] and
//!   an [`Effect`]; the last rule that matches wins. [`Policy::base`] is the built-in
//!   policy of each mode (`manual`, `cautious`, `auto`), and a [`CommandPattern`]
//!   judges one simple command of a line at a time, in a place ([`Under`]) and with a
//!   [`Check`] where words alone cannot say enough; a line it cannot split is a
//!   [`Construct`] and asks by default.
//! - [`Engine::decide`]: the [`Decision`], `Allow`, `Contain`, `Ask` or `Deny`, with a
//!   [`Reason`] for every requirement, by the mode's policy and then the user's rules;
//!   [`effective_mode`] caps the mode of a remote turn at `cautious`.
//! - The `auto` mode: a shell call runs contained in a kernel sandbox, and
//!   [`exits::predict`] finds what leaves it, each an [`ExitNeed`] that asks, with the
//!   [`AutoSupport`] of the machine and the [`CallFacts`] that the daemon collected.
//!   [`exits::unsandboxed_line_problem`] is the one-command rule of an exit that runs
//!   outside the sandbox. [`secret_paths`], [`SANDBOX_MASKS`], [`PROTECTED_NAMES`] and
//!   [`PERSISTENCE_FLOORS`] are the tables the engine and the sandbox share.
//!
//! `efr-tools` declares what a call needs, this crate decides, and
//! `efr-conversation/src/turn.rs` enforces; `xtask/src/deps.rs` forbids
//! `efr-tools -> efr-permissions`, so a tool can never grant itself anything.
//!
//! Allowed dependencies: `efr-protocol` only, for `Scope`, `Origin`, `Mode` and
//! `ProjectId`.
//! What does not belong here: the file system, git, the clock, the project registry
//! file (`efr-scope`) and the approval flow (`efr-conversation`). Every test is a table
//! of inputs and decisions.

// NOTE: missing_docs is set here and not in Cargo.toml, because Cargo rejects a
// `[lints]` table that both inherits the workspace lints and adds its own.
#![warn(missing_docs)]

mod command;
mod decision;
mod engine;
mod error;
pub mod exits;
mod path_class;
mod policy;
mod request;
mod support;
mod tables;

pub use command::Construct;
pub use decision::{Cause, Decision, Effect, Layer, Reason, Subject};
pub use engine::{Engine, effective_mode};
pub use error::PermissionsError;
pub use exits::{ExitNeed, PathFacts, WriteBind};
pub use path_class::{Locations, PathClass, secret_paths};
pub use policy::{Action, Check, CommandPattern, Policy, Resource, Rule, Under};
pub use request::{
    Access, CallFacts, ConversationPolicy, DecisionInput, PathAccess, Requirements, SettingsChange,
    TargetKind,
};
pub use support::{AutoSupport, Egress};
pub use tables::{
    PERSISTENCE_FLOORS, PROTECTED_NAMES, SANDBOX_MASKS, persistence_floors, protected_names,
    sandbox_masks,
};

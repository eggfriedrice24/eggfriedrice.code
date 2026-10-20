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
//!   policy of each mode (`manual`, `cautious`, `auto`, `full`), and a
//!   [`CommandPattern`] judges one simple command of a line at a time, in a place
//!   ([`Under`]) and with a [`Check`] where words alone cannot say enough; a line it
//!   cannot split is a [`Construct`] and asks by default.
//! - [`Engine::decide`]: the [`Decision`], `Allow`, `Contain`, `Ask` or `Deny`, with a
//!   [`Reason`] for every requirement, by the mode's policy and then the user's rules;
//!   [`effective_mode`] caps the mode of a remote turn at `cautious`.
//! - The `auto` mode: a shell call runs contained in a kernel sandbox, and
//!   [`exits::predict`] finds what leaves it, each an [`ExitNeed`] that asks, with the
//!   [`AutoSupport`] of the machine and the [`CallFacts`] that the daemon collected.
//!   A view of the host that needs no exit, such as the network view, is a reason with
//!   [`Subject::View`]; [`Decision::view_grants`] gives its grants to the launch.
//!   Each exit has an [`Approver`] and the words that name it; [`exits::names_match`]
//!   decides whether a user message names it.
//!   [`exits::unsandboxed_line_problem`] is the one-command rule of an exit that runs
//!   outside the sandbox. [`secret_paths`], [`SANDBOX_MASKS`], [`PROTECTED_NAMES`],
//!   [`PERSISTENCE_FLOORS`] and [`ZSH_STARTUP_FILES`] are the tables the engine and the
//!   sandbox share.
//! - Full mode: [`Engine::decide`] decides a full call as an `auto` call with
//!   [`AutoSupport::for_full`], then lets it run with no sandbox and no question where
//!   only the sandbox or a built-in row held it; floors, privilege, persistence, a
//!   delete that undo does not cover, the user's rules and the deletes that full mode
//!   guards ([`exits::critical_deletes`], [`CriticalDelete`]) still hold.
//! - Background jobs (phase C): [`job_refusal`] names the exits that no job may hold,
//!   in every mode; outside `auto` a job asks the user before it starts.
//! - The network proxy (phase D2): [`network`] holds the [`Preset`]s and the
//!   [`HostList`] that the config check, the engine and the proxy share, the pacman
//!   mirror parser, and [`network::tokens`], the scans for credentials in files that
//!   the sandbox must read. [`Engine::with_hosts`] gives the engine that list, and
//!   [`CallFacts`] carries the git remotes ([`RemoteFact`], [`RemoteUrl`]) and the
//!   hosts that the proxy refused.
//! - Phase E: [`BusFilter`] holds the rules of the filtered bus proxy (the defaults and
//!   the `[[dbus.allow]]` entries, [`BusRule`]) and the proxy's arguments;
//!   [`AllowRule`] is one `[[sandbox.allow]]` always-allow rule, with its shape check.
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

mod always;
pub mod bus;
mod command;
mod critical;
mod decision;
mod engine;
mod error;
pub mod exits;
mod full;
mod jobs;
pub mod network;
mod path_class;
mod policy;
mod request;
mod support;
mod tables;

pub use always::{AllowFloor, AllowRule, AlwaysVerdict};
pub use bus::{BusFilter, BusRule, check_rules};
pub use command::{Construct, runs_as_another_user};
pub use critical::{CriticalDelete, CriticalTarget, CriticalWhy};
pub use decision::{Cause, Decision, Effect, Layer, Reason, Subject};
pub use engine::{Engine, effective_mode};
pub use error::{AllowRuleProblem, BusRuleProblem, HostError, PermissionsError};
pub use exits::{Approver, ExitNeed, FactRequest, Installed, PathFacts, Rebuild, WriteBind};
pub use jobs::{JOB_REFUSAL, SERVICE_SOCKETS, job_refusal, service_socket};
pub use network::{HostList, HostRule, HostTable, HostVerdict, MirrorHost, PacmanFile, Preset};
pub use path_class::{Locations, PathClass, secret_paths};
pub use policy::{Action, Check, CommandPattern, Policy, Resource, Rule, Under};
pub use request::{
    Access, CallFacts, ConversationPolicy, CoverAsk, Coverage, DecisionInput, GitCodeAsk,
    PathAccess, RemoteFact, RemoteUrl, Requirements, SettingsChange, Source, SourceFact,
    TargetKind,
};
pub use support::{AutoSupport, Egress};
pub use tables::{
    FloorGroup, PERSISTENCE_FLOOR_GROUPS, PERSISTENCE_FLOORS, PROTECTED_NAMES, SANDBOX_MASKS,
    ZSH_STARTUP_FILES, persistence_floors, protected_names, sandbox_masks,
};

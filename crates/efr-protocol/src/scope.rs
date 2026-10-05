//! Where a turn acts, where a request comes from, and what a connection may do.
//!
//! `Scope` and `Origin` live in the protocol crate because the permission engine and the
//! daemon both match on them, and the permission engine depends on this crate only.

use std::fmt;
use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::ids::uuid_id;

uuid_id!(
    /// A project that the user registered explicitly in `projects.toml`. A `.git`
    /// directory alone never makes a project, because `$HOME` may be a dotfiles work tree.
    ProjectId
);

/// What a turn is about, derived again from the shell's working directory on every turn.
///
/// On the wire: `{"kind": "machine"}`, `{"kind": "path", "value": "/etc/nixos"}` or
/// `{"kind": "project", "value": "<project id>"}`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
#[non_exhaustive]
pub enum Scope {
    /// The machine as a whole: `$HOME`, `/` and anything outside a known location.
    Machine,
    /// A specific directory that is not a registered project, such as `/etc/nixos`.
    Path(PathBuf),
    /// A registered project.
    Project(ProjectId),
}

/// The kind of surface that a request or a turn comes from. The permission engine is
/// stricter for some origins: a turn from the phone needs approval for anything outside
/// `$SCRATCH`.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Origin {
    /// The zsh plugin: a `,` line or the Ctrl+Space widget.
    Shell,
    /// `efr` typed by hand or run from a script.
    Cli,
    /// The PTY proxy around a visible terminal.
    Proxy,
    /// A phone over the tailnet listener.
    Phone,
}

/// A permission that a connection holds. The daemon checks the scope of every method
/// before it runs; [`ScopeName::for_method`] is the table.
///
/// | Scope | Allows | Phone default |
/// |---|---|---|
/// | `read` | `hello`, list, subscribe, history, `lease.report`, `models.list` | yes |
/// | `operate` | send prompts, interrupt, steer | yes |
/// | `approve` | answer approvals | yes |
/// | `terminal` | attach to, write to and resize a PTY | no, an explicit opt-in |
/// | `admin` | status, login, and later enrollment and settings | never; Unix socket only |
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ScopeName {
    /// List, subscribe and page history, and list the models.
    Read,
    /// Send prompts, interrupt and steer turns.
    Operate,
    /// Answer approval requests.
    Approve,
    /// Attach to, write to and resize PTYs.
    Terminal,
    /// Administer the daemon. Never granted to a remote connection.
    Admin,
}

impl ScopeName {
    /// Every scope, from the least to the most powerful.
    pub const ALL: [ScopeName; 5] = [
        ScopeName::Read,
        ScopeName::Operate,
        ScopeName::Approve,
        ScopeName::Terminal,
        ScopeName::Admin,
    ];

    /// The wire form of the scope, such as `operate`.
    pub const fn as_str(self) -> &'static str {
        match self {
            ScopeName::Read => "read",
            ScopeName::Operate => "operate",
            ScopeName::Approve => "approve",
            ScopeName::Terminal => "terminal",
            ScopeName::Admin => "admin",
        }
    }
}

impl fmt::Display for ScopeName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests;

//! Always-allow rules (phase E, efr's auto spec 11.2): the `[[sandbox.allow]]` tables of
//! `config.toml`.
//!
//! An exit question of an eligible kind offers a third answer, `a` (always). It saves a
//! rule that names a kind and one resource, never a command pattern. A later exit that
//! a rule covers runs with the same widening that a "yes" gives, with no classifier and
//! no question. The rules sit below every floor and above the classifier.
//!
//! This module holds the rule type and the checks of its shape, which need no fact
//! about the machine. The floor checks (`check.rs`) need the engine's locations and the
//! host list: [`Engine::with_allow`](crate::Engine::with_allow) runs them when the rules
//! load, and disables a rule that fails one ([`AllowFloor`]). The matcher (`cover.rs`)
//! runs per call in [`Engine::always`](crate::Engine::always), on the real paths of the
//! call's grants, and says whether a rule covers every exit of the call
//! ([`AlwaysVerdict`]).

mod check;
mod cover;

use std::fmt;
use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::network::{is_ip_literal, is_name};
use crate::{AllowRuleProblem, PermissionsError};

pub(crate) use self::check::check;
pub(crate) use self::cover::{Machine, verdict};

/// One `[[sandbox.allow]]` rule: a kind and a resource, never a command pattern.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum AllowRule {
    /// A `host` exit to one host and port through efr's proxy. It never covers the
    /// whole network, so with `network.mode = "none"` it covers nothing.
    Host {
        /// The host name, matched without case. Never a pattern, an IP address or
        /// `localhost`.
        host: String,
        /// The port.
        #[serde(default = "https_port")]
        port: u16,
    },
    /// A contained `write` exit whose bind lies at or below one directory.
    Write {
        /// The directory, absolute or below `~/`: the file, the parent dir, the new
        /// file or the new dir that the "yes" opened.
        under: String,
        /// When set, the rule covers only turns of this project (its root, absolute or
        /// below `~/`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        project: Option<String>,
    },
    /// A `socket` exit to one socket path, never a link.
    Socket {
        /// The socket, absolute or below `~/`.
        path: String,
    },
}

/// The port of a host rule that names none.
fn https_port() -> u16 {
    443
}

impl AllowRule {
    /// The rule in words, as the card and `efr sandbox allow` show it, such as
    /// `write under ~/notes` or `host download.example.org:443`.
    pub fn describe(&self) -> String {
        match self {
            AllowRule::Host { host, port } => format!("host {host}:{port}"),
            AllowRule::Write { under, project: None } => format!("write under {under}"),
            AllowRule::Write { under, project: Some(project) } => {
                format!("write under {under} in project {project}")
            }
            AllowRule::Socket { path } => format!("socket {path}"),
        }
    }

    /// The checks that need no fact about the machine: the host syntax (one ASCII host
    /// name, never a pattern, an IP address or `localhost`), a port above 0, and paths
    /// that are absolute or below `~/` with no `.` or `..` part. `index` names the rule
    /// in the error.
    pub fn check_shape(&self, index: usize) -> Result<(), PermissionsError> {
        self.problem().map_or(Ok(()), |problem| Err(PermissionsError::AllowRule { index, problem }))
    }

    fn problem(&self) -> Option<AllowRuleProblem> {
        match self {
            AllowRule::Host { host, port } => {
                host_problem(host).or((*port == 0).then_some(AllowRuleProblem::PortZero))
            }
            AllowRule::Write { under, project } => {
                path_problem(under).or_else(|| project.as_deref().and_then(path_problem))
            }
            AllowRule::Socket { path } => path_problem(path),
        }
    }
}

fn host_problem(host: &str) -> Option<AllowRuleProblem> {
    let owned = || host.to_owned();
    if !host.is_ascii() {
        return Some(AllowRuleProblem::BadHost { host: owned() });
    }
    let lower = host.to_ascii_lowercase();
    let name = lower.strip_suffix('.').unwrap_or(&lower);
    if name.contains('*') {
        return Some(AllowRuleProblem::HostPattern { host: owned() });
    }
    if is_ip_literal(name) {
        return Some(AllowRuleProblem::IpLiteral { host: owned() });
    }
    if name == "localhost" || name.ends_with(".localhost") {
        return Some(AllowRuleProblem::Localhost { host: owned() });
    }
    if !is_name(name, false) {
        return Some(AllowRuleProblem::BadHost { host: owned() });
    }
    None
}

fn path_problem(path: &str) -> Option<AllowRuleProblem> {
    let owned = || path.to_owned();
    let rest = if path == "~" {
        ""
    } else if let Some(rest) = path.strip_prefix("~/") {
        rest
    } else if let Some(rest) = path.strip_prefix('/') {
        rest
    } else {
        return Some(AllowRuleProblem::BadPath { path: owned() });
    };
    if path.contains('\0') {
        return Some(AllowRuleProblem::BadPath { path: owned() });
    }
    if rest.split('/').any(|part| part == "." || part == "..") {
        return Some(AllowRuleProblem::DotDot { path: owned() });
    }
    None
}

/// Whether an always-allow rule covers a call (phase E, DE20), from
/// [`Engine::always`](crate::Engine::always).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AlwaysVerdict {
    /// `sandbox.allow[index]` covers every exit of the call: it runs with the widening
    /// of a "yes", with no classifier and no question.
    Covered {
        /// The position of the rule in `sandbox.allow`, counted from 0.
        index: usize,
    },
    /// No rule covers the call; this rule would. An `a` answer saves it. It passed the
    /// same checks as a rule that loads ([`Engine::check_allow`](crate::Engine::check_allow)).
    Offer(AllowRule),
    /// No rule covers the call, and no rule may.
    None,
}

/// Why an always-allow rule fails a floor check (efr's auto spec 11.3, DE23 to DE25,
/// DE44). The engine keeps such a rule, marked disabled with this reason, and never lets
/// it cover a call. A shape error is [`BadShape`](Self::BadShape).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum AllowFloor {
    /// The rule has a bad shape ([`AllowRule::check_shape`]).
    BadShape {
        /// What is wrong.
        problem: AllowRuleProblem,
    },
    /// A `write` rule names the home directory, `/` or a directory above the home
    /// directory.
    AtOrAboveHome,
    /// A `write` rule names a directory outside the home directory, where a contained
    /// call never gets a write grant.
    OutsideHome,
    /// The path is a secret, lies in one, or holds one.
    Secret,
    /// The path is efr's config (`$C`, the files behind its links, `projects.toml`), lies
    /// in it, or holds it.
    Config,
    /// The path is one of efr's own dirs or programs (`$D`, `$S`, `$R`), lies in one, or
    /// holds one.
    EfrDir,
    /// The path lies at or below a sandbox mask.
    Mask,
    /// The path has a `.git` part: a git dir, whose hooks and config run code.
    GitDir,
    /// A `write` rule names a floor or a path below one: `floor` runs code later outside
    /// the sandbox.
    UnderFloor {
        /// The floor.
        floor: PathBuf,
    },
    /// A `write` rule names a directory above a built-in floor of the home directory,
    /// such as `~/.config` above `~/.config/systemd`.
    AboveHomeFloor {
        /// The floor below the directory.
        floor: PathBuf,
    },
    /// The path lies in a folder that a sync service copies off the machine, or a `write`
    /// rule holds one.
    Synced,
    /// A `write` rule names a write root of every contained call, or a directory above
    /// one: the sandbox writes there with no exit.
    WriteRoot {
        /// The write root.
        root: PathBuf,
    },
    /// A `write` rule names a directory that holds more than one registered project.
    ManyProjects,
    /// A `socket` rule names a socket inside a write root, which a contained call can
    /// replace with a link.
    InWriteRoot,
    /// A `socket` rule names the socket of a service manager or a container engine, such
    /// as `systemd/private`, `docker.sock` or `podman.sock`, or a path below
    /// `$XDG_RUNTIME_DIR/systemd/`.
    ServiceSocket,
    /// A `socket` rule names the system bus or a session bus.
    BusSocket,
    /// A `socket` rule names the socket of a credential agent, such as gpg-agent or
    /// ssh-agent.
    CredentialAgent,
    /// A `socket` rule names the control socket of a desktop.
    DesktopSocket,
    /// A `host` rule names a host that `network.deny` holds.
    NetworkDeny,
}

impl fmt::Display for AllowFloor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AllowFloor::BadShape { problem } => write!(f, "{problem}"),
            AllowFloor::AtOrAboveHome => f.write_str("it names ~, / or a dir above ~"),
            AllowFloor::OutsideHome => {
                f.write_str("it names a dir outside ~, where a sandboxed command never writes")
            }
            AllowFloor::Secret => f.write_str("it names, holds or lies in a secret"),
            AllowFloor::Config => f.write_str("it names, holds or lies in efr's config"),
            AllowFloor::EfrDir => f.write_str("it names, holds or lies in efr's own dirs"),
            AllowFloor::Mask => f.write_str("it lies in a sandbox mask"),
            AllowFloor::GitDir => f.write_str("it names a git dir"),
            AllowFloor::UnderFloor { floor } => {
                write!(f, "it lies in {}, which runs code later", floor.display())
            }
            AllowFloor::AboveHomeFloor { floor } => {
                write!(f, "it holds {}, which runs code later", floor.display())
            }
            AllowFloor::Synced => f.write_str("it names, holds or lies in a synced folder"),
            AllowFloor::WriteRoot { root } => {
                write!(f, "it names or holds the write root {}", root.display())
            }
            AllowFloor::ManyProjects => f.write_str("it holds more than one project"),
            AllowFloor::InWriteRoot => {
                f.write_str("the socket lies in a write root, where a command can replace it")
            }
            AllowFloor::ServiceSocket => {
                f.write_str("it names the socket of a service manager or a container engine")
            }
            AllowFloor::BusSocket => f.write_str("it names a message bus"),
            AllowFloor::CredentialAgent => f.write_str("it names a credential agent"),
            AllowFloor::DesktopSocket => f.write_str("it names a desktop socket"),
            AllowFloor::NetworkDeny => f.write_str("network.deny holds the host"),
        }
    }
}

#[cfg(test)]
mod tests;

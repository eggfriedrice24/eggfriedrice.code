//! Always-allow rules (phase E, efr's auto spec 11.2): the `[[sandbox.allow]]` tables of
//! `config.toml`.
//!
//! An exit question of an eligible kind offers a third answer, `a` (always). It saves a
//! rule that names a kind and one resource, never a command pattern. A later exit that
//! a rule covers runs with the same widening that a "yes" gives, with no classifier and
//! no question. The rules sit below every floor and above the classifier.
//!
//! This module holds the rule type and the checks of its shape, which need no fact
//! about the machine. The floor checks need the engine's locations and come with the
//! matcher.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::network::{is_ip_literal, is_name};
use crate::{AllowRuleProblem, PermissionsError};

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

#[cfg(test)]
mod tests;

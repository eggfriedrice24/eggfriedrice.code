//! The engine's answer: `Allow`, `Ask` or `Deny`, with the reasons behind it.

use std::fmt;
use std::path::PathBuf;

use efr_protocol::Origin;
use serde::{Deserialize, Serialize};

use crate::{Access, Construct, PathClass};

/// What happens to a tool call, ordered from the least to the most strict, so the
/// strictest of several effects is their maximum.
///
/// The enum is deliberately exhaustive: the check point in `efr-conversation` must
/// handle every effect, and a new one must not fall into a wildcard arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Effect {
    /// The call runs.
    Allow,
    /// The call waits until the user approves or denies it.
    Ask,
    /// The call does not run; the model gets an error that names the reason.
    Deny,
}

impl fmt::Display for Effect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Effect::Allow => "allow",
            Effect::Ask => "ask",
            Effect::Deny => "deny",
        })
    }
}

/// The decision about one tool call: the strictest effect of its reasons.
///
/// Only the engine builds decisions, so the effect always agrees with the reasons.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    effect: Effect,
    reasons: Vec<Reason>,
}

impl Decision {
    /// A decision from the reasons for every requirement. No reasons means a bug in the
    /// engine, and the decision fails closed.
    pub(crate) fn from_reasons(reasons: Vec<Reason>) -> Self {
        let effect = reasons.iter().map(|reason| reason.effect).max().unwrap_or(Effect::Deny);
        Decision { effect, reasons }
    }

    /// What happens to the call.
    pub fn effect(&self) -> Effect {
        self.effect
    }

    /// One reason per requirement, in the order the requirements were declared, then
    /// the reason for an interactive call.
    pub fn reasons(&self) -> &[Reason] {
        &self.reasons
    }

    /// The reasons that set the effect: why the call is denied, or what needs approval.
    pub fn deciding(&self) -> impl Iterator<Item = &Reason> {
        self.reasons.iter().filter(move |reason| reason.effect == self.effect)
    }
}

/// Why one requirement got its effect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reason {
    /// The requirement.
    pub subject: Subject,
    /// The effect for this requirement alone.
    pub effect: Effect,
    /// What set the effect.
    pub cause: Cause,
}

/// The requirement that a [`Reason`] is about.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Subject {
    /// A path that the call reads or writes.
    Path {
        /// The path in lexical normal form, or as declared when it is relative.
        path: PathBuf,
        /// Read or write.
        access: Access,
        /// The class, or `None` for a relative path.
        class: Option<PathClass>,
    },
    /// A command line that the call runs.
    Command {
        /// The line as the model wrote it.
        line: String,
    },
    /// Network access by the call itself.
    Network,
    /// The call may wait for input at the terminal.
    Interactive,
    /// The call declared no requirement.
    Nothing,
}

/// What set the effect of a [`Reason`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Cause {
    /// A rule matched, and it was the last rule to match.
    Rule {
        /// The policy that holds the rule.
        layer: Layer,
        /// The rule's position in that policy, counted from 0.
        index: usize,
    },
    /// A rule matched one simple command of a command line with several, and that
    /// command set the line's effect.
    Part {
        /// The policy that holds the rule.
        layer: Layer,
        /// The rule's position in that policy, counted from 0.
        index: usize,
        /// The simple command, its words joined by spaces.
        part: String,
    },
    /// The command line holds something that no command rule can judge, so only a rule
    /// for every command line matched it, and that rule decided.
    Opaque {
        /// What the line holds.
        construct: Construct,
        /// The policy that holds the rule.
        layer: Layer,
        /// The rule's position in that policy, counted from 0.
        index: usize,
    },
    /// The command line runs a program as another user, which always needs approval.
    Privileged {
        /// The program, such as `sudo`.
        program: String,
    },
    /// A rule allowed reading everything below the path, but a secret lies below it,
    /// so the user must approve.
    ReachesSecret {
        /// The first secret below the path that no rule allows.
        secret: PathBuf,
    },
    /// The path holds efr's own credentials, which no rule opens.
    Sealed,
    /// No rule matched, so the engine refused.
    NoRule,
    /// The path is relative, so its class is unknown.
    NotAbsolute,
    /// A rule allowed the requirement, or the call declared none, but the turn comes from
    /// a remote origin, which needs approval for everything outside `$SCRATCH`.
    RemoteOrigin {
        /// The origin.
        origin: Origin,
    },
    /// The call may wait for input at the terminal, so the user must be there.
    Interactive,
    /// The call declared nothing that needs a decision.
    NoRequirements,
}

/// The policy that a rule belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Layer {
    /// The engine's policy: the defaults plus the user's configured rules.
    Machine,
    /// The rules of one conversation.
    Conversation,
}

impl fmt::Display for Layer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Layer::Machine => "machine",
            Layer::Conversation => "conversation",
        })
    }
}

/// One line for a log, an approval summary or the error that the model sees, such as
/// `write /home/u/.zshrc (user config): ask, by rule 4 of the machine policy`.
impl fmt::Display for Reason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.subject, self.effect)?;
        match &self.cause {
            Cause::Rule { layer, index } => write!(f, ", by rule {index} of the {layer} policy"),
            Cause::Part { layer, index, part } => {
                write!(f, ", by rule {index} of the {layer} policy for {part:?}")
            }
            Cause::Opaque { construct, layer, index } => write!(
                f,
                ", by rule {index} of the {layer} policy, because the line holds {construct}, \
                 which no command rule can judge"
            ),
            Cause::Privileged { program } => {
                write!(f, ", because {program} runs commands as another user")
            }
            Cause::ReachesSecret { secret } => {
                write!(f, ", because the secret {} lies below it", secret.display())
            }
            Cause::Sealed => {
                f.write_str(", because efr keeps its own credentials there and no rule opens them")
            }
            Cause::NoRule => f.write_str(", because no rule matched"),
            Cause::NotAbsolute => f.write_str(", because the path is not absolute"),
            Cause::RemoteOrigin { origin } if self.subject == Subject::Nothing => write!(
                f,
                ", because the turn comes from {} and the call declares nothing to judge",
                origin_name(*origin)
            ),
            Cause::RemoteOrigin { origin } => {
                write!(
                    f,
                    ", because the turn comes from {} and is outside $SCRATCH",
                    origin_name(*origin)
                )
            }
            Cause::Interactive => f.write_str(", because the user must answer at the terminal"),
            Cause::NoRequirements => Ok(()),
        }
    }
}

impl fmt::Display for Subject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Subject::Path { path, access, class: Some(class) } => {
                write!(f, "{access} {} ({class})", path.display())
            }
            Subject::Path { path, access, class: None } => write!(f, "{access} {}", path.display()),
            Subject::Command { line } => write!(f, "run {line:?}"),
            Subject::Network => f.write_str("network access"),
            Subject::Interactive => f.write_str("input at the terminal"),
            Subject::Nothing => f.write_str("no requirements"),
        }
    }
}

fn origin_name(origin: Origin) -> &'static str {
    match origin {
        Origin::Shell => "the shell",
        Origin::Cli => "the CLI",
        Origin::Proxy => "the PTY proxy",
        Origin::Phone => "the phone",
        _ => "a remote client",
    }
}

#[cfg(test)]
mod tests;

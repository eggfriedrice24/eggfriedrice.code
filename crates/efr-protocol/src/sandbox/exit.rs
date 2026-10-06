//! Exits: actions that leave the sandbox's envelope, what the model may ask for, and
//! what an approval question shows about them.

use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{BusKind, Grant, Judgement, Launch, ProtocolError};

/// The kind of an exit: an action that the sandbox does not allow on its own.
///
/// Each kind has its own launch after a "yes" and its own approvers. A floor kind
/// ([`ExitKind::is_floor`]) is denied before any question.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ExitKind {
    /// A write outside the write roots.
    Write,
    /// A network host (phase 1: any network use).
    Host,
    /// A view of the host's network (`ss`, `ip addr`).
    HostView,
    /// A Unix socket of a tool.
    Socket,
    /// A desktop's IPC socket (`swaymsg`, `xrandr`): it can type or run commands.
    DesktopIpc,
    /// A message bus.
    Bus,
    /// A device node.
    Device,
    /// A read of a sandbox mask, such as a project `.env`.
    MaskedRead,
    /// A destructive action inside the write roots, such as `git reset --hard`.
    Destructive,
    /// The model asked to run outside the sandbox.
    Outside,
    /// `sudo` and other ways to more rights.
    Privilege,
    /// A change that runs later: rc files, services, cron, a floor path.
    Persistence,
    /// Data that leaves the machine with credentials: `git push`, a publish, a POST.
    Upload,
    /// A write to a folder that a sync service copies off the machine.
    SyncedWrite,
    /// A write at or above a write root, such as `rm -rf ..`.
    AboveRoot,
    /// A read or write of an engine secret: denied.
    Secret,
    /// A write of efr's config: denied.
    Config,
}

impl ExitKind {
    /// Every kind, in declaration order.
    pub const ALL: [ExitKind; 17] = [
        ExitKind::Write,
        ExitKind::Host,
        ExitKind::HostView,
        ExitKind::Socket,
        ExitKind::DesktopIpc,
        ExitKind::Bus,
        ExitKind::Device,
        ExitKind::MaskedRead,
        ExitKind::Destructive,
        ExitKind::Outside,
        ExitKind::Privilege,
        ExitKind::Persistence,
        ExitKind::Upload,
        ExitKind::SyncedWrite,
        ExitKind::AboveRoot,
        ExitKind::Secret,
        ExitKind::Config,
    ];

    /// The wire form, such as `masked_read`.
    pub const fn as_str(self) -> &'static str {
        match self {
            ExitKind::Write => "write",
            ExitKind::Host => "host",
            ExitKind::HostView => "host_view",
            ExitKind::Socket => "socket",
            ExitKind::DesktopIpc => "desktop_ipc",
            ExitKind::Bus => "bus",
            ExitKind::Device => "device",
            ExitKind::MaskedRead => "masked_read",
            ExitKind::Destructive => "destructive",
            ExitKind::Outside => "outside",
            ExitKind::Privilege => "privilege",
            ExitKind::Persistence => "persistence",
            ExitKind::Upload => "upload",
            ExitKind::SyncedWrite => "synced_write",
            ExitKind::AboveRoot => "above_root",
            ExitKind::Secret => "secret",
            ExitKind::Config => "config",
        }
    }

    /// True for a kind that is denied before any question: no grant, no classifier and
    /// no "yes" opens it.
    pub const fn is_floor(self) -> bool {
        matches!(self, ExitKind::Secret | ExitKind::Config)
    }

    /// True for a kind that only the user may approve, whatever its target: the
    /// classifier is never asked. A `write` can be user only too, for a system or
    /// masked target, which [`ExitInfo::user_only`] records per question.
    pub const fn user_only(self) -> bool {
        matches!(
            self,
            ExitKind::MaskedRead
                | ExitKind::Privilege
                | ExitKind::Persistence
                | ExitKind::Upload
                | ExitKind::SyncedWrite
                | ExitKind::DesktopIpc
                | ExitKind::AboveRoot
        )
    }

    /// True for a kind whose approved launch is [`Launch::Unsandboxed`], the exit
    /// child. A user-only `write` that no contained bind serves runs there too; the
    /// conversation's `grant()` decides that case.
    pub const fn runs_unsandboxed(self) -> bool {
        matches!(
            self,
            ExitKind::Privilege
                | ExitKind::Persistence
                | ExitKind::Upload
                | ExitKind::Outside
                | ExitKind::SyncedWrite
                | ExitKind::AboveRoot
        )
    }
}

impl fmt::Display for ExitKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Where an exit came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ExitSource {
    /// efr read it from the line before the run.
    Predicted,
    /// The model asked for it with the shell tool's `needs`.
    Needs,
}

/// What the model asks for with the shell tool's optional `needs` input, after a call
/// failed in the sandbox. The entries are the model's words: efr normalizes them,
/// merges them with its own prediction and asks the user. `needs` is ignored outside
/// `auto`.
///
/// Empty members are left out on the wire. [`Needs::check`] holds the limits of
/// [`Needs::input_schema`], which the tool's input schema embeds.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct Needs {
    /// Paths to write, at most [`Needs::MAX_WRITE`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub write: Vec<String>,
    /// Hosts to reach, at most [`Needs::MAX_HOSTS`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hosts: Vec<String>,
    /// Unix sockets to connect to, at most [`Needs::MAX_SOCKETS`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sockets: Vec<String>,
    /// A message bus.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bus: Option<BusKind>,
    /// A device node.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    /// Masked paths to read, at most [`Needs::MAX_UNMASK`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unmask: Vec<String>,
    /// Run outside the sandbox.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub outside: bool,
    /// The model's reason, at most [`Needs::MAX_REASON`] characters. The user sees it,
    /// labelled as the model's; the classifier never does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl Needs {
    /// The most paths in `write`.
    pub const MAX_WRITE: usize = 8;
    /// The most hosts in `hosts`.
    pub const MAX_HOSTS: usize = 8;
    /// The most sockets in `sockets`.
    pub const MAX_SOCKETS: usize = 2;
    /// The most paths in `unmask`.
    pub const MAX_UNMASK: usize = 4;
    /// The most characters in `reason`.
    pub const MAX_REASON: usize = 300;

    /// True when the model asks for nothing; a reason alone asks for nothing.
    pub fn is_empty(&self) -> bool {
        self.write.is_empty()
            && self.hosts.is_empty()
            && self.sockets.is_empty()
            && self.bus.is_none()
            && self.device.is_none()
            && self.unmask.is_empty()
            && !self.outside
    }

    /// Checks the limits of the input schema, for input that did not come through a
    /// validating client.
    pub fn check(&self) -> Result<(), ProtocolError> {
        let lists = [
            ("write", self.write.len(), Self::MAX_WRITE),
            ("hosts", self.hosts.len(), Self::MAX_HOSTS),
            ("sockets", self.sockets.len(), Self::MAX_SOCKETS),
            ("unmask", self.unmask.len(), Self::MAX_UNMASK),
        ];
        for (field, len, max) in lists {
            if len > max {
                return Err(ProtocolError::InvalidNeeds { field, max });
            }
        }
        if self.reason.as_deref().is_some_and(|reason| reason.chars().count() > Self::MAX_REASON) {
            return Err(ProtocolError::InvalidNeeds { field: "reason", max: Self::MAX_REASON });
        }
        Ok(())
    }

    /// The JSON schema of the `needs` member of the shell tool's input, as the model
    /// sees it.
    pub fn input_schema() -> Value {
        let paths =
            |max: usize| json!({ "type": "array", "items": { "type": "string" }, "maxItems": max });
        json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "write": paths(Self::MAX_WRITE),
                "hosts": paths(Self::MAX_HOSTS),
                "sockets": paths(Self::MAX_SOCKETS),
                "bus": { "enum": ["system", "session"] },
                "device": { "type": "string" },
                "unmask": paths(Self::MAX_UNMASK),
                "outside": { "type": "boolean" },
                "reason": { "type": "string", "maxLength": Self::MAX_REASON },
            },
        })
    }
}

/// What an approval question for an exit shows, on top of its one-line summary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExitInfo {
    /// The exits of the call, each kind once.
    pub kinds: Vec<ExitKind>,
    /// How the call runs after a "yes".
    pub launch: Launch,
    /// Exactly what a "yes" opens; empty for an unsandboxed launch.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub grants: Vec<Grant>,
    /// Facts that efr collected itself, one line each, such as `runs /usr/bin/python,
    /// not .venv/bin/python` or `untrusted: written in the sandbox`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub facts: Vec<String>,
    /// The model's reason from `needs`, shown labelled as the model's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_reason: Option<String>,
    /// The classifier's answer, when it judged first and asked the user (phase 3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub judged: Option<Judgement>,
    /// True when only the user may approve this exit.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub user_only: bool,
}

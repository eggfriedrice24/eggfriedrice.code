//! Events: the facts the daemon records in its event log and sends to subscribers.

use std::path::PathBuf;
use std::str::FromStr;

use jiff::Timestamp;
use schemars::JsonSchema;
use serde::de::{self, Deserializer};
use serde::{Deserialize, Serialize, Serializer};
use serde_json::{Map, Value};

use crate::{
    CallId, CommandId, ConversationId, EffectiveSettings, ErrorBody, Origin, PtyId, Scope, Seq,
    ShellContext, TurnId, TurnSettings,
};

/// Something that happened, as the event log records it and subscribers receive it.
///
/// Variants are named `<Noun><PastParticiple>`. Each one carries the full state of what it
/// reports, never a delta against an earlier event, so a subscriber can resume from any
/// sequence number without reading anything else.
///
/// On the wire an event is one object whose `kind` member is the snake_case variant
/// name, such as `{"kind": "turn_started", ...}`. A kind that this build does not know
/// decodes as [`Event::Unknown`] and encodes back to the same JSON, so an old reader
/// keeps advancing its cursor past events from a newer daemon. A known kind with a
/// malformed body is an error, not `Unknown`.
///
/// A new variant needs a fixture in `fixtures/v1/events/`; a test fails until it has one.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, strum::EnumDiscriminants,
)]
#[serde(tag = "kind", rename_all = "snake_case")]
// NOTE: `EventKind` names every known kind without a hand-kept list. `Unknown` uses it
// to refuse a known kind, which keeps a malformed known event an error.
#[strum_discriminants(
    name(EventKind),
    vis(pub(crate)),
    derive(strum::EnumString, strum::IntoStaticStr),
    strum(serialize_all = "snake_case")
)]
#[non_exhaustive]
pub enum Event {
    /// A conversation began.
    ConversationCreated {
        /// The surface that started it.
        origin: Origin,
        /// The terminal it is the active conversation of, when it came from a shell.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tty: Option<String>,
    },

    /// A prompt was accepted and waits for its turn. Prompts sent while a turn runs queue
    /// behind it.
    PromptQueued {
        /// The turn that will answer the prompt.
        turn_id: TurnId,
        /// The client's id for the `prompt.send` that carried it.
        command_id: CommandId,
        /// The prompt text.
        text: String,
        /// The surface that sent it.
        origin: Origin,
        /// The user's shell when the prompt was sent. A [`ShellContext`] holds no command
        /// line, so this event, which the log keeps forever and every subscriber receives,
        /// never carries the last command of `prompt.send`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        context: Option<ShellContext>,
        /// The settings that the prompt asked for, kept with it while it waits, so a held
        /// prompt runs with them too. Absent means none.
        #[serde(default, skip_serializing_if = "TurnSettings::is_empty")]
        settings: TurnSettings,
    },

    /// A queued prompt is held after a daemon restart until the user confirms it. Nothing
    /// continues on its own after a restart.
    PromptHeld {
        /// The turn of the held prompt.
        turn_id: TurnId,
    },

    /// A turn began running.
    TurnStarted {
        /// The turn.
        turn_id: TurnId,
        /// The user's working directory when the prompt was sent.
        cwd: PathBuf,
        /// The scope derived from that directory.
        scope: Scope,
        /// The settings that the turn runs with until it ends. Absent in turns recorded
        /// before turn settings, whose model and effort no event names.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        settings: Option<EffectiveSettings>,
    },

    /// The scope of the conversation changed between turns, because the user moved.
    ScopeChanged {
        /// The turn whose working directory produced the new scope.
        turn_id: TurnId,
        /// The scope before.
        from: Scope,
        /// The scope now.
        to: Scope,
    },

    /// An assistant message grew while the model streamed it. The event carries only
    /// the text added since the previous update, so a long answer costs the log its
    /// length once rather than once per update; `assistant_message_completed` carries
    /// the whole text. A reader that joined in the middle of a message (`offset` past
    /// what it holds) waits for the completed text.
    AssistantMessageUpdated {
        /// The turn.
        turn_id: TurnId,
        /// The position of the message among the turn's assistant messages, from 0.
        index: u32,
        /// The length in bytes of the message's text before `delta`.
        offset: u64,
        /// The text added at `offset`.
        delta: String,
    },

    /// An assistant message is complete.
    AssistantMessageCompleted {
        /// The turn.
        turn_id: TurnId,
        /// The position of the message among the turn's assistant messages, from 0.
        index: u32,
        /// The whole text.
        text: String,
    },

    /// The model asked for a tool call.
    ToolCallStarted {
        /// The turn.
        turn_id: TurnId,
        /// The call.
        call_id: CallId,
        /// The tool's registered name, such as `shell` or `read_file`.
        tool: String,
        /// The tool's input as the model wrote it.
        input: Value,
    },

    /// A running tool call produced more output. The daemon coalesces updates per call.
    ToolCallOutputUpdated {
        /// The turn.
        turn_id: TurnId,
        /// The call.
        call_id: CallId,
        /// The end of the output so far, bounded in size.
        tail: String,
        /// The size of the whole output so far, in bytes.
        bytes: u64,
    },

    /// A running tool call began or stopped waiting for the user to type a line. The
    /// daemon reports each change once. A client that can read keys asks the user while
    /// the call's latest `input` is not `none`, and sends the answer with
    /// `input.respond`; the call's `tool_call_completed` ends the wait as well.
    ToolCallInputChanged {
        /// The turn.
        turn_id: TurnId,
        /// The call.
        call_id: CallId,
        /// What the call waits for now.
        input: InputWait,
    },

    /// A tool call finished.
    ToolCallCompleted {
        /// The turn.
        turn_id: TurnId,
        /// The call.
        call_id: CallId,
        /// The output that the model sees, truncated in the middle when it is long.
        output: String,
        /// True when `output` was truncated; the full output stays in the recording.
        truncated: bool,
        /// True when the tool failed or the call was denied.
        is_error: bool,
        /// The exit code, for a tool that runs a command.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        exit_code: Option<i32>,
    },

    /// A tool call needs the user's approval before it runs.
    ApprovalRequested {
        /// The turn, which waits for the answer.
        turn_id: TurnId,
        /// The call that waits.
        call_id: CallId,
        /// What the call would do, in one line.
        summary: String,
        /// A diff of the change, for a call that writes a file.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        diff_preview: Option<String>,
    },

    /// The user answered an approval request.
    ApprovalResolved {
        /// The turn.
        turn_id: TurnId,
        /// The call.
        call_id: CallId,
        /// The answer.
        decision: ApprovalDecision,
        /// The surface that answered.
        origin: Origin,
    },

    /// An approval request can no longer be answered, because the daemon restarted while
    /// it was pending.
    ApprovalExpired {
        /// The turn.
        turn_id: TurnId,
        /// The call.
        call_id: CallId,
    },

    /// The user added guidance to the running turn.
    TurnSteered {
        /// The turn.
        turn_id: TurnId,
        /// The guidance.
        text: String,
    },

    /// The user asked to interrupt the turn. The turn ends with
    /// [`Event::TurnInterrupted`] once the model's stream actually stops.
    TurnInterruptRequested {
        /// The turn.
        turn_id: TurnId,
        /// The surface that asked.
        origin: Origin,
    },

    /// An interrupted turn stopped.
    TurnInterrupted {
        /// The turn.
        turn_id: TurnId,
    },

    /// A turn finished normally.
    TurnCompleted {
        /// The turn.
        turn_id: TurnId,
        /// The tokens that the turn used, when the provider reported them.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        usage: Option<Usage>,
    },

    /// A turn ended with an error, such as a provider failure.
    TurnFailed {
        /// The turn.
        turn_id: TurnId,
        /// What went wrong.
        error: ErrorBody,
    },

    /// A turn that was running when the daemon stopped was cancelled at the next start.
    TurnCancelled {
        /// The turn.
        turn_id: TurnId,
    },

    /// The conversation's hidden shell started.
    ShellStarted {
        /// The shell's PTY, which `pty.attach` takes.
        pty_id: PtyId,
        /// The directory the shell started in.
        cwd: PathBuf,
        /// The shell's process id.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pid: Option<u32>,
    },

    /// The conversation's hidden shell exited.
    ShellExited {
        /// The shell's PTY.
        pty_id: PtyId,
        /// The exit code; absent when a signal ended the shell.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        exit_code: Option<i32>,
    },

    /// The hidden shell reported a new working directory (OSC 7).
    CwdChanged {
        /// The shell's PTY.
        pty_id: PtyId,
        /// The new directory.
        cwd: PathBuf,
        /// The host in the report, when it named one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        host: Option<String>,
    },

    /// A provider login finished and its credentials are stored.
    LoginCompleted {
        /// The provider, such as `openai`.
        provider: String,
    },

    /// An event of a kind that this build does not know. It encodes back to the same JSON
    /// object it was decoded from. The schema leaves it out: it describes the known
    /// kinds, and says that readers must accept others.
    #[serde(untagged)]
    #[schemars(skip)]
    Unknown {
        /// The `kind` member, never one of the kinds above.
        #[serde(deserialize_with = "unknown_kind")]
        kind: String,
        /// Every other member. A `kind` key in it is not written.
        #[serde(flatten, serialize_with = "payload_without_kind")]
        payload: Map<String, Value>,
    },
}

impl Event {
    /// The wire name of the event's kind, such as `turn_started`. The event log stores
    /// it in its `kind` column.
    pub fn kind(&self) -> &str {
        match self {
            Event::Unknown { kind, .. } => kind,
            known => {
                let name: &'static str = EventKind::from(known).into();
                name
            }
        }
    }

    /// The turn that the event belongs to, when it belongs to one.
    pub fn turn_id(&self) -> Option<TurnId> {
        match self {
            Event::PromptQueued { turn_id, .. }
            | Event::PromptHeld { turn_id }
            | Event::TurnStarted { turn_id, .. }
            | Event::ScopeChanged { turn_id, .. }
            | Event::AssistantMessageUpdated { turn_id, .. }
            | Event::AssistantMessageCompleted { turn_id, .. }
            | Event::ToolCallStarted { turn_id, .. }
            | Event::ToolCallOutputUpdated { turn_id, .. }
            | Event::ToolCallInputChanged { turn_id, .. }
            | Event::ToolCallCompleted { turn_id, .. }
            | Event::ApprovalRequested { turn_id, .. }
            | Event::ApprovalResolved { turn_id, .. }
            | Event::ApprovalExpired { turn_id, .. }
            | Event::TurnSteered { turn_id, .. }
            | Event::TurnInterruptRequested { turn_id, .. }
            | Event::TurnInterrupted { turn_id }
            | Event::TurnCompleted { turn_id, .. }
            | Event::TurnFailed { turn_id, .. }
            | Event::TurnCancelled { turn_id } => Some(*turn_id),
            Event::ConversationCreated { .. }
            | Event::ShellStarted { .. }
            | Event::ShellExited { .. }
            | Event::CwdChanged { .. }
            | Event::LoginCompleted { .. }
            | Event::Unknown { .. } => None,
        }
    }
}

/// Writes the payload of [`Event::Unknown`]. A payload built by hand may hold its own
/// `kind`, and writing it would give the object two `kind` members.
fn payload_without_kind<S: Serializer>(
    payload: &Map<String, Value>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.collect_map(payload.iter().filter(|(key, _)| key.as_str() != "kind"))
}

/// Reads the `kind` of [`Event::Unknown`], refusing a kind that this build knows.
///
/// serde tries the untagged `Unknown` variant after every tagged variant has failed,
/// which also happens when a known kind has a malformed body. Refusing known kinds here
/// turns that case into an error instead of a silent `Unknown`. The price is serde's
/// generic message for it, because serde drops the errors of the variants it tried.
fn unknown_kind<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    let kind = String::deserialize(deserializer)?;
    match EventKind::from_str(&kind) {
        Ok(known) if known != EventKind::Unknown => Err(de::Error::custom(format!(
            "the body of the `{kind}` event does not match its kind"
        ))),
        _ => Ok(kind),
    }
}

/// An event with its place in the log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EventEnvelope {
    /// The event's global sequence number.
    pub seq: Seq,
    /// The conversation the event belongs to; absent for daemon-wide events such as
    /// [`Event::LoginCompleted`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<ConversationId>,
    /// When the daemon recorded the event.
    pub at: Timestamp,
    /// The event.
    pub event: Event,
}

/// The user's answer to an approval request.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ApprovalDecision {
    /// Run the tool call.
    Allow,
    /// Do not run it; the model sees a denied call.
    Deny,
}

/// Whether a running tool call's command waits for the user to type a line, and what
/// happens to that line.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum InputWait {
    /// The call does not wait for input now, so a client stops asking the user.
    #[default]
    None,
    /// The command waits for a line with echo on, such as a `[Y/n]` question. The user's
    /// answer is shown on the terminal and reaches the output that the model reads.
    Visible,
    /// The command reads a line with echo off, such as the password prompt of `sudo` or
    /// `ssh`. The user's answer is not shown and the terminal does not put it in the
    /// output; the model sees it only if the program that reads it prints it.
    Hidden,
}

/// Tokens that a turn used, as the provider reported them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct Usage {
    /// Tokens sent to the model.
    pub input_tokens: u64,
    /// Tokens the model produced.
    pub output_tokens: u64,
}

#[cfg(test)]
mod tests;

//! The `Method` enum: every request a client can make, and the scope each one needs.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    AdminConfigReload, AdminLoginOpenAi, AdminStatus, ApprovalRespond, CommandId,
    ConversationHistory, ConversationSubscribe, ConversationsList, Hello, InputRespond,
    LeaseReport, ModelsList, PromptSend, PtyAttach, PtyResize, PtyWrite, ScopeName, TurnInterrupt,
    TurnSteer,
};

/// A request: the wire method name and its params.
///
/// On the wire this is the `method` and `params` members of a request frame, such as
/// `{"id": 1, "method": "conversations.list", "params": {}}`. `params` is always an
/// object, even for a method that takes nothing.
///
/// Deliberately not `#[non_exhaustive]`, unlike every other wire enum: the daemon matches
/// it exhaustively, so a new method cannot ship without a scope and a handler. The
/// matches in this file are exhaustive for the same reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "method", content = "params")]
pub enum Method {
    /// `hello`: the first request on every connection.
    #[serde(rename = "hello")]
    Hello(Hello),
    /// `conversations.list`: page through the conversations.
    #[serde(rename = "conversations.list")]
    ConversationsList(ConversationsList),
    /// `conversation.subscribe`: follow a conversation's events (streaming).
    #[serde(rename = "conversation.subscribe")]
    ConversationSubscribe(ConversationSubscribe),
    /// `conversation.history`: page backwards through a conversation's events.
    #[serde(rename = "conversation.history")]
    ConversationHistory(ConversationHistory),
    /// `prompt.send`: send a prompt.
    #[serde(rename = "prompt.send")]
    PromptSend(PromptSend),
    /// `turn.interrupt`: stop the running turn.
    #[serde(rename = "turn.interrupt")]
    TurnInterrupt(TurnInterrupt),
    /// `turn.steer`: add guidance to the running turn.
    #[serde(rename = "turn.steer")]
    TurnSteer(TurnSteer),
    /// `approval.respond`: answer an approval request.
    #[serde(rename = "approval.respond")]
    ApprovalRespond(ApprovalRespond),
    /// `pty.attach`: watch a PTY (streaming).
    #[serde(rename = "pty.attach")]
    PtyAttach(PtyAttach),
    /// `pty.write`: type into a PTY.
    #[serde(rename = "pty.write")]
    PtyWrite(PtyWrite),
    /// `pty.resize`: change a PTY's size.
    #[serde(rename = "pty.resize")]
    PtyResize(PtyResize),
    /// `input.respond`: send the line that the user typed to a running tool call that
    /// waits for input.
    #[serde(rename = "input.respond")]
    InputRespond(InputRespond),
    /// `lease.report`: say what the client is watching.
    #[serde(rename = "lease.report")]
    LeaseReport(LeaseReport),
    /// `models.list`: the models that a prompt may name, with their efforts.
    #[serde(rename = "models.list")]
    ModelsList(ModelsList),
    /// `admin.status`: the daemon's health (Unix socket only).
    #[serde(rename = "admin.status")]
    AdminStatus(AdminStatus),
    /// `admin.config_reload`: read the config file again now (Unix socket only).
    #[serde(rename = "admin.config_reload")]
    AdminConfigReload(AdminConfigReload),
    /// `admin.login_openai`: log in to the OpenAI subscription (streaming, Unix socket
    /// only).
    #[serde(rename = "admin.login_openai")]
    AdminLoginOpenAi(AdminLoginOpenAi),
}

impl Method {
    /// The wire name, such as `conversation.subscribe`. The daemon's receipts and log
    /// spans use it.
    pub const fn name(&self) -> &'static str {
        match self {
            Method::Hello(_) => "hello",
            Method::ConversationsList(_) => "conversations.list",
            Method::ConversationSubscribe(_) => "conversation.subscribe",
            Method::ConversationHistory(_) => "conversation.history",
            Method::PromptSend(_) => "prompt.send",
            Method::TurnInterrupt(_) => "turn.interrupt",
            Method::TurnSteer(_) => "turn.steer",
            Method::ApprovalRespond(_) => "approval.respond",
            Method::PtyAttach(_) => "pty.attach",
            Method::PtyWrite(_) => "pty.write",
            Method::PtyResize(_) => "pty.resize",
            Method::InputRespond(_) => "input.respond",
            Method::LeaseReport(_) => "lease.report",
            Method::ModelsList(_) => "models.list",
            Method::AdminStatus(_) => "admin.status",
            Method::AdminConfigReload(_) => "admin.config_reload",
            Method::AdminLoginOpenAi(_) => "admin.login_openai",
        }
    }

    /// The command id of a write, which the daemon keeps a receipt for so that a retry
    /// returns the first result. `None` for reads and for PTY input and answers, which
    /// are not retried.
    pub const fn command_id(&self) -> Option<CommandId> {
        match self {
            Method::PromptSend(params) => Some(params.command_id),
            Method::TurnInterrupt(params) => Some(params.command_id),
            Method::TurnSteer(params) => Some(params.command_id),
            Method::ApprovalRespond(params) => Some(params.command_id),
            Method::Hello(_)
            | Method::ConversationsList(_)
            | Method::ConversationSubscribe(_)
            | Method::ConversationHistory(_)
            | Method::PtyAttach(_)
            | Method::PtyWrite(_)
            | Method::PtyResize(_)
            | Method::InputRespond(_)
            | Method::LeaseReport(_)
            | Method::ModelsList(_)
            | Method::AdminStatus(_)
            | Method::AdminConfigReload(_)
            | Method::AdminLoginOpenAi(_) => None,
        }
    }

    /// True for a streaming method, which may send many items. A unary method sends
    /// exactly one.
    pub const fn is_stream(&self) -> bool {
        match self {
            Method::ConversationSubscribe(_)
            | Method::PtyAttach(_)
            | Method::AdminLoginOpenAi(_) => true,
            Method::Hello(_)
            | Method::ConversationsList(_)
            | Method::ConversationHistory(_)
            | Method::PromptSend(_)
            | Method::TurnInterrupt(_)
            | Method::TurnSteer(_)
            | Method::ApprovalRespond(_)
            | Method::PtyWrite(_)
            | Method::PtyResize(_)
            | Method::InputRespond(_)
            | Method::LeaseReport(_)
            | Method::ModelsList(_)
            | Method::AdminStatus(_)
            | Method::AdminConfigReload(_) => false,
        }
    }
}

impl ScopeName {
    /// The scope that a connection needs to call `method`.
    ///
    /// `hello`, `lease.report` and `models.list` need only `read`, which every
    /// connection holds.
    pub const fn for_method(method: &Method) -> ScopeName {
        match method {
            Method::Hello(_)
            | Method::ConversationsList(_)
            | Method::ConversationSubscribe(_)
            | Method::ConversationHistory(_)
            | Method::LeaseReport(_)
            | Method::ModelsList(_) => ScopeName::Read,
            Method::PromptSend(_) | Method::TurnInterrupt(_) | Method::TurnSteer(_) => {
                ScopeName::Operate
            }
            Method::ApprovalRespond(_) => ScopeName::Approve,
            // An answer is typed into a PTY, so it needs the scope that `pty.write` needs.
            Method::PtyAttach(_)
            | Method::PtyWrite(_)
            | Method::PtyResize(_)
            | Method::InputRespond(_) => ScopeName::Terminal,
            Method::AdminStatus(_) | Method::AdminConfigReload(_) | Method::AdminLoginOpenAi(_) => {
                ScopeName::Admin
            }
        }
    }
}

#[cfg(test)]
mod tests;

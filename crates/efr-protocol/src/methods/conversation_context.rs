//! `conversation.context`: what fills the model's context of a conversation, and the
//! cache use of its model calls, for `efr context`.

use jiff::Timestamp;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{CompactionTrigger, ConversationId, TurnId, Usage};

/// The params of `conversation.context`, a read method.
///
/// efrd builds the history that the next request of the conversation would send, with
/// the system prompt and the tool definitions, and measures each part. It never sends
/// the text of a message: only sizes, counts and the labels that `efr history` shows
/// already (the command line of a shell call, the path of a file tool, the id of a
/// job). While a turn runs, the answer holds the turn's messages up to now, from the
/// log.
///
/// Refusals: `not_found` for a conversation that does not exist.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ConversationContext {
    /// The conversation.
    pub conversation_id: ConversationId,
    /// The model to measure for. Absent: the model of the newest turn, else the
    /// default model. The model decides the window and whether the provider's own
    /// items of earlier turns go back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

/// The result of `conversation.context`.
///
/// Every token count in `parts`, `largest` and `estimate` is an estimate: the bytes of
/// the request's JSON divided by 4. The provider's own count (`counted`) differs, mostly
/// for reasoning and images, and a client shows it first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ConversationContextResult {
    /// The model that the numbers are for.
    pub model: String,
    /// The model's context window, in tokens. When efrd does not know it, the default
    /// that efrd counts with.
    pub window: u64,
    /// True when the model list knows the window; false when `window` is the default.
    pub window_known: bool,
    /// The context size at which an auto compaction runs.
    pub trigger: u64,
    /// The largest request that efrd sends: 95% of the window.
    pub hard_cap: u64,
    /// Whether auto compaction is on (`compaction.auto`).
    pub auto: bool,
    /// The provider's last real count: the context at the end of the newest turn.
    /// Absent when no turn has one since the newest compaction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub counted: Option<CountedUse>,
    /// The estimated tokens of the next request now, without the next prompt and its
    /// preamble: the sum of `parts`.
    pub estimate: u64,
    /// True when a turn runs: the history holds its messages up to now.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub running: bool,
    /// What the next request holds, one entry per kind, in the order of
    /// [`ContextPartKind::ALL`]. A kind with nothing in it is absent.
    pub parts: Vec<ContextPart>,
    /// The largest single items of the history, largest first: at most
    /// [`MAX_LARGEST`] of them.
    pub largest: Vec<ContextItem>,
    /// The compactions of the conversation.
    pub compactions: CompactionSummary,
    /// The cache use of the conversation's model calls.
    pub cache: CacheSummary,
}

/// The most items in [`ConversationContextResult::largest`].
pub const MAX_LARGEST: usize = 5;

/// The provider's own count of the context at the end of a turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CountedUse {
    /// The tokens in the context at the last model call of the turn: its input plus its
    /// output.
    pub tokens: u64,
    /// The turn.
    pub turn_id: TurnId,
    /// When the turn ended.
    pub at: Timestamp,
}

/// A kind of content in the request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ContextPartKind {
    /// The system prompt, with the request's own fields.
    System,
    /// The tool definitions.
    Tools,
    /// The fresh context block and the summary of the newest compaction, with the notes
    /// on what the history leaves out.
    Summary,
    /// The prompts and steers of the user, and efr's notes at the end of a turn that did
    /// not complete.
    Prompts,
    /// The live-state preamble of each prompt. Only the newest is current; the older
    /// ones stay so that each request starts with the request before it.
    Preambles,
    /// The notices of background jobs.
    Notices,
    /// The model's answers: its text, its reasoning and its provider items.
    Answers,
    /// The model's tool calls with their input.
    ToolCalls,
    /// The output of the tool calls.
    ToolOutputs,
    /// The tool outputs that a compaction replaced with a short stub.
    Pruned,
    /// A kind that this build does not know.
    #[serde(other)]
    Unknown,
}

impl ContextPartKind {
    /// Every kind, in the order of the enum.
    pub const ALL: [ContextPartKind; 11] = [
        ContextPartKind::System,
        ContextPartKind::Tools,
        ContextPartKind::Summary,
        ContextPartKind::Prompts,
        ContextPartKind::Preambles,
        ContextPartKind::Notices,
        ContextPartKind::Answers,
        ContextPartKind::ToolCalls,
        ContextPartKind::ToolOutputs,
        ContextPartKind::Pruned,
        ContextPartKind::Unknown,
    ];
}

/// The estimated size of one kind of content in the request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ContextPart {
    /// The kind.
    pub kind: ContextPartKind,
    /// Its estimated tokens.
    pub tokens: u64,
    /// How many items it has: prompts, preambles, notices, answers, calls or outputs;
    /// the tools for `tools`, the messages for `summary`, 1 or 0 for `system`.
    pub count: u32,
}

/// One large item of the history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ContextItem {
    /// What it is.
    pub kind: ContextPartKind,
    /// Its estimated tokens.
    pub tokens: u64,
    /// The number of its turn in the conversation, counted from 1. Absent for the
    /// summary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<u32>,
    /// The tool, for a tool call or its output.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    /// What `efr history` shows for it already, never the text of a message: the first
    /// line of the command of a shell call, the path of a file tool, the files of a
    /// patch, the id of a job of a notice.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

/// The compactions of a conversation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CompactionSummary {
    /// How many compactions the conversation had.
    pub count: u32,
    /// The newest one. Absent when there is none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub newest: Option<CompactionBrief>,
}

/// The facts of one compaction, without its summary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CompactionBrief {
    /// When efrd recorded it.
    pub at: Timestamp,
    /// What started it.
    pub trigger: CompactionTrigger,
    /// The tokens in the context before it.
    pub tokens_before: u64,
    /// The estimated tokens in the context after it.
    pub tokens_after: u64,
    /// True when it wrote a summary; false when pruning alone freed enough room.
    pub summarized: bool,
    /// The tool outputs that it replaced with the stub. Absent when none.
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub pruned_outputs: u32,
}

/// The cache use of a conversation's model calls.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CacheSummary {
    /// The usage of the newest ended turn that reported one. Absent when none did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_turn: Option<Usage>,
    /// The sum over every turn and every compaction since the conversation started.
    /// Its `context_tokens` is absent: a context is no sum.
    pub conversation: Usage,
    /// The turns whose usage is in `conversation`.
    pub turns: u32,
}

fn is_zero_u32(value: &u32) -> bool {
    *value == 0
}

//! `conversations.search`: the conversations whose titles or text match some words, one
//! row per conversation, best first, for the picker of `,resume`, `efr search` and the
//! phone. The types of a hit, which `conversation.search` shares, live here too.

use std::path::PathBuf;

use jiff::Timestamp;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{ConversationId, PageCursor, RecentConversation, RouteLabel, Seq, TurnId};

/// The params of `conversations.search`, a read method for user-side peers: one row per
/// conversation.
///
/// efrd searches an index of the titles, the user's prompts, the model's answers and
/// the commands that the agent ran. The index holds no command output and no file
/// contents, and its text has the secrets that efr recognises replaced. efrd parses
/// `query` itself, so a client never sends search syntax: words split on white space,
/// a `"double quoted"` part is one phrase, every word must match in one prompt, answer
/// or command, and only the last word matches as a prefix. A query of more than 256
/// characters or 16 words is `invalid`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ConversationsSearch {
    /// The words to find. Empty after a trim: the recent conversations, in the order of
    /// `conversations.recent`.
    pub query: String,
    /// The terminal that asks, such as `/dev/pts/3`: its conversations show as `here`,
    /// and `current` names its current one. Absent on the phone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tty: Option<String>,
    /// The working directory that decides the current scope, for an empty query.
    /// Absent: no scope comes first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pwd: Option<PathBuf>,
    /// The most rows to return: 50 when absent, 200 at most.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// Also return the measures of the search in `stats`. False when absent.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stats: bool,
}

/// The result of `conversations.search`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ConversationsSearchResult {
    /// The scope of `pwd`, whose conversations come first, for an empty query only:
    /// the same as in `conversations.recent`. Absent for a query and without a `pwd`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<RouteLabel>,
    /// The current conversation of the terminal, as in `conversations.recent`. Absent
    /// without a `tty`, or when the terminal holds none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current: Option<ConversationId>,
    /// The rows, best first. For an empty query: the ones of the scope of `pwd` first,
    /// each part newest first.
    pub rows: Vec<SearchRow>,
    /// Why the search stopped before it saw every match. Absent: it saw every match.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub partial: Option<SearchPartial>,
    /// How far efrd has indexed the conversations. Absent: the index is complete.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<SearchIndexState>,
    /// The measures of the search, when the params asked for them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stats: Option<SearchStats>,
}

/// One conversation of the result of `conversations.search`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SearchRow {
    /// The same members as a row of `conversations.recent`, but `summary.title` always
    /// holds the title with its secrets replaced, the text that `title_ranges` point
    /// into. Absent title: the conversation has none yet.
    pub conversation: RecentConversation,
    /// The shortest start of the conversation id, at least 8 characters, that no other
    /// conversation of the store shares. `,resume` takes it.
    pub short_id: String,
    /// The parts of the title that match the query. Absent when none match.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub title_ranges: Vec<TextRange>,
    /// The best hit in the text of the conversation. Absent for an empty query and for
    /// a match in the title only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hit: Option<SearchHit>,
}

/// A place in a conversation that matches the query: one prompt, answer or command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SearchHit {
    /// What the text is. A steer is a prompt.
    pub kind: SearchKind,
    /// The sequence number of the event that holds the text.
    pub seq: Seq,
    /// The turn of that event. Absent for an event outside a turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<TurnId>,
    /// When the event happened.
    pub at: Timestamp,
    /// A `conversation.history` cursor whose page ends at this event, so a client can
    /// open the conversation at the hit.
    pub history_cursor: PageCursor,
    /// The text: one snippet line in `conversations.search`, whole lines in
    /// `conversation.search`. Each line is plain text with no control character but the
    /// tab.
    pub lines: Vec<SearchLine>,
}

/// One line of text with the parts that match the query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SearchLine {
    /// The text, on one line.
    pub text: String,
    /// The parts of `text` that match, sorted, with no overlap. Absent when none match.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ranges: Vec<TextRange>,
}

/// A half-open range `[start, end)` of Unicode code points in a text, counted from 0.
/// A client cuts a range that ends after its text to the text, and ignores an empty or
/// reversed one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct TextRange {
    /// The first code point of the range.
    pub start: u32,
    /// The code point after the last one of the range.
    pub end: u32,
}

/// What the text of a hit is. A reader takes a kind that it does not know as
/// `unknown`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum SearchKind {
    /// The title of the conversation.
    Title,
    /// A prompt of the user, or a steer.
    Prompt,
    /// An answer of the model.
    Answer,
    /// A command that the agent ran.
    Command,
    /// A kind that this build does not know.
    #[serde(other)]
    Unknown,
}

impl SearchKind {
    /// Every kind, in the order of the enum.
    pub const ALL: [SearchKind; 5] = [
        SearchKind::Title,
        SearchKind::Prompt,
        SearchKind::Answer,
        SearchKind::Command,
        SearchKind::Unknown,
    ];

    /// The wire form, such as `prompt`.
    pub const fn as_str(self) -> &'static str {
        match self {
            SearchKind::Title => "title",
            SearchKind::Prompt => "prompt",
            SearchKind::Answer => "answer",
            SearchKind::Command => "command",
            SearchKind::Unknown => "unknown",
        }
    }
}

/// Why a search stopped before it saw every match. It is no error: the rows are the
/// best of what it saw. A reader takes a reason that it does not know as `unknown`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum SearchPartial {
    /// The search read the most rows that one search may read, the newest first.
    RowCap,
    /// The search took the most time that one search may take.
    TimeCap,
    /// A reason that this build does not know.
    #[serde(other)]
    Unknown,
}

impl SearchPartial {
    /// Every reason, in the order of the enum.
    pub const ALL: [SearchPartial; 3] =
        [SearchPartial::RowCap, SearchPartial::TimeCap, SearchPartial::Unknown];

    /// The wire form, such as `time_cap`.
    pub const fn as_str(self) -> &'static str {
        match self {
            SearchPartial::RowCap => "row_cap",
            SearchPartial::TimeCap => "time_cap",
            SearchPartial::Unknown => "unknown",
        }
    }
}

/// How far efrd has indexed the conversations, while it indexes the old ones after an
/// upgrade or a change of what it replaces as secrets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SearchIndexState {
    /// The part of the event log that the index holds, from 0 to 100.
    pub percent: u8,
}

/// The measures of one search, for benchmarks on a real store.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SearchStats {
    /// The rows of the text index that the search read.
    pub rows_scanned: u32,
    /// The conversations that those rows belong to.
    pub conversations_scored: u32,
    /// The titles that the search compared with the query.
    pub titles_scored: u32,
    /// The time of the search in efrd, in microseconds.
    pub elapsed_us: u64,
}

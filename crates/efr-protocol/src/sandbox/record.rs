//! The classifier's input and output (phase A): the record of one exit, which holds
//! only user messages, the action and facts that efr collected itself, and the verdict.
//!
//! They are wire types because `exit_requested`, `exit_judged` and [`ExitInfo`] carry
//! them; `efr-conversation` keeps the builder and the judge.
//!
//! [`ExitInfo`]: crate::ExitInfo

use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{ExitKind, ExitSource, Grant, Scope};

/// Everything the classifier sees about one exit. Never in it: tool output, file
/// contents, assistant messages, the model's `needs.reason` or project files.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExitRecord {
    /// The record's version, [`ExitRecord::VERSION`].
    pub version: u32,
    /// Every prompt of the conversation, newest last, cut to 24 KiB with the oldest
    /// dropped first.
    pub user_messages: Vec<String>,
    /// The action that leaves the sandbox.
    pub action: ActionFacts,
    /// Facts that efr collected itself.
    pub facts: ExitFacts,
}

impl ExitRecord {
    /// The version of the record that this build writes.
    pub const VERSION: u32 = 1;
    /// The most bytes of user messages a record holds.
    pub const MAX_USER_MESSAGES_BYTES: usize = 24 * 1024;
}

/// The action of an exit record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ActionFacts {
    /// The tool, such as `shell`.
    pub tool: String,
    /// The exact line. The model wrote it, so it can hold attacker text: a comment or a
    /// string in it never authorizes anything.
    pub line: String,
    /// The working directory of the call.
    pub cwd: PathBuf,
    /// The turn's scope.
    pub scope: Scope,
    /// The exits of the call.
    pub exits: Vec<ExitKind>,
    /// Exactly what an "allow" opens.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub grants: Vec<Grant>,
    /// Where the exits came from.
    pub source: ExitSource,
    /// The payload of a `write_file`, `edit` or `apply_patch` call that edits a floor
    /// file, which efr copies to its backup store first: the new content, the old and
    /// the new strings, or the hunks of that file. At most
    /// [`MAX_EDIT_BYTES`](Self::MAX_EDIT_BYTES); the user decides a longer payload. The
    /// model wrote it, so it can hold attacker text, as `line` can.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edit: Option<String>,
}

impl ActionFacts {
    /// The most bytes of [`edit`](Self::edit) that the classifier judges.
    pub const MAX_EDIT_BYTES: usize = 8 * 1024;
}

/// Facts about an exit that efr collected itself, never from the model or a tool.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExitFacts {
    /// The paths the action writes or reads.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub targets: Vec<TargetFact>,
    /// The hosts the action reaches.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hosts: Vec<HostFact>,
    /// The programs of the line.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub programs: Vec<ProgramFact>,
    /// The upload patterns that the line matched, such as `git push`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub upload_patterns: Vec<String>,
    /// True when a git setting or another file that runs code changed in this turn.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub repo_surface_changed_this_turn: bool,
    /// For a destructive exit: the counts of the hardened `git status`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_status: Option<GitCounts>,
    /// Whether the turn's snapshot covers the targets (phase B).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot_covers: Option<bool>,
    /// The names, never the values, that sandboxed calls exported.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sandbox_export_names: Vec<String>,
    /// The earlier exits of this turn and how each was judged.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub previous_exits_this_turn: Vec<(ExitKind, Verdict)>,
    /// Floor refusals in a row without a person's answer.
    #[serde(default)]
    pub refusals_in_a_row: u8,
    /// Classifier denials in a row without an allow or a person's answer.
    #[serde(default)]
    pub denials_in_a_row: u8,
    /// Classifier denials in this turn.
    #[serde(default)]
    pub denials_this_turn: u8,
}

/// The engine's path classes, as the protocol names them (`efr-protocol` cannot name
/// `efr_permissions::PathClass`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum PathClassName {
    /// The conversation's `$SCRATCH`.
    Scratch,
    /// The user's configuration.
    UserConfig,
    /// The user's files.
    UserData,
    /// Everything outside the home directory.
    System,
    /// Keys, password stores and credentials.
    Secrets,
    /// A tool cache, such as `~/.cache` or `~/.cargo`, which the tool can fill again.
    Cache,
    /// Build output below a registered project, such as `target` or `node_modules`,
    /// which a build makes again.
    BuildOutput,
    /// efr's trash, `$XDG_DATA_HOME/efr/trash`.
    Trash,
}

/// A path that an exit writes or reads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TargetFact {
    /// The path, absolute.
    pub path: PathBuf,
    /// The engine's class of the path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub class: Option<PathClassName>,
    /// True when the path lies in a write root.
    pub in_write_root: bool,
    /// True when the path is a floor.
    pub floor: bool,
    /// True when the path lies in a synced folder.
    pub synced: bool,
    /// True when the path exists.
    pub exists: bool,
    /// True when a user message names the path: its `~` form, its absolute form or its
    /// last two components.
    pub named_in_user_messages: bool,
    /// True when efr copies the file to its backup store before the call writes it: a
    /// floor file that the user can copy back.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub backup_before_write: bool,
}

/// A host that an exit reaches.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HostFact {
    /// The host, in lower case.
    pub host: String,
    /// True when the proxy's allow list holds it (phase D).
    pub on_allow_list: bool,
    /// True when a user message names it.
    pub named_in_user_messages: bool,
    /// True when the proxy refused it in this conversation's last call (phase D).
    pub refused_by_proxy: bool,
}

/// A program word of the line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProgramFact {
    /// The word as the line has it.
    pub word: String,
    /// Where it resolves: the absolute path of a file, or `builtin`, a relative path,
    /// for a word that the shell runs itself, such as `cd` or `:`. Absent when it does
    /// not resolve.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved: Option<PathBuf>,
    /// True when the program lies in a write root.
    pub in_write_root: bool,
    /// True when the program changed in this turn.
    pub changed_this_turn: bool,
}

/// The counts of a hardened `git status`, never the names.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct GitCounts {
    /// Modified tracked files.
    pub modified: u32,
    /// Untracked files.
    pub untracked: u32,
    /// Staged changes.
    pub staged: u32,
}

/// The classifier's answer about one exit (phase A).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Judgement {
    /// Allow, deny or ask the user.
    pub verdict: Verdict,
    /// How risky the action is.
    pub risk: Risk,
    /// How clearly a user message authorizes it.
    pub user_authorization: UserAuthorization,
    /// The policy category, at most 40 characters.
    pub category: String,
    /// Why, at most 300 characters.
    pub rationale: String,
}

/// A verdict about an exit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Verdict {
    /// Run it.
    Allow,
    /// Do not run it.
    Deny,
    /// Let the user decide.
    AskUser,
}

/// The risk of an exit, as the classifier rates it.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Risk {
    /// Low.
    Low,
    /// Medium.
    Medium,
    /// High: allowed only when a user message names the action or its target.
    High,
    /// Critical: never allowed by the classifier.
    Critical,
}

/// How clearly the user's messages authorize an exit, as the classifier rates it.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum UserAuthorization {
    /// No user message relates to it.
    None,
    /// Low.
    Low,
    /// Medium.
    Medium,
    /// High.
    High,
}

/// Who judged an exit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum JudgeKind {
    /// The classifier (phase A).
    Classifier,
    /// The user.
    User,
    /// A floor, before any question.
    Floor,
    /// An always-allow rule (phase E).
    Always,
    /// Nobody answered a question while the user was away, so the call did not run.
    PresenceExpired,
}

/// The step of the classifier's chain that gave a verdict: its own model, the turn's
/// model, or the user when each model step failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ChainStep {
    /// The small model of the turn's provider, or `classifier.model`.
    ClassifierModel,
    /// The turn's model at a low effort.
    TurnModel,
    /// The user, after each model step failed.
    User,
}

/// Why the classifier stops for the rest of a turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum PauseReason {
    /// It denied 3 actions in a row.
    InARow,
    /// It denied 10 actions in the turn.
    PerTurn,
}

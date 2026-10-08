//! The efr wire contract: everything that the daemon and its clients exchange.
//!
//! Frames, the `Method` enum with one params type per method, results and stream items,
//! `Event` and its envelope, ids, `Scope`, `ShellContext`, turn settings, the `auto`
//! sandbox's launches, grants, exits and reports, the files a call or a turn changed,
//! screen snapshots, wire errors, the
//! pure length-prefix framing and [`PROTOCOL_VERSION`]. The daemon, `efr`, the tests,
//! the PTY proxy and the WebSocket clients compile against these types; the phone app
//! reads `docs/protocol.md` and the frozen fixtures in `fixtures/v1/`.
//!
//! Allowed dependencies: the allowlist permits `efr-stdx`, but this crate uses no
//! workspace crate, because `efr-stdx` depends on tokio and this crate must never reach
//! tokio. What does not belong here: IO of any kind, tokio, clocks and random generators,
//! and any knowledge of what the daemon does with a request.

// NOTE: missing_docs is set here and not in Cargo.toml, because Cargo rejects a
// `[lints]` table that both inherits the workspace lints and adds its own.
#![warn(missing_docs)]

mod capabilities;
mod changes;
mod error;
mod event;
#[cfg(test)]
mod fixtures_check;
mod frame;
pub mod framing;
mod ids;
mod method;
mod methods;
mod sandbox;
#[cfg(feature = "schema")]
pub mod schema;
mod scope;
mod screen;
mod secret_text;
mod settings;
mod shell_context;
mod version;

pub use capabilities::Capabilities;
pub use changes::{
    ChangeKind, FileChange, FileChanges, MAX_CALL_DIFF_LINES, MAX_LISTED_FILES, MAX_TURN_DIFF_LINES,
};
pub use error::{ErrorBody, ErrorCode, ErrorFrame, ProtocolError};
pub use event::{ApprovalDecision, Event, EventEnvelope, InputWait, Usage};
pub use frame::{ClientFrame, ServerFrame};
pub use ids::{
    CallId, CommandId, ConversationId, DaemonId, DeviceId, PtyId, QuestionId, RequestId, Seq,
    TurnId,
};
pub use method::Method;
pub use methods::admin_config_reload::{AdminConfigReload, AdminConfigReloadResult};
pub use methods::admin_login_openai::{AdminLoginOpenAi, AdminLoginOpenAiItem};
pub use methods::admin_project_add::{AdminProjectAdd, AdminProjectAddResult};
pub use methods::admin_project_remove::{AdminProjectRemove, AdminProjectRemoveResult};
pub use methods::admin_sandbox_check::{AdminSandboxCheck, AdminSandboxCheckResult};
pub use methods::admin_status::{
    AdminStatus, AdminStatusResult, ConfigStatus, DaemonRoots, ProviderStatus, RootDir, RootSource,
};
pub use methods::approval_respond::{ApprovalRespond, ApprovalRespondResult};
pub use methods::conversation_diff::{ConversationDiff, ConversationDiffResult};
pub use methods::conversation_history::{ConversationHistory, ConversationHistoryResult};
pub use methods::conversation_subscribe::{
    ConversationSnapshot, ConversationSubscribe, ConversationSubscribeItem, Draft, DraftPart,
};
pub use methods::conversations_list::{
    ConversationStatus, ConversationSummary, ConversationsList, ConversationsListResult,
};
pub use methods::hello::{DaemonPaths, Hello, HelloResult};
pub use methods::input_respond::{InputRespond, InputRespondResult};
pub use methods::lease_report::{LeaseReport, LeaseReportResult};
pub use methods::models_list::{
    EFFORT_MAX_LEN, ModelInfo, ModelSource, ModelsList, ModelsListResult, is_effort_word,
};
pub use methods::projects_list::{ProjectInfo, ProjectsList, ProjectsListResult};
pub use methods::prompt_send::{PromptSend, PromptSendResult};
pub use methods::prompt_withdraw::{
    PromptWithdraw, PromptWithdrawResult, WithdrawTarget, WithdrawnPrompt,
};
pub use methods::pty_attach::{PtyAttach, PtyAttachItem};
pub use methods::pty_resize::{PtyResize, PtyResizeResult};
pub use methods::pty_write::{PtyWrite, PtyWriteResult};
pub use methods::sandbox_explain::{SandboxExplain, SandboxExplainResult, SandboxPathRole};
pub use methods::sandbox_surface_respond::{SandboxSurfaceRespond, SandboxSurfaceRespondResult};
pub use methods::turn_interrupt::{
    ResentSteers, TurnInterrupt, TurnInterruptResult, WithdrawnSteer,
};
pub use methods::turn_steer::{LateSteer, TurnSteer, TurnSteerResult};
pub use methods::{Base64Bytes, ConfigFileError, PageCursor};
pub use sandbox::exit::{ExitInfo, ExitKind, ExitSource, Needs};
pub use sandbox::record::{
    ActionFacts, ExitFacts, ExitRecord, GitCounts, HostFact, JudgeKind, Judgement, PathClassName,
    ProgramFact, Risk, TargetFact, UserAuthorization, Verdict,
};
pub use sandbox::status::{CheckOutcome, SandboxCheck, SandboxPaths, SandboxStatus};
pub use sandbox::summary::{BlockReason, Blocked, ReportedFile, SandboxSummary, SurfaceChange};
pub use sandbox::{BusKind, CacheMode, Grant, Launch, ModeFallback, NetworkMode};
pub use scope::{Origin, ProjectId, Scope, ScopeName};
pub use screen::{Cell, Color, Cursor, RowCells, ScreenSnapshot, Size};
pub use secret_text::SecretText;
pub use settings::{EffectiveSettings, Mode, OverriddenSettings, TurnSettings};
pub use shell_context::ShellContext;
pub use version::PROTOCOL_VERSION;

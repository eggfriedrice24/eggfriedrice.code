//! The efr wire contract: everything that the daemon and its clients exchange.
//!
//! Frames, the `Method` enum with one params type per method, results and stream items,
//! `Event` and its envelope, ids, `Scope`, `ShellContext`, turn settings, the `auto`
//! sandbox's launches, grants, exits and reports, the presence of a person at a
//! conversation, the background jobs and their notices, the files a call or a turn
//! changed and the undo of a turn, the
//! context's use and its compactions, the search of conversations, the enrolled
//! devices with their pairing and the message that a device signs to prove its key,
//! the steps of the provider's web search and the pages that an answer cites, the
//! limits of an account and why an account could not serve a turn, the model's
//! question to the user and its answer,
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

mod accounts;
mod attachment;
mod capabilities;
mod changes;
mod compaction;
mod device;
mod error;
mod event;
#[cfg(test)]
mod fixtures_check;
mod frame;
pub mod framing;
mod ids;
mod jobs;
mod method;
mod methods;
mod presence;
mod question;
mod route;
mod sandbox;
#[cfg(feature = "schema")]
pub mod schema;
mod scope;
mod screen;
mod secret_prompt;
mod secret_text;
mod settings;
mod shell_context;
mod undo;
mod version;
mod web;

pub use accounts::{
    AccountLimits, AccountOut, AccountState, LimitKind, LimitReached, LimitWindow, OtherAccount,
    OthersFull,
};
pub use attachment::{
    Attachment, AttachmentId, AttachmentKind, AttachmentLimits, AttachmentOrigin, CHUNK_BYTES,
    KEPT_EXTENSION_BYTES, MAX_OPEN_UPLOADS, NAME_MAX_BYTES, SentForm, UPLOAD_TTL, UploadId,
    UploadInfo, WithheldReason, fit_name,
};
pub use capabilities::Capabilities;
pub use changes::{
    ChangeKind, FileChange, FileChanges, MAX_CALL_DIFF_LINES, MAX_LISTED_FILES, MAX_TURN_DIFF_LINES,
};
pub use compaction::{Compaction, CompactionTrigger, ContextUse};
pub use device::{DeviceInfo, PROVE_DOMAIN, PairEnd, PairingQr, ProveContext, ProveMessage};
pub use error::{ErrorBody, ErrorCode, ErrorFrame, ProtocolError};
pub use event::{
    AlwaysSaved, ApprovalDecision, ApprovedBy, Event, EventEnvelope, InputWait, NotSavedReason,
    ShellCommandEnd, StopReason, TurnStop, Usage,
};
pub use frame::{ClientFrame, ServerFrame};
pub use ids::{
    CallId, CommandId, CompactionId, ConversationId, DaemonId, DeviceId, PairingId, PtyId,
    QuestionId, RequestId, Seq, TurnId, UndoId,
};
pub use jobs::{
    BATCH_HEAD, BATCH_TAIL, ForwardOutcome, HeldReason, JobCgroup, JobId, JobInfo, JobState,
    JobStopBy, MovedCall, NoticeKind, PortForward, WAKE_QUESTION, Watch,
};
pub use method::Method;
pub use methods::admin_config_reload::{AdminConfigReload, AdminConfigReloadResult};
pub use methods::admin_login_api_key::{AdminLoginApiKey, AdminLoginApiKeyResult};
pub use methods::admin_login_openai::{AdminLoginOpenAi, AdminLoginOpenAiItem};
pub use methods::admin_logout::{AdminLogout, AdminLogoutResult};
pub use methods::admin_project_add::{AdminProjectAdd, AdminProjectAddResult};
pub use methods::admin_project_remove::{AdminProjectRemove, AdminProjectRemoveResult};
pub use methods::admin_push_test::{
    AdminPushTest, AdminPushTestResult, PUSH_DEVICE_NOT_REGISTERED, PUSH_INVALID_CREDENTIALS,
    PUSH_RATE_LIMITED, PUSH_TOO_MANY_EXPERIENCE_IDS, PUSH_UNAUTHORIZED, PushTestResult,
    PushTestRow,
};
pub use methods::admin_sandbox_check::{AdminSandboxCheck, AdminSandboxCheckResult};
pub use methods::admin_status::{
    AdminStatus, AdminStatusResult, ConfigStatus, DaemonRoots, FullModeStatus, HeldConfig,
    LoginKind, ProviderStatus, RootDir, RootSource,
};
pub use methods::approval_respond::{ApprovalRespond, ApprovalRespondResult};
pub use methods::attachment_discard::{AttachmentDiscard, AttachmentDiscardResult};
pub use methods::attachment_put::{AttachmentPut, AttachmentPutResult, SecretFile};
pub use methods::call_background::{CallBackground, CallBackgroundResult};
pub use methods::conversation_compact::{ConversationCompact, ConversationCompactResult};
pub use methods::conversation_context::{
    CacheSummary, CompactionBrief, CompactionSummary, ContextItem, ContextPart, ContextPartKind,
    ConversationContext, ConversationContextResult, CountedUse, MAX_LARGEST,
};
pub use methods::conversation_delete::{ConversationDelete, ConversationDeleteResult};
pub use methods::conversation_diff::{ConversationDiff, ConversationDiffResult};
pub use methods::conversation_history::{ConversationHistory, ConversationHistoryResult};
pub use methods::conversation_resume::{ConversationResume, ConversationResumeResult};
pub use methods::conversation_search::{ConversationSearch, ConversationSearchResult};
pub use methods::conversation_subscribe::{
    ConversationSnapshot, ConversationSubscribe, ConversationSubscribeItem, Draft, DraftPart,
    LiveTurn,
};
pub use methods::conversation_switch_preview::{
    ConversationSwitchPreview, ConversationSwitchPreviewResult, SwitchCost,
};
pub use methods::conversations_list::{
    Attention, ConversationStatus, ConversationSummary, ConversationsList, ConversationsListResult,
};
pub use methods::conversations_recent::{
    ConversationsRecent, ConversationsRecentResult, RecentConversation,
};
pub use methods::conversations_search::{
    ConversationsSearch, ConversationsSearchResult, SearchHit, SearchIndexState, SearchKind,
    SearchLine, SearchPartial, SearchRow, SearchStats, TextRange,
};
pub use methods::conversations_watch::{ConversationsWatch, ConversationsWatchItem};
pub use methods::device_enroll::{DeviceEnroll, DeviceEnrollResult};
pub use methods::device_list::{DeviceList, DeviceListResult};
pub use methods::device_pair_confirm::{DevicePairConfirm, DevicePairConfirmResult};
pub use methods::device_pair_start::{DevicePairStart, DevicePairStartItem};
pub use methods::device_prove::{DeviceProve, DeviceProveResult};
pub use methods::device_push_set::{
    DevicePushSet, DevicePushSetResult, EXPO_TOKEN_MAX_INSIDE, is_expo_push_token,
};
pub use methods::device_revoke::{DeviceRevoke, DeviceRevokeResult};
pub use methods::hello::{DaemonPaths, Hello, HelloResult};
pub use methods::input_respond::{InputRespond, InputRespondResult};
pub use methods::jobs_list::{JobsList, JobsListResult};
pub use methods::jobs_stop::{JobsStop, JobsStopResult};
pub use methods::lease_report::{LeaseReport, LeaseReportResult};
pub use methods::models_list::{
    CatalogOrigin, CatalogStatus, EFFORT_MAX_LEN, ModelInfo, ModelSource, ModelsList,
    ModelsListResult, group_by_kind, is_effort_word, order_by_list, provider_id_named,
    provider_kind, provider_named,
};
pub use methods::presence_set::{PresenceSet, PresenceSetResult};
pub use methods::projects_list::{ProjectInfo, ProjectsList, ProjectsListResult};
pub use methods::prompt_send::{MissingUploads, PromptSend, PromptSendResult, RecentCommand};
pub use methods::prompt_withdraw::{
    PromptWithdraw, PromptWithdrawResult, WithdrawTarget, WithdrawnPrompt,
};
pub use methods::provider_warm::{ProviderWarm, ProviderWarmResult};
pub use methods::pty_attach::{PtyAttach, PtyAttachItem};
pub use methods::pty_resize::{PtyResize, PtyResizeResult};
pub use methods::pty_write::{PtyWrite, PtyWriteResult};
pub use methods::question_respond::{QuestionRespond, QuestionRespondResult};
pub use methods::sandbox_explain::{SandboxExplain, SandboxExplainResult, SandboxPathRole};
pub use methods::sandbox_surface_respond::{SandboxSurfaceRespond, SandboxSurfaceRespondResult};
pub use methods::shell_interrupt::{ShellInterrupt, ShellInterruptResult};
pub use methods::shell_status::{BusyShell, ShellStatus, ShellStatusResult};
pub use methods::turn_interrupt::{
    ResentSteers, TurnInterrupt, TurnInterruptResult, WithdrawnSteer,
};
pub use methods::turn_steer::{LateSteer, TurnSteer, TurnSteerResult};
pub use methods::turn_undo::{TurnUndo, TurnUndoResult};
pub use methods::{Base64Bytes, ConfigFileError, PageCursor};
pub use presence::{Presence, PresenceSource};
pub use question::{
    ASK_USER, MAX_QUESTION_CHARS, MAX_QUESTION_OPTIONS, MIN_QUESTION_OPTIONS, QuestionOption,
    QuestionReply, UserAnswer,
};
pub use route::{AttachHow, NewReason, RepoHint, RouteLabel, RouteScope, Routed, Switch};
pub use sandbox::exit::{ExitInfo, ExitKind, ExitSource, Needs};
pub use sandbox::record::{
    ActionFacts, ChainStep, ExitFacts, ExitRecord, GitCounts, HostFact, JudgeKind, Judgement,
    PathClassName, PauseReason, ProgramFact, Risk, TargetFact, UserAuthorization, Verdict,
};
pub use sandbox::status::{BusProxyOff, CheckOutcome, SandboxCheck, SandboxPaths, SandboxStatus};
pub use sandbox::summary::{
    BlockReason, Blocked, BusRefusal, ReportedFile, SandboxSummary, SurfaceChange,
};
pub use sandbox::{
    BusKind, CacheMode, DesktopKind, EFR_KEY_NAMES, Grant, Launch, ModeFallback, NetworkMode,
};
pub use scope::{Origin, ProjectId, Scope, ScopeName};
pub use screen::{Cell, Color, Cursor, RowCells, ScreenSnapshot, Size};
pub use secret_prompt::looks_secret;
pub use secret_text::SecretText;
pub use settings::{
    EffectiveSettings, FullRefusal, Mode, OverriddenSettings, SwitchedModel, TurnSettings,
};
pub use shell_context::{DesktopEnv, ShellContext};
pub use undo::{
    MAX_NOT_COVERED, MAX_UNDO_ENTRIES, RefChange, RootFiles, SnapshotPhase, TurnUndoInfo,
    UndoAction, UndoEntry, UndoReason, UndoRefused, UndoableTurn,
};
pub use version::PROTOCOL_VERSION;
pub use web::{
    MAX_WEB_RESULTS, MAX_WEB_SOURCES, MAX_WEB_TEXT_CHARS, MAX_WEB_TITLE_CHARS, MAX_WEB_URL_CHARS,
    StoredCopyReason, WebAction, WebError, WebResult, WebSource,
};

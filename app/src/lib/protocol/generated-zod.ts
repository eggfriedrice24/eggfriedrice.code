// Generated from docs/protocol.schema.json by `pnpm gen:protocol` (scripts/gen-protocol.mjs).
// Do not edit it by hand. Change efr-protocol, run `cargo xtask protocol-schema` at the repo
// top, then run `pnpm gen:protocol` in app/.
//
// One schema for each definition, after the definitions that it names. Each schema
// satisfies its type in generated.ts, so a drift between the two files is a type error.
// An optional member uses exactOptional: JSON never carries undefined. A u64 is a safe
// integer (z.int()), never a bigint. A wire object strips the members that it does not
// list, so a member from a newer efrd passes.
//
// Use the schemas through the decoder (src/lib/protocol/envelope.ts). It finds an event,
// draft or item kind that this file does not list before a schema reads it, and gives an
// "other" wrapper for it instead of an error.

import { z } from "zod";

import type {
  ActionFacts,
  AdminConfigReload,
  AdminConfigReloadResult,
  AdminLoginApiKey,
  AdminLoginApiKeyResult,
  AdminLoginOpenAi,
  AdminLoginOpenAiItem,
  AdminLogout,
  AdminLogoutResult,
  AdminProjectAdd,
  AdminProjectAddResult,
  AdminProjectRemove,
  AdminProjectRemoveResult,
  AdminSandboxCheck,
  AdminSandboxCheckResult,
  AdminStatus,
  AdminStatusResult,
  AlwaysSaved,
  ApprovalDecision,
  ApprovalRespond,
  ApprovalRespondResult,
  ApprovedBy,
  AttachHow,
  Base64Bytes,
  BlockReason,
  Blocked,
  BusKind,
  BusProxyOff,
  BusRefusal,
  BusyShell,
  CacheMode,
  CacheSummary,
  CallBackground,
  CallBackgroundResult,
  CallId,
  Capabilities,
  CatalogOrigin,
  CatalogStatus,
  Cell,
  ChainStep,
  ChangeKind,
  CheckOutcome,
  ClientCancel,
  ClientFrame,
  ClientRequest,
  Color,
  CommandId,
  Compaction,
  CompactionBrief,
  CompactionId,
  CompactionSummary,
  CompactionTrigger,
  ConfigFileError,
  ConfigStatus,
  ContextItem,
  ContextPart,
  ContextPartKind,
  ContextUse,
  ConversationCompact,
  ConversationCompactResult,
  ConversationContext,
  ConversationContextResult,
  ConversationDelete,
  ConversationDeleteResult,
  ConversationDiff,
  ConversationDiffResult,
  ConversationHistory,
  ConversationHistoryResult,
  ConversationId,
  ConversationResume,
  ConversationResumeResult,
  ConversationSearch,
  ConversationSearchResult,
  ConversationSnapshot,
  ConversationStatus,
  ConversationSubscribe,
  ConversationSubscribeItem,
  ConversationSummary,
  ConversationsList,
  ConversationsListResult,
  ConversationsRecent,
  ConversationsRecentResult,
  ConversationsSearch,
  ConversationsSearchResult,
  CountedUse,
  Cursor,
  DaemonId,
  DaemonPaths,
  DaemonRoots,
  DesktopEnv,
  DesktopKind,
  DeviceEnroll,
  DeviceEnrollResult,
  DeviceId,
  DeviceInfo,
  DeviceList,
  DeviceListResult,
  DevicePairConfirm,
  DevicePairConfirmResult,
  DevicePairStart,
  DevicePairStartItem,
  DeviceProve,
  DeviceProveResult,
  DeviceRevoke,
  DeviceRevokeResult,
  Draft,
  DraftPart,
  EffectiveSettings,
  ErrorBody,
  ErrorCode,
  ErrorFrame,
  Event,
  EventEnvelope,
  ExitFacts,
  ExitInfo,
  ExitKind,
  ExitRecord,
  ExitSource,
  FileChange,
  FileChanges,
  ForwardOutcome,
  FullModeStatus,
  FullRefusal,
  GitCodeFact,
  GitCodeFacts,
  GitCounts,
  Grant,
  HeldConfig,
  HeldReason,
  Hello,
  HelloResult,
  HostFact,
  InputRespond,
  InputRespondResult,
  InputWait,
  JobCgroup,
  JobId,
  JobInfo,
  JobState,
  JobStopBy,
  JobsList,
  JobsListResult,
  JobsStop,
  JobsStopResult,
  JudgeKind,
  Judgement,
  LateSteer,
  Launch,
  LeaseReport,
  LeaseReportResult,
  LiveTurn,
  LoginKind,
  Method,
  MethodName,
  MethodParams,
  MethodResults,
  Mode,
  ModeFallback,
  ModelInfo,
  ModelSource,
  ModelsList,
  ModelsListResult,
  MovedCall,
  NetworkMode,
  NewReason,
  NotSavedReason,
  NoticeKind,
  Origin,
  OverriddenSettings,
  PageCursor,
  PairEnd,
  PairingId,
  PairingQr,
  PathClassName,
  PauseReason,
  PortForward,
  Presence,
  PresenceSet,
  PresenceSetResult,
  PresenceSource,
  ProgramFact,
  ProjectId,
  ProjectInfo,
  ProjectsList,
  ProjectsListResult,
  PromptSend,
  PromptSendResult,
  PromptWithdraw,
  PromptWithdrawResult,
  ProtocolVersion,
  ProveContext,
  ProviderStatus,
  PtyAttach,
  PtyAttachItem,
  PtyId,
  PtyResize,
  PtyResizeResult,
  PtyWrite,
  PtyWriteResult,
  QuestionId,
  RecentCommand,
  RecentConversation,
  RefChange,
  RepoHint,
  ReportedFile,
  RequestId,
  ResentSteers,
  Risk,
  RootDir,
  RootFiles,
  RootSource,
  RouteLabel,
  RouteScope,
  Routed,
  RowCells,
  SandboxCheck,
  SandboxExplain,
  SandboxExplainResult,
  SandboxPathRole,
  SandboxPaths,
  SandboxStatus,
  SandboxSummary,
  SandboxSurfaceRespond,
  SandboxSurfaceRespondResult,
  Scope,
  ScopeName,
  ScreenSnapshot,
  SearchHit,
  SearchIndexState,
  SearchKind,
  SearchLine,
  SearchPartial,
  SearchRow,
  SearchStats,
  SecretText,
  Seq,
  ServerAckFrame,
  ServerEndFrame,
  ServerFrame,
  ServerItemFrame,
  ShellCommandEnd,
  ShellContext,
  ShellInterrupt,
  ShellInterruptResult,
  ShellStatus,
  ShellStatusResult,
  Size,
  SnapshotPhase,
  StopReason,
  StreamMethodName,
  SurfaceChange,
  Switch,
  TargetFact,
  TextRange,
  TurnId,
  TurnInterrupt,
  TurnInterruptResult,
  TurnSettings,
  TurnSteer,
  TurnSteerResult,
  TurnStop,
  TurnUndo,
  TurnUndoInfo,
  TurnUndoResult,
  UndoAction,
  UndoEntry,
  UndoId,
  UndoReason,
  UndoableTurn,
  Usage,
  UserAuthorization,
  Verdict,
  Watch,
  WithdrawTarget,
  WithdrawnPrompt,
  WithdrawnSteer,
} from "./generated";

export const exitKindSchema = z.enum([
  "write",
  "host",
  "host_view",
  "socket",
  "desktop_ipc",
  "bus",
  "device",
  "masked_read",
  "destructive",
  "outside",
  "privilege",
  "persistence",
  "upload",
  "synced_write",
  "above_root",
  "secret",
  "config",
]) satisfies z.ZodType<ExitKind>;

export const exitSourceSchema = z.enum([
  "predicted",
  "needs",
  "cache_delete",
]) satisfies z.ZodType<ExitSource>;

export const busKindSchema = z.enum(["system", "session"]) satisfies z.ZodType<BusKind>;

export const desktopKindSchema = z.enum([
  "wayland",
  "x11",
  "hyprland",
  "sway",
  "i3",
  "niri",
]) satisfies z.ZodType<DesktopKind>;

export const grantSchema = z.discriminatedUnion("kind", [
  z.object({ kind: z.literal("write"), path: z.string() }),
  z.object({ host: z.string(), kind: z.literal("host"), port: z.int().min(0).max(65535) }),
  z.object({ kind: z.literal("open_network") }),
  z.object({ kind: z.literal("socket"), path: z.string() }),
  z.object({ bus: busKindSchema, kind: z.literal("bus") }),
  z.object({ desktop: desktopKindSchema, kind: z.literal("desktop") }),
  z.object({ kind: z.literal("device"), path: z.string() }),
  z.object({ kind: z.literal("unmask"), path: z.string() }),
  z.object({ kind: z.literal("net_view") }),
  z.object({ bus: busKindSchema, kind: z.literal("bus_filter") }),
  z.object({ kind: z.literal("unknown") }),
]) satisfies z.ZodType<Grant>;

export const projectIdSchema = z.guid() satisfies z.ZodType<ProjectId>;

export const scopeSchema = z.discriminatedUnion("kind", [
  z.object({ kind: z.literal("machine") }),
  z.object({ kind: z.literal("path"), value: z.string() }),
  z.object({ kind: z.literal("project"), value: projectIdSchema }),
]) satisfies z.ZodType<Scope>;

export const actionFactsSchema = z.object({
  background: z.boolean().exactOptional(),
  cwd: z.string(),
  edit: z.string().nullable().exactOptional(),
  exits: z.array(exitKindSchema),
  grants: z.array(grantSchema).exactOptional(),
  line: z.string(),
  max_minutes: z.uint32().nullable().exactOptional(),
  scope: scopeSchema,
  source: exitSourceSchema,
  tool: z.string(),
  wake_turn: z.boolean().exactOptional(),
  watch: z.boolean().exactOptional(),
}) satisfies z.ZodType<ActionFacts>;

export const adminConfigReloadSchema = z.object({}) satisfies z.ZodType<AdminConfigReload>;

export const configFileErrorSchema = z.object({
  column: z.uint32().nullable().exactOptional(),
  key: z.string().nullable().exactOptional(),
  line: z.uint32().nullable().exactOptional(),
  message: z.string(),
}) satisfies z.ZodType<ConfigFileError>;

export const adminConfigReloadResultSchema = z.object({
  applied: z.boolean(),
  error: configFileErrorSchema.nullable().exactOptional(),
  restart_needed: z.array(z.string()).exactOptional(),
  warnings: z.array(z.string()).exactOptional(),
}) satisfies z.ZodType<AdminConfigReloadResult>;

export const secretTextSchema = z.string() satisfies z.ZodType<SecretText>;

export const adminLoginApiKeySchema = z.object({
  check: z.boolean().exactOptional(),
  key: secretTextSchema,
  provider: z.string(),
}) satisfies z.ZodType<AdminLoginApiKey>;

export const adminLoginApiKeyResultSchema = z.object({
  active: z.boolean(),
  checked: z.boolean(),
  key_hint: z.string(),
  provider: z.string(),
}) satisfies z.ZodType<AdminLoginApiKeyResult>;

export const adminLoginOpenAiSchema = z.object({}) satisfies z.ZodType<AdminLoginOpenAi>;

export const adminLoginOpenAiItemSchema = z.discriminatedUnion("kind", [
  z.object({ kind: z.literal("authorize_url"), url: z.string() }),
  z.object({ kind: z.literal("completed"), provider: z.string() }),
]) satisfies z.ZodType<AdminLoginOpenAiItem>;

export const adminLogoutSchema = z.object({
  provider: z.string(),
}) satisfies z.ZodType<AdminLogout>;

export const adminLogoutResultSchema = z.object({
  active: z.boolean().exactOptional(),
  logged_out: z.boolean(),
  provider: z.string(),
}) satisfies z.ZodType<AdminLogoutResult>;

export const adminProjectAddSchema = z.object({
  git_root: z.boolean().exactOptional(),
  name: z.string().nullable().exactOptional(),
  path: z.string(),
}) satisfies z.ZodType<AdminProjectAdd>;

export const projectInfoSchema = z.object({
  id: projectIdSchema,
  name: z.string().nullable().exactOptional(),
  root: z.string(),
}) satisfies z.ZodType<ProjectInfo>;

export const adminProjectAddResultSchema = z.object({
  file: z.string(),
  project: projectInfoSchema,
  reload: adminConfigReloadResultSchema,
}) satisfies z.ZodType<AdminProjectAddResult>;

export const adminProjectRemoveSchema = z.object({
  path: z.string(),
}) satisfies z.ZodType<AdminProjectRemove>;

export const adminProjectRemoveResultSchema = z.object({
  file: z.string(),
  project: projectInfoSchema,
  reload: adminConfigReloadResultSchema,
}) satisfies z.ZodType<AdminProjectRemoveResult>;

export const adminSandboxCheckSchema = z.object({}) satisfies z.ZodType<AdminSandboxCheck>;

export const checkOutcomeSchema = z.enum([
  "ok",
  "warn",
  "fail",
  "skipped",
]) satisfies z.ZodType<CheckOutcome>;

export const sandboxCheckSchema = z.object({
  detail: z.string().nullable().exactOptional(),
  fix: z.string().nullable().exactOptional(),
  name: z.string(),
  outcome: checkOutcomeSchema,
}) satisfies z.ZodType<SandboxCheck>;

export const busProxyOffSchema = z.enum([
  "config",
  "no_program",
  "self_test",
  "old_version",
  "unknown",
]) satisfies z.ZodType<BusProxyOff>;

export const cacheModeSchema = z.enum([
  "overlay",
  "tmp",
  "readonly",
]) satisfies z.ZodType<CacheMode>;

export const networkModeSchema = z.enum(["none", "proxy"]) satisfies z.ZodType<NetworkMode>;

export const sandboxStatusSchema = z.object({
  available: z.boolean(),
  bus_proxy: z.boolean().exactOptional(),
  bus_proxy_off: busProxyOffSchema.nullable().exactOptional(),
  bwrap: z.string().nullable().exactOptional(),
  bwrap_version: z.string().nullable().exactOptional(),
  cache_mode: cacheModeSchema,
  errata: z.uint32().nullable().exactOptional(),
  fix: z.string().nullable().exactOptional(),
  landlock_abi: z.uint32().nullable().exactOptional(),
  netview: z.boolean().exactOptional(),
  netview_off: z.boolean().exactOptional(),
  network_mode: networkModeSchema,
  proxy: z.boolean().exactOptional(),
  reason: z.string().nullable().exactOptional(),
  warnings: z.array(z.string()).exactOptional(),
}) satisfies z.ZodType<SandboxStatus>;

export const adminSandboxCheckResultSchema = z.object({
  checks: z.array(sandboxCheckSchema),
  launch_us: z.int().nonnegative().nullable().exactOptional(),
  snapshot_launch_us: z.int().nonnegative().nullable().exactOptional(),
  status: sandboxStatusSchema,
}) satisfies z.ZodType<AdminSandboxCheckResult>;

export const adminStatusSchema = z.object({}) satisfies z.ZodType<AdminStatus>;

export const catalogOriginSchema = z.enum([
  "backend",
  "cache",
  "builtin",
  "missing",
]) satisfies z.ZodType<CatalogOrigin>;

export const catalogStatusSchema = z.object({
  fetched_at: z.iso.datetime({ offset: true }).nullable().exactOptional(),
  origin: catalogOriginSchema,
  provider: z.string().nullable().exactOptional(),
}) satisfies z.ZodType<CatalogStatus>;

export const heldConfigSchema = z.object({
  loosens: z.array(z.string()),
}) satisfies z.ZodType<HeldConfig>;

export const configStatusSchema = z.object({
  exists: z.boolean(),
  held: heldConfigSchema.nullable().exactOptional(),
  path: z.string(),
  reload_error: configFileErrorSchema.nullable().exactOptional(),
  restart_needed: z.array(z.string()).exactOptional(),
  symlink_target: z.string().nullable().exactOptional(),
}) satisfies z.ZodType<ConfigStatus>;

export const daemonIdSchema = z.guid() satisfies z.ZodType<DaemonId>;

export const rootSourceSchema = z.enum([
  "dir_variable",
  "efr_home",
  "xdg",
  "run_user",
]) satisfies z.ZodType<RootSource>;

export const rootDirSchema = z.object({
  path: z.string(),
  source: rootSourceSchema,
}) satisfies z.ZodType<RootDir>;

export const daemonRootsSchema = z.object({
  config: rootDirSchema,
  data: rootDirSchema,
  runtime: rootDirSchema,
  state: rootDirSchema,
}) satisfies z.ZodType<DaemonRoots>;

export const fullRefusalSchema = z.enum([
  "root",
  "snapshot_off",
  "undo_off",
  "no_zsh",
]) satisfies z.ZodType<FullRefusal>;

export const fullModeStatusSchema = z.object({
  allowed: z.boolean(),
  refused: fullRefusalSchema.nullable().exactOptional(),
}) satisfies z.ZodType<FullModeStatus>;

export const loginKindSchema = z.enum(["subscription", "api_key"]) satisfies z.ZodType<LoginKind>;

export const providerStatusSchema = z.object({
  active: z.boolean().exactOptional(),
  expires_at: z.iso.datetime({ offset: true }).nullable().exactOptional(),
  key_checked_at: z.iso.datetime({ offset: true }).nullable().exactOptional(),
  key_hint: z.string().nullable().exactOptional(),
  key_refused_at: z.iso.datetime({ offset: true }).nullable().exactOptional(),
  logged_in: z.boolean(),
  login: loginKindSchema.nullable().exactOptional(),
  provider: z.string(),
}) satisfies z.ZodType<ProviderStatus>;

export const sandboxPathsSchema = z.object({
  launcher: z.string().nullable().exactOptional(),
  launcher_sha256_ok: z.boolean().nullable().exactOptional(),
  launcher_source: z.string().nullable().exactOptional(),
  runtime: z.string(),
  state: z.string(),
}) satisfies z.ZodType<SandboxPaths>;

export const adminStatusResultSchema = z.object({
  catalog: catalogStatusSchema.nullable().exactOptional(),
  config: configStatusSchema.nullable().exactOptional(),
  conversations: z.uint32(),
  daemon_id: daemonIdSchema,
  full_mode: fullModeStatusSchema.nullable().exactOptional(),
  pid: z.uint32(),
  protocol: z.uint32(),
  providers: z.array(providerStatusSchema),
  roots: daemonRootsSchema.nullable().exactOptional(),
  sandbox: sandboxStatusSchema.nullable().exactOptional(),
  sandbox_paths: sandboxPathsSchema.nullable().exactOptional(),
  screen_backend: z.string(),
  shells: z.uint32(),
  started_at: z.iso.datetime({ offset: true }),
  version: z.string(),
}) satisfies z.ZodType<AdminStatusResult>;

export const notSavedReasonSchema = z.enum([
  "changed",
  "read_only",
  "load_check",
  "io",
  "reload_timeout",
  "unknown",
]) satisfies z.ZodType<NotSavedReason>;

export const alwaysSavedSchema = z.discriminatedUnion("kind", [
  z.object({ index: z.uint32(), kind: z.literal("saved"), rule: z.string() }),
  z.object({ kind: z.literal("not_saved"), reason: notSavedReasonSchema }),
  z.object({ kind: z.literal("unknown") }),
]) satisfies z.ZodType<AlwaysSaved>;

export const approvalDecisionSchema = z.enum([
  "allow",
  "deny",
  "allow_always",
]) satisfies z.ZodType<ApprovalDecision>;

export const callIdSchema = z.guid() satisfies z.ZodType<CallId>;

export const commandIdSchema = z.guid() satisfies z.ZodType<CommandId>;

export const conversationIdSchema = z.guid() satisfies z.ZodType<ConversationId>;

export const approvalRespondSchema = z.object({
  call_id: callIdSchema,
  command_id: commandIdSchema,
  conversation_id: conversationIdSchema,
  decision: approvalDecisionSchema,
}) satisfies z.ZodType<ApprovalRespond>;

export const seqSchema = z.int().nonnegative() satisfies z.ZodType<Seq>;

export const approvalRespondResultSchema = z.object({
  seq: seqSchema,
}) satisfies z.ZodType<ApprovalRespondResult>;

export const approvedBySchema = z.enum([
  "you",
  "your_rule",
  "reviewer",
  "full_mode",
  "unknown",
]) satisfies z.ZodType<ApprovedBy>;

export const attachHowSchema = z.enum([
  "picked_up",
  "resumed",
  "unknown",
]) satisfies z.ZodType<AttachHow>;

export const base64BytesSchema = z.string() satisfies z.ZodType<Base64Bytes>;

export const blockReasonSchema = z.enum([
  "not_allowed",
  "private_address",
  "sni_mismatch",
  "upload_limit",
  "method",
  "ip_literal",
  "not_tls",
  "bad_request",
  "limit",
  "denied",
  "unknown",
]) satisfies z.ZodType<BlockReason>;

export const blockedSchema = z.object({
  count: z.uint32().exactOptional(),
  host: z.string(),
  port: z.int().min(0).max(65535),
  reason: blockReasonSchema,
}) satisfies z.ZodType<Blocked>;

export const busRefusalSchema = z.object({
  bus: busKindSchema,
  count: z.uint32().exactOptional(),
  member: z.string(),
  name: z.string(),
  path: z.string(),
}) satisfies z.ZodType<BusRefusal>;

export const inputWaitSchema = z.enum(["none", "visible", "hidden"]) satisfies z.ZodType<InputWait>;

export const busyShellSchema = z.object({
  approved_by: approvedBySchema.nullable().exactOptional(),
  call_id: callIdSchema,
  command: z.string(),
  contained: z.boolean(),
  input: inputWaitSchema,
  looks_secret: z.boolean().exactOptional(),
  prompt: z.string().exactOptional(),
  since: z.iso.datetime({ offset: true }),
}) satisfies z.ZodType<BusyShell>;

export const usageSchema = z.object({
  cache_write_1h_tokens: z.int().nonnegative().exactOptional(),
  cache_write_tokens: z.int().nonnegative().exactOptional(),
  cached_input_tokens: z.int().nonnegative().exactOptional(),
  context_tokens: z.int().nonnegative().exactOptional(),
  input_tokens: z.int().nonnegative(),
  output_tokens: z.int().nonnegative(),
  reasoning_tokens: z.int().nonnegative().exactOptional(),
}) satisfies z.ZodType<Usage>;

export const cacheSummarySchema = z.object({
  conversation: usageSchema,
  last_turn: usageSchema.nullable().exactOptional(),
  turns: z.uint32(),
}) satisfies z.ZodType<CacheSummary>;

export const callBackgroundSchema = z.object({
  call_id: callIdSchema,
  command_id: commandIdSchema,
  confirm: z.int().nonnegative().nullable().exactOptional(),
  conversation_id: conversationIdSchema,
}) satisfies z.ZodType<CallBackground>;

export const riskSchema = z.enum(["low", "medium", "high", "critical"]) satisfies z.ZodType<Risk>;

export const userAuthorizationSchema = z.enum([
  "none",
  "low",
  "medium",
  "high",
]) satisfies z.ZodType<UserAuthorization>;

export const verdictSchema = z.enum(["allow", "deny", "ask_user"]) satisfies z.ZodType<Verdict>;

export const judgementSchema = z.object({
  category: z.string(),
  rationale: z.string(),
  risk: riskSchema,
  user_authorization: userAuthorizationSchema,
  verdict: verdictSchema,
}) satisfies z.ZodType<Judgement>;

export const launchSchema = z.discriminatedUnion("kind", [
  z.object({ kind: z.literal("direct") }),
  z.object({ grants: z.array(grantSchema).exactOptional(), kind: z.literal("contained") }),
  z.object({ kind: z.literal("unsandboxed") }),
]) satisfies z.ZodType<Launch>;

export const exitInfoSchema = z.object({
  always: z.string().nullable().exactOptional(),
  facts: z.array(z.string()).exactOptional(),
  grants: z.array(grantSchema).exactOptional(),
  judged: judgementSchema.nullable().exactOptional(),
  kinds: z.array(exitKindSchema),
  launch: launchSchema,
  model_reason: z.string().nullable().exactOptional(),
  user_only: z.boolean().exactOptional(),
}) satisfies z.ZodType<ExitInfo>;

export const jobIdSchema = z.string().regex(/^j[1-9][0-9]{0,8}$/) satisfies z.ZodType<JobId>;

export const callBackgroundResultSchema = z.discriminatedUnion("kind", [
  z.object({
    command: z.string(),
    exits: z.array(exitInfoSchema).exactOptional(),
    kind: z.literal("preview"),
    max_minutes: z.uint32(),
    token: z.int().nonnegative(),
  }),
  z.object({ job_id: jobIdSchema, kind: z.literal("moved") }),
  z.object({ kind: z.literal("unknown") }),
]) satisfies z.ZodType<CallBackgroundResult>;

export const capabilitiesSchema = z.looseObject({
  admin: z.boolean().nullable().exactOptional(),
  approval_local_only: z.boolean().nullable().exactOptional(),
  device_prove: z.boolean().nullable().exactOptional(),
  drafts_coalesced: z.boolean().nullable().exactOptional(),
  screen_snapshots: z.boolean().nullable().exactOptional(),
  search: z.boolean().nullable().exactOptional(),
  snapshot_live: z.boolean().nullable().exactOptional(),
}) satisfies z.ZodType<Capabilities>;

export const colorSchema = z.union([
  z.strictObject({ indexed: z.int().min(0).max(255) }),
  z.strictObject({
    rgb: z.tuple([z.int().min(0).max(255), z.int().min(0).max(255), z.int().min(0).max(255)]),
  }),
]) satisfies z.ZodType<Color>;

export const cellSchema = z.object({
  bg: colorSchema.nullable().exactOptional(),
  bold: z.boolean().exactOptional(),
  fg: colorSchema.nullable().exactOptional(),
  inverse: z.boolean().exactOptional(),
  italic: z.boolean().exactOptional(),
  text: z.string(),
  underline: z.boolean().exactOptional(),
  wide: z.boolean().exactOptional(),
}) satisfies z.ZodType<Cell>;

export const chainStepSchema = z.enum([
  "classifier_model",
  "turn_model",
  "user",
]) satisfies z.ZodType<ChainStep>;

export const changeKindSchema = z.enum([
  "added",
  "modified",
  "deleted",
  "renamed",
]) satisfies z.ZodType<ChangeKind>;

export const requestIdSchema = z.int().nonnegative() satisfies z.ZodType<RequestId>;

export const clientCancelSchema = z.strictObject({
  cancel: requestIdSchema,
}) satisfies z.ZodType<ClientCancel>;

export const conversationCompactSchema = z.object({
  command_id: commandIdSchema,
  conversation_id: conversationIdSchema,
  focus: z.string().nullable().exactOptional(),
}) satisfies z.ZodType<ConversationCompact>;

export const conversationContextSchema = z.object({
  conversation_id: conversationIdSchema,
  model: z.string().nullable().exactOptional(),
}) satisfies z.ZodType<ConversationContext>;

export const conversationDeleteSchema = z.object({
  command_id: commandIdSchema,
  conversation_id: conversationIdSchema,
  force: z.boolean().exactOptional(),
  stop: z.boolean().exactOptional(),
}) satisfies z.ZodType<ConversationDelete>;

export const turnIdSchema = z.guid() satisfies z.ZodType<TurnId>;

export const conversationDiffSchema = z.object({
  conversation_id: conversationIdSchema.nullable().exactOptional(),
  stat: z.boolean().exactOptional(),
  turn_id: turnIdSchema.nullable().exactOptional(),
}) satisfies z.ZodType<ConversationDiff>;

export const pageCursorSchema = z.string() satisfies z.ZodType<PageCursor>;

export const conversationHistorySchema = z.object({
  conversation_id: conversationIdSchema,
  cursor: pageCursorSchema.nullable().exactOptional(),
  limit: z.uint32().nullable().exactOptional(),
}) satisfies z.ZodType<ConversationHistory>;

export const desktopEnvSchema = z.object({
  display: z.string().nullable().exactOptional(),
  has_xauthority: z.boolean().exactOptional(),
  hyprland_instance_signature: z.string().nullable().exactOptional(),
  i3sock: z.string().nullable().exactOptional(),
  niri_socket: z.string().nullable().exactOptional(),
  swaysock: z.string().nullable().exactOptional(),
  wayland_display: z.string().nullable().exactOptional(),
}) satisfies z.ZodType<DesktopEnv>;

export const shellContextSchema = z.object({
  desktop: desktopEnvSchema.nullable().exactOptional(),
  hostname: z.string().nullable().exactOptional(),
  last_status: z.int32().nullable().exactOptional(),
  oldpwd: z.string().nullable().exactOptional(),
  pwd: z.string(),
  shell_pid: z.uint32().nullable().exactOptional(),
  shlvl: z.uint32().nullable().exactOptional(),
  ssh_connection: z.string().nullable().exactOptional(),
  tty: z.string().nullable().exactOptional(),
}) satisfies z.ZodType<ShellContext>;

export const conversationResumeSchema = z.object({
  command_id: commandIdSchema,
  context: shellContextSchema,
  conversation_id: conversationIdSchema,
  take: z.boolean().exactOptional(),
}) satisfies z.ZodType<ConversationResume>;

export const conversationSearchSchema = z.object({
  conversation_id: conversationIdSchema,
  limit: z.uint32().nullable().exactOptional(),
  query: z.string(),
}) satisfies z.ZodType<ConversationSearch>;

export const conversationSubscribeSchema = z.object({
  after_seq: seqSchema.nullable().exactOptional(),
  answers_input: z.boolean().exactOptional(),
  conversation_id: conversationIdSchema,
  drafts: z.boolean().exactOptional(),
}) satisfies z.ZodType<ConversationSubscribe>;

export const conversationsListSchema = z.object({
  cursor: pageCursorSchema.nullable().exactOptional(),
  limit: z.uint32().nullable().exactOptional(),
  tty: z.string().nullable().exactOptional(),
}) satisfies z.ZodType<ConversationsList>;

export const conversationsRecentSchema = z.object({
  limit: z.uint32().nullable().exactOptional(),
  pwd: z.string().nullable().exactOptional(),
  tty: z.string().nullable().exactOptional(),
}) satisfies z.ZodType<ConversationsRecent>;

export const conversationsSearchSchema = z.object({
  limit: z.uint32().nullable().exactOptional(),
  pwd: z.string().nullable().exactOptional(),
  query: z.string(),
  stats: z.boolean().exactOptional(),
  tty: z.string().nullable().exactOptional(),
}) satisfies z.ZodType<ConversationsSearch>;

export const deviceEnrollSchema = z.object({
  name: z.string(),
  public_key: base64BytesSchema,
  token: secretTextSchema,
}) satisfies z.ZodType<DeviceEnroll>;

export const deviceListSchema = z.object({}) satisfies z.ZodType<DeviceList>;

export const pairingIdSchema = z.guid() satisfies z.ZodType<PairingId>;

export const devicePairConfirmSchema = z.object({
  accept: z.boolean(),
  pairing_id: pairingIdSchema,
}) satisfies z.ZodType<DevicePairConfirm>;

export const devicePairStartSchema = z.object({
  name: z.string().nullable().exactOptional(),
}) satisfies z.ZodType<DevicePairStart>;

export const deviceIdSchema = z.guid() satisfies z.ZodType<DeviceId>;

export const deviceProveSchema = z.object({
  device_id: deviceIdSchema,
  signature: base64BytesSchema,
}) satisfies z.ZodType<DeviceProve>;

export const deviceRevokeSchema = z.object({
  device_id: deviceIdSchema,
}) satisfies z.ZodType<DeviceRevoke>;

export const originSchema = z.enum(["shell", "cli", "proxy", "phone"]) satisfies z.ZodType<Origin>;

export const helloSchema = z.object({
  capabilities: capabilitiesSchema.exactOptional(),
  client: z.string().nullable().exactOptional(),
  device_id: deviceIdSchema.nullable().exactOptional(),
  origin: originSchema,
  pid: z.uint32().nullable().exactOptional(),
  protocol: z.uint32(),
  tty: z.string().nullable().exactOptional(),
}) satisfies z.ZodType<Hello>;

export const inputRespondSchema = z.object({
  call_id: callIdSchema,
  conversation_id: conversationIdSchema,
  hidden: z.boolean(),
  manual: z.boolean().exactOptional(),
  text: secretTextSchema,
}) satisfies z.ZodType<InputRespond>;

export const jobsListSchema = z.object({
  conversation_id: conversationIdSchema.nullable().exactOptional(),
}) satisfies z.ZodType<JobsList>;

export const jobsStopSchema = z.object({
  command_id: commandIdSchema,
  conversation_id: conversationIdSchema.nullable().exactOptional(),
  job_id: jobIdSchema,
}) satisfies z.ZodType<JobsStop>;

export const ptyIdSchema = z.guid() satisfies z.ZodType<PtyId>;

export const leaseReportSchema = z.object({
  conversations: z.array(conversationIdSchema).exactOptional(),
  ptys: z.array(ptyIdSchema).exactOptional(),
  visible: z.boolean(),
}) satisfies z.ZodType<LeaseReport>;

export const modelsListSchema = z.object({}) satisfies z.ZodType<ModelsList>;

export const presenceSchema = z.enum(["present", "away"]) satisfies z.ZodType<Presence>;

export const presenceSetSchema = z.object({
  command_id: commandIdSchema,
  conversation_id: conversationIdSchema.nullable().exactOptional(),
  presence: presenceSchema,
}) satisfies z.ZodType<PresenceSet>;

export const projectsListSchema = z.object({}) satisfies z.ZodType<ProjectsList>;

export const recentCommandSchema = z.object({
  command: z.string(),
  cwd: z.string().nullable().exactOptional(),
  duration_ms: z.int().nonnegative().nullable().exactOptional(),
  ended_at: z.iso.datetime({ offset: true }).nullable().exactOptional(),
  status: z.int32().nullable().exactOptional(),
}) satisfies z.ZodType<RecentCommand>;

export const modeSchema = z.enum(["manual", "cautious", "auto", "full"]) satisfies z.ZodType<Mode>;

export const turnSettingsSchema = z.object({
  effort: z.string().nullable().exactOptional(),
  mode: modeSchema.nullable().exactOptional(),
  model: z.string().nullable().exactOptional(),
}) satisfies z.ZodType<TurnSettings>;

export const promptSendSchema = z.object({
  command_id: commandIdSchema,
  context: shellContextSchema.nullable().exactOptional(),
  conversation_id: conversationIdSchema.nullable().exactOptional(),
  last_command: z.string().nullable().exactOptional(),
  new_conversation: z.boolean().exactOptional(),
  recent_commands: z.array(recentCommandSchema).exactOptional(),
  settings: turnSettingsSchema.exactOptional(),
  text: z.string(),
}) satisfies z.ZodType<PromptSend>;

export const withdrawTargetSchema = z.discriminatedUnion("kind", [
  z.object({ kind: z.literal("turn"), turn_id: turnIdSchema }),
  z.object({ kind: z.literal("newest_from_tty"), tty: z.string() }),
]) satisfies z.ZodType<WithdrawTarget>;

export const promptWithdrawSchema = z.object({
  command_id: commandIdSchema,
  conversation_id: conversationIdSchema,
  target: withdrawTargetSchema,
}) satisfies z.ZodType<PromptWithdraw>;

export const ptyAttachSchema = z.object({
  pty_id: ptyIdSchema,
  scrollback_rows: z.uint32().nullable().exactOptional(),
  since_seq: seqSchema.nullable().exactOptional(),
}) satisfies z.ZodType<PtyAttach>;

export const sizeSchema = z.object({
  cols: z.int().min(0).max(65535),
  rows: z.int().min(0).max(65535),
}) satisfies z.ZodType<Size>;

export const ptyResizeSchema = z.object({
  pty_id: ptyIdSchema,
  size: sizeSchema,
}) satisfies z.ZodType<PtyResize>;

export const ptyWriteSchema = z.object({
  data: base64BytesSchema,
  pty_id: ptyIdSchema,
}) satisfies z.ZodType<PtyWrite>;

export const sandboxExplainSchema = z.object({
  cwd: z.string().nullable().exactOptional(),
  path: z.string(),
}) satisfies z.ZodType<SandboxExplain>;

export const questionIdSchema = z.guid() satisfies z.ZodType<QuestionId>;

export const sandboxSurfaceRespondSchema = z.object({
  command_id: commandIdSchema,
  conversation_id: conversationIdSchema,
  keep: z.boolean(),
  question_id: questionIdSchema,
}) satisfies z.ZodType<SandboxSurfaceRespond>;

export const shellInterruptSchema = z.object({
  call_id: callIdSchema,
  close: z.boolean().exactOptional(),
  command_id: commandIdSchema,
  conversation_id: conversationIdSchema,
}) satisfies z.ZodType<ShellInterrupt>;

export const shellStatusSchema = z.object({
  conversation_id: conversationIdSchema,
}) satisfies z.ZodType<ShellStatus>;

export const lateSteerSchema = z.object({
  context: shellContextSchema.nullable().exactOptional(),
  kind: z.literal("queue"),
  last_command: z.string().nullable().exactOptional(),
  recent_commands: z.array(recentCommandSchema).exactOptional(),
  settings: turnSettingsSchema.exactOptional(),
}) satisfies z.ZodType<LateSteer>;

export const turnInterruptSchema = z.object({
  command_id: commandIdSchema,
  conversation_id: conversationIdSchema,
  resend_as: lateSteerSchema.nullable().exactOptional(),
  resend_steers: z.array(seqSchema).exactOptional(),
  turn_id: turnIdSchema.nullable().exactOptional(),
  withdraw: z.array(turnIdSchema).exactOptional(),
  withdraw_steers: z.array(seqSchema).exactOptional(),
}) satisfies z.ZodType<TurnInterrupt>;

export const turnSteerSchema = z.object({
  command_id: commandIdSchema,
  conversation_id: conversationIdSchema,
  if_late: lateSteerSchema.nullable().exactOptional(),
  text: z.string(),
  turn_id: turnIdSchema.nullable().exactOptional(),
}) satisfies z.ZodType<TurnSteer>;

export const turnUndoSchema = z.object({
  command_id: commandIdSchema,
  conversation_id: conversationIdSchema.nullable().exactOptional(),
  dry_run: z.boolean().exactOptional(),
  force: z.boolean().exactOptional(),
  list: z.boolean().exactOptional(),
  redo: z.boolean().exactOptional(),
  turn_id: turnIdSchema.nullable().exactOptional(),
}) satisfies z.ZodType<TurnUndo>;

export const methodSchema = z.discriminatedUnion("method", [
  z.object({ method: z.literal("hello"), params: helloSchema }),
  z.object({ method: z.literal("conversations.list"), params: conversationsListSchema }),
  z.object({ method: z.literal("conversation.subscribe"), params: conversationSubscribeSchema }),
  z.object({ method: z.literal("conversation.history"), params: conversationHistorySchema }),
  z.object({ method: z.literal("prompt.send"), params: promptSendSchema }),
  z.object({ method: z.literal("turn.interrupt"), params: turnInterruptSchema }),
  z.object({ method: z.literal("turn.steer"), params: turnSteerSchema }),
  z.object({ method: z.literal("approval.respond"), params: approvalRespondSchema }),
  z.object({ method: z.literal("pty.attach"), params: ptyAttachSchema }),
  z.object({ method: z.literal("pty.write"), params: ptyWriteSchema }),
  z.object({ method: z.literal("pty.resize"), params: ptyResizeSchema }),
  z.object({ method: z.literal("input.respond"), params: inputRespondSchema }),
  z.object({ method: z.literal("lease.report"), params: leaseReportSchema }),
  z.object({ method: z.literal("models.list"), params: modelsListSchema }),
  z.object({ method: z.literal("projects.list"), params: projectsListSchema }),
  z.object({ method: z.literal("admin.project_add"), params: adminProjectAddSchema }),
  z.object({ method: z.literal("admin.project_remove"), params: adminProjectRemoveSchema }),
  z.object({ method: z.literal("admin.status"), params: adminStatusSchema }),
  z.object({ method: z.literal("admin.config_reload"), params: adminConfigReloadSchema }),
  z.object({ method: z.literal("admin.login_openai"), params: adminLoginOpenAiSchema }),
  z.object({ method: z.literal("admin.login_api_key"), params: adminLoginApiKeySchema }),
  z.object({ method: z.literal("admin.logout"), params: adminLogoutSchema }),
  z.object({ method: z.literal("sandbox.explain"), params: sandboxExplainSchema }),
  z.object({ method: z.literal("sandbox.surface_respond"), params: sandboxSurfaceRespondSchema }),
  z.object({ method: z.literal("admin.sandbox_check"), params: adminSandboxCheckSchema }),
  z.object({ method: z.literal("conversation.diff"), params: conversationDiffSchema }),
  z.object({ method: z.literal("prompt.withdraw"), params: promptWithdrawSchema }),
  z.object({ method: z.literal("conversation.compact"), params: conversationCompactSchema }),
  z.object({ method: z.literal("presence.set"), params: presenceSetSchema }),
  z.object({ method: z.literal("turn.undo"), params: turnUndoSchema }),
  z.object({ method: z.literal("jobs.list"), params: jobsListSchema }),
  z.object({ method: z.literal("jobs.stop"), params: jobsStopSchema }),
  z.object({ method: z.literal("conversation.context"), params: conversationContextSchema }),
  z.object({ method: z.literal("conversations.recent"), params: conversationsRecentSchema }),
  z.object({ method: z.literal("conversation.resume"), params: conversationResumeSchema }),
  z.object({ method: z.literal("shell.interrupt"), params: shellInterruptSchema }),
  z.object({ method: z.literal("shell.status"), params: shellStatusSchema }),
  z.object({ method: z.literal("call.background"), params: callBackgroundSchema }),
  z.object({ method: z.literal("conversations.search"), params: conversationsSearchSchema }),
  z.object({ method: z.literal("conversation.search"), params: conversationSearchSchema }),
  z.object({ method: z.literal("conversation.delete"), params: conversationDeleteSchema }),
  z.object({ method: z.literal("device.pair_start"), params: devicePairStartSchema }),
  z.object({ method: z.literal("device.pair_confirm"), params: devicePairConfirmSchema }),
  z.object({ method: z.literal("device.enroll"), params: deviceEnrollSchema }),
  z.object({ method: z.literal("device.prove"), params: deviceProveSchema }),
  z.object({ method: z.literal("device.list"), params: deviceListSchema }),
  z.object({ method: z.literal("device.revoke"), params: deviceRevokeSchema }),
]) satisfies z.ZodType<Method>;

export const clientRequestSchema = z.intersection(
  z.object({ id: requestIdSchema }),
  methodSchema,
) satisfies z.ZodType<ClientRequest>;

export const clientFrameSchema = z.union([
  clientRequestSchema,
  clientCancelSchema,
]) satisfies z.ZodType<ClientFrame>;

export const compactionIdSchema = z.guid() satisfies z.ZodType<CompactionId>;

export const compactionTriggerSchema = z.enum([
  "auto",
  "manual",
  "overflow",
]) satisfies z.ZodType<CompactionTrigger>;

export const compactionSchema = z.object({
  compaction_id: compactionIdSchema,
  focus: z.string().nullable().exactOptional(),
  fresh: z.string().nullable().exactOptional(),
  kept_turns: z.uint32(),
  limit: z.int().nonnegative(),
  model: z.string(),
  omitted_messages: z.uint32().exactOptional(),
  omitted_turns: z.uint32().exactOptional(),
  pruned_outputs: z.uint32().exactOptional(),
  pruned_tokens: z.int().nonnegative().exactOptional(),
  summary: z.string().nullable().exactOptional(),
  through_message: z.uint32().nullable().exactOptional(),
  through_turn: turnIdSchema,
  tokens_after: z.int().nonnegative(),
  tokens_before: z.int().nonnegative(),
  trigger: compactionTriggerSchema,
  turn_id: turnIdSchema.nullable().exactOptional(),
  usage: usageSchema.nullable().exactOptional(),
  window: z.int().nonnegative(),
}) satisfies z.ZodType<Compaction>;

export const compactionBriefSchema = z.object({
  at: z.iso.datetime({ offset: true }),
  pruned_outputs: z.uint32().exactOptional(),
  summarized: z.boolean(),
  tokens_after: z.int().nonnegative(),
  tokens_before: z.int().nonnegative(),
  trigger: compactionTriggerSchema,
}) satisfies z.ZodType<CompactionBrief>;

export const compactionSummarySchema = z.object({
  count: z.uint32(),
  newest: compactionBriefSchema.nullable().exactOptional(),
}) satisfies z.ZodType<CompactionSummary>;

export const contextPartKindSchema = z.enum([
  "system",
  "tools",
  "summary",
  "prompts",
  "preambles",
  "notices",
  "answers",
  "tool_calls",
  "tool_outputs",
  "pruned",
  "unknown",
]) satisfies z.ZodType<ContextPartKind>;

export const contextItemSchema = z.object({
  kind: contextPartKindSchema,
  label: z.string().nullable().exactOptional(),
  tokens: z.int().nonnegative(),
  tool: z.string().nullable().exactOptional(),
  turn: z.uint32().nullable().exactOptional(),
}) satisfies z.ZodType<ContextItem>;

export const contextPartSchema = z.object({
  count: z.uint32(),
  kind: contextPartKindSchema,
  tokens: z.int().nonnegative(),
}) satisfies z.ZodType<ContextPart>;

export const contextUseSchema = z.object({
  limit: z.int().nonnegative(),
  tokens: z.int().nonnegative(),
  window: z.int().nonnegative(),
}) satisfies z.ZodType<ContextUse>;

export const conversationCompactResultSchema = z.object({
  compaction: compactionSchema,
  seq: seqSchema,
}) satisfies z.ZodType<ConversationCompactResult>;

export const countedUseSchema = z.object({
  at: z.iso.datetime({ offset: true }),
  tokens: z.int().nonnegative(),
  turn_id: turnIdSchema,
}) satisfies z.ZodType<CountedUse>;

export const conversationContextResultSchema = z.object({
  auto: z.boolean(),
  cache: cacheSummarySchema,
  compactions: compactionSummarySchema,
  counted: countedUseSchema.nullable().exactOptional(),
  estimate: z.int().nonnegative(),
  hard_cap: z.int().nonnegative(),
  largest: z.array(contextItemSchema),
  model: z.string(),
  parts: z.array(contextPartSchema),
  running: z.boolean().exactOptional(),
  trigger: z.int().nonnegative(),
  window: z.int().nonnegative(),
  window_known: z.boolean(),
}) satisfies z.ZodType<ConversationContextResult>;

export const conversationDeleteResultSchema = z.discriminatedUnion("kind", [
  z.object({
    conversation_id: conversationIdSchema,
    jobs_stopped: z.uint32().exactOptional(),
    kind: z.literal("deleted"),
    not_removed: z.array(z.string()).exactOptional(),
    title: z.string().nullable().exactOptional(),
  }),
  z.object({
    kind: z.literal("held"),
    title: z.string().nullable().exactOptional(),
    tty: z.string(),
  }),
  z.object({ kind: z.literal("unknown") }),
]) satisfies z.ZodType<ConversationDeleteResult>;

export const fileChangeSchema = z.object({
  added: z.uint32(),
  binary: z.boolean().exactOptional(),
  from: z.string().nullable().exactOptional(),
  kind: changeKindSchema,
  path: z.string(),
  removed: z.uint32(),
}) satisfies z.ZodType<FileChange>;

export const fileChangesSchema = z.object({
  added: z.uint32().exactOptional(),
  files: z.array(fileChangeSchema),
  more: z.uint32().exactOptional(),
  removed: z.uint32().exactOptional(),
}) satisfies z.ZodType<FileChanges>;

export const conversationDiffResultSchema = z.object({
  changes: fileChangesSchema,
  diff: z.string().nullable().exactOptional(),
  turn_id: turnIdSchema,
}) satisfies z.ZodType<ConversationDiffResult>;

export const modeFallbackSchema = z.object({
  asked: modeSchema,
  reason: z.string(),
}) satisfies z.ZodType<ModeFallback>;

export const overriddenSettingsSchema = z.object({
  effort: z.boolean().exactOptional(),
  mode: z.boolean().exactOptional(),
  model: z.boolean().exactOptional(),
}) satisfies z.ZodType<OverriddenSettings>;

export const effectiveSettingsSchema = z.object({
  effort: z.string().nullable().exactOptional(),
  fallback: modeFallbackSchema.nullable().exactOptional(),
  mode: modeSchema,
  model: z.string(),
  overridden: overriddenSettingsSchema.exactOptional(),
}) satisfies z.ZodType<EffectiveSettings>;

export const errorCodeSchema = z.enum([
  "protocol_mismatch",
  "unauthorized",
  "forbidden",
  "not_found",
  "conflict",
  "busy",
  "overflow",
  "cancelled",
  "invalid",
  "internal",
]) satisfies z.ZodType<ErrorCode>;

export const errorBodySchema = z.object({
  code: errorCodeSchema,
  data: z.unknown().exactOptional(),
  message: z.string(),
}) satisfies z.ZodType<ErrorBody>;

export const gitCodeFactSchema = z.object({
  file: z.string(),
  item: z.string(),
  repo: z.string(),
  writable: z.boolean(),
}) satisfies z.ZodType<GitCodeFact>;

export const gitCodeFactsSchema = z.object({
  checked: z.boolean(),
  items: z.array(gitCodeFactSchema).exactOptional(),
}) satisfies z.ZodType<GitCodeFacts>;

export const gitCountsSchema = z.object({
  modified: z.uint32(),
  staged: z.uint32(),
  untracked: z.uint32(),
}) satisfies z.ZodType<GitCounts>;

export const hostFactSchema = z.object({
  host: z.string(),
  named_in_user_messages: z.boolean(),
  on_allow_list: z.boolean(),
  project_remote: z.boolean().exactOptional(),
  refused_by_proxy: z.boolean(),
}) satisfies z.ZodType<HostFact>;

export const programFactSchema = z.object({
  changed_this_turn: z.boolean(),
  in_write_root: z.boolean(),
  resolved: z.string().nullable().exactOptional(),
  word: z.string(),
}) satisfies z.ZodType<ProgramFact>;

export const pathClassNameSchema = z.enum([
  "scratch",
  "user_config",
  "user_data",
  "system",
  "secrets",
  "cache",
  "build_output",
  "trash",
]) satisfies z.ZodType<PathClassName>;

export const targetFactSchema = z.object({
  backup_before_write: z.boolean().exactOptional(),
  class: pathClassNameSchema.nullable().exactOptional(),
  exists: z.boolean(),
  floor: z.boolean(),
  in_write_root: z.boolean(),
  masked: z.boolean().exactOptional(),
  named_in_user_messages: z.boolean(),
  path: z.string(),
  synced: z.boolean(),
}) satisfies z.ZodType<TargetFact>;

export const exitFactsSchema = z.object({
  denials_in_a_row: z.int().min(0).max(255).exactOptional(),
  denials_this_turn: z.int().min(0).max(255).exactOptional(),
  desktop_read: z.boolean().exactOptional(),
  git_code: gitCodeFactsSchema.nullable().exactOptional(),
  git_status: gitCountsSchema.nullable().exactOptional(),
  hosts: z.array(hostFactSchema).exactOptional(),
  outside_commands: z.array(z.string()).exactOptional(),
  previous_exits_this_turn: z
    .array(
      z.tuple([z.union([exitKindSchema, verdictSchema]), z.union([exitKindSchema, verdictSchema])]),
    )
    .exactOptional(),
  programs: z.array(programFactSchema).exactOptional(),
  refusals_in_a_row: z.int().min(0).max(255).exactOptional(),
  repo_surface_changed_this_turn: z.boolean().exactOptional(),
  sandbox_export_names: z.array(z.string()).exactOptional(),
  snapshot_covers: z.boolean().nullable().exactOptional(),
  targets: z.array(targetFactSchema).exactOptional(),
  upload_patterns: z.array(z.string()).exactOptional(),
}) satisfies z.ZodType<ExitFacts>;

export const exitRecordSchema = z.object({
  action: actionFactsSchema,
  facts: exitFactsSchema,
  user_messages: z.array(z.string()),
  version: z.uint32(),
}) satisfies z.ZodType<ExitRecord>;

export const forwardOutcomeSchema = z.enum([
  "forwarded",
  "in_use",
  "refused",
  "limit",
  "stopped",
  "unknown",
]) satisfies z.ZodType<ForwardOutcome>;

export const heldReasonSchema = z.enum([
  "not_auto",
  "wake_limit",
  "wake_turn_job",
  "daemon_restart",
  "wakes_off",
  "unknown",
]) satisfies z.ZodType<HeldReason>;

export const jobCgroupSchema = z.object({
  cpu_usec: z.int().nonnegative().nullable().exactOptional(),
  memory_bytes: z.int().nonnegative().nullable().exactOptional(),
  off: z.string().nullable().exactOptional(),
}) satisfies z.ZodType<JobCgroup>;

export const jobStateSchema = z.enum([
  "running",
  "exited",
  "stopped",
  "timed_out",
  "failed",
  "unknown",
]) satisfies z.ZodType<JobState>;

export const jobStopBySchema = z.enum([
  "model",
  "user",
  "time_limit",
  "daemon_stop",
  "daemon_restart",
  "guard",
  "full_off",
  "unknown",
]) satisfies z.ZodType<JobStopBy>;

export const movedCallSchema = z.object({
  after_secs: z.int().nonnegative(),
  call_id: callIdSchema,
  exits: z.array(exitInfoSchema).exactOptional(),
  launcher: z.boolean().exactOptional(),
  turn_id: turnIdSchema,
}) satisfies z.ZodType<MovedCall>;

export const portForwardSchema = z.object({
  listen: z.string(),
  port: z.int().min(0).max(65535),
}) satisfies z.ZodType<PortForward>;

export const watchSchema = z.object({
  max_wakes: z.int().min(0).max(255),
  pattern: z.string(),
}) satisfies z.ZodType<Watch>;

export const jobInfoSchema = z.object({
  call_id: callIdSchema,
  cgroup: jobCgroupSchema.nullable().exactOptional(),
  command: z.string(),
  conversation_id: conversationIdSchema,
  ended_at: z.iso.datetime({ offset: true }).nullable().exactOptional(),
  ends_by: z.iso.datetime({ offset: true }),
  exit_code: z.int32().nullable().exactOptional(),
  forwards: z.array(portForwardSchema).exactOptional(),
  id: jobIdSchema,
  launch: launchSchema,
  moved_from: movedCallSchema.nullable().exactOptional(),
  output: z.string(),
  output_bytes: z.int().nonnegative(),
  started_at: z.iso.datetime({ offset: true }),
  state: jobStateSchema,
  stopped_by: jobStopBySchema.nullable().exactOptional(),
  turn_id: turnIdSchema,
  waits_input: z.boolean().exactOptional(),
  wakes: z.int().min(0).max(255).exactOptional(),
  watch: watchSchema.nullable().exactOptional(),
}) satisfies z.ZodType<JobInfo>;

export const judgeKindSchema = z.enum([
  "classifier",
  "user",
  "floor",
  "always",
  "presence_expired",
  "full",
]) satisfies z.ZodType<JudgeKind>;

export const noticeKindSchema = z.enum([
  "exited",
  "watch",
  "unknown",
]) satisfies z.ZodType<NoticeKind>;

export const pauseReasonSchema = z.enum(["in_a_row", "per_turn"]) satisfies z.ZodType<PauseReason>;

export const presenceSourceSchema = z.enum([
  "input",
  "user",
  "timeout",
]) satisfies z.ZodType<PresenceSource>;

export const undoReasonSchema = z.enum([
  "changed_after_turn",
  "no_record_after_turn",
  "left_out",
  "masked",
  "over_limit",
  "new_in_turn",
  "later_turn_not_undone",
  "no_mirror",
  "objects_missing",
  "outside_turn_calls",
  "already_done",
  "root_replaced",
  "unknown",
  "call_unfinished",
  "snapshot_failed",
  "store_changed",
  "store_full",
  "staged_only",
]) satisfies z.ZodType<UndoReason>;

export const refChangeSchema = z.object({
  applied: z.boolean().nullable().exactOptional(),
  from: z.string().nullable().exactOptional(),
  name: z.string(),
  reason: undoReasonSchema.nullable().exactOptional(),
  root: z.string(),
  to: z.string().nullable().exactOptional(),
}) satisfies z.ZodType<RefChange>;

export const reportedFileSchema = z.object({
  detail: z.string().nullable().exactOptional(),
  path: z.string(),
}) satisfies z.ZodType<ReportedFile>;

export const routeScopeSchema = z.discriminatedUnion("kind", [
  z.object({ kind: z.literal("project"), value: projectIdSchema }),
  z.object({ kind: z.literal("outside") }),
  z.object({ kind: z.literal("unknown") }),
]) satisfies z.ZodType<RouteScope>;

export const surfaceChangeSchema = z.object({
  key: z.string().nullable().exactOptional(),
  path: z.string(),
  quarantined: z.boolean(),
  rule: z.string(),
}) satisfies z.ZodType<SurfaceChange>;

export const sandboxSummarySchema = z.object({
  background_stopped: z.array(z.string()).exactOptional(),
  blocked: z.array(blockedSchema).exactOptional(),
  blocked_more: z.uint32().exactOptional(),
  bus_refused: z.array(busRefusalSchema).exactOptional(),
  bus_refused_more: z.uint32().exactOptional(),
  cache_deletes: z.array(z.string()).exactOptional(),
  cache_deletes_more: z.uint32().exactOptional(),
  confined: z.boolean(),
  cwd_changed: z.boolean().exactOptional(),
  dropped: z.array(z.string()).exactOptional(),
  kept_out: z.array(z.string()).exactOptional(),
  promoted: z.array(z.string()).exactOptional(),
  proxied: z.boolean().exactOptional(),
  setup_error: z.string().nullable().exactOptional(),
  surface_changes: z.array(surfaceChangeSchema).exactOptional(),
  survivors: z.array(z.string()).exactOptional(),
}) satisfies z.ZodType<SandboxSummary>;

export const shellCommandEndSchema = z.enum([
  "ended",
  "stopped",
  "limit",
  "closed",
  "shell_exited",
  "moved",
  "unknown",
]) satisfies z.ZodType<ShellCommandEnd>;

export const snapshotPhaseSchema = z.enum(["pre", "post"]) satisfies z.ZodType<SnapshotPhase>;

export const stopReasonSchema = z.enum([
  "end_turn",
  "max_tokens",
  "refusal",
  "content_filter",
  "pause_turn",
]) satisfies z.ZodType<StopReason>;

export const turnStopSchema = z.object({
  empty: z.boolean().exactOptional(),
  reason: stopReasonSchema,
}) satisfies z.ZodType<TurnStop>;

export const rootFilesSchema = z.object({
  files: z.uint32(),
  refs: z.uint32().exactOptional(),
  shown: z.string(),
}) satisfies z.ZodType<RootFiles>;

export const turnUndoInfoSchema = z.object({
  home_files: z.uint32().exactOptional(),
  not_covered: z.array(z.string()).exactOptional(),
  not_covered_more: z.uint32().exactOptional(),
  roots: z.array(rootFilesSchema).exactOptional(),
  undoable: z.boolean().exactOptional(),
}) satisfies z.ZodType<TurnUndoInfo>;

export const undoIdSchema = z.guid() satisfies z.ZodType<UndoId>;

export const eventSchema = z.discriminatedUnion("kind", [
  z.object({
    kind: z.literal("conversation_created"),
    origin: originSchema,
    route: routeScopeSchema.nullable().exactOptional(),
    tty: z.string().nullable().exactOptional(),
  }),
  z.object({
    how: attachHowSchema,
    kind: z.literal("terminal_attached"),
    route: routeScopeSchema,
    shell_pid: z.uint32().nullable().exactOptional(),
    tty: z.string(),
  }),
  z.object({
    command_id: commandIdSchema,
    context: shellContextSchema.nullable().exactOptional(),
    kind: z.literal("prompt_queued"),
    origin: originSchema,
    settings: turnSettingsSchema.exactOptional(),
    steers: z.array(seqSchema).exactOptional(),
    text: z.string(),
    turn_id: turnIdSchema,
  }),
  z.object({ kind: z.literal("prompt_withdrawn"), origin: originSchema, turn_id: turnIdSchema }),
  z.object({ kind: z.literal("prompt_held"), turn_id: turnIdSchema }),
  z.object({
    cwd: z.string(),
    kind: z.literal("turn_started"),
    scope: scopeSchema,
    settings: effectiveSettingsSchema.nullable().exactOptional(),
    turn_id: turnIdSchema,
  }),
  z.object({
    from: scopeSchema,
    kind: z.literal("scope_changed"),
    to: scopeSchema,
    turn_id: turnIdSchema,
  }),
  z.object({
    delta: z.string(),
    index: z.uint32(),
    kind: z.literal("assistant_message_updated"),
    offset: z.int().nonnegative(),
    turn_id: turnIdSchema,
  }),
  z.object({
    index: z.uint32(),
    kind: z.literal("assistant_message_completed"),
    text: z.string(),
    turn_id: turnIdSchema,
  }),
  z.object({
    call_id: callIdSchema,
    freeform: z.boolean().exactOptional(),
    input: z.unknown(),
    kind: z.literal("tool_call_started"),
    launch: launchSchema.nullable().exactOptional(),
    manual_input: z.boolean().exactOptional(),
    tool: z.string(),
    turn_id: turnIdSchema,
  }),
  z.object({
    bytes: z.int().nonnegative(),
    call_id: callIdSchema,
    kind: z.literal("tool_call_output_updated"),
    tail: z.string(),
    turn_id: turnIdSchema,
  }),
  z.object({
    call_id: callIdSchema,
    input: inputWaitSchema,
    kind: z.literal("tool_call_input_changed"),
    looks_secret: z.boolean().exactOptional(),
    turn_id: turnIdSchema,
  }),
  z.object({
    call_id: callIdSchema,
    changes: fileChangesSchema.nullable().exactOptional(),
    diff: z.string().nullable().exactOptional(),
    exit_code: z.int32().nullable().exactOptional(),
    is_error: z.boolean(),
    kind: z.literal("tool_call_completed"),
    left_running: z.boolean().exactOptional(),
    output: z.string(),
    refusal: z.string().nullable().exactOptional(),
    sandbox: sandboxSummarySchema.nullable().exactOptional(),
    truncated: z.boolean(),
    turn_id: turnIdSchema,
  }),
  z.object({
    call_id: callIdSchema,
    diff_preview: z.string().nullable().exactOptional(),
    exit: exitInfoSchema.nullable().exactOptional(),
    expires_in_secs: z.uint32().nullable().exactOptional(),
    interactive: z.boolean().exactOptional(),
    kind: z.literal("approval_requested"),
    local_only: z.boolean().exactOptional(),
    reason: z.string().nullable().exactOptional(),
    summary: z.string(),
    turn_id: turnIdSchema,
  }),
  z.object({
    call_id: callIdSchema,
    decision: approvalDecisionSchema,
    kind: z.literal("approval_resolved"),
    origin: originSchema,
    turn_id: turnIdSchema,
  }),
  z.object({
    away: z.boolean().exactOptional(),
    call_id: callIdSchema,
    kind: z.literal("approval_expired"),
    turn_id: turnIdSchema,
  }),
  z.object({ kind: z.literal("turn_steered"), text: z.string(), turn_id: turnIdSchema }),
  z.object({
    kind: z.literal("steering_delivered"),
    steers: z.array(seqSchema),
    turn_id: turnIdSchema,
  }),
  z.object({
    kind: z.literal("steering_withdrawn"),
    origin: originSchema,
    steers: z.array(seqSchema),
    turn_id: turnIdSchema,
  }),
  z.object({
    kind: z.literal("turn_interrupt_requested"),
    origin: originSchema,
    turn_id: turnIdSchema,
  }),
  z.object({
    changes: fileChangesSchema.nullable().exactOptional(),
    context: contextUseSchema.nullable().exactOptional(),
    kind: z.literal("turn_interrupted"),
    turn_id: turnIdSchema,
    undo: turnUndoInfoSchema.nullable().exactOptional(),
    usage: usageSchema.nullable().exactOptional(),
  }),
  z.object({
    changes: fileChangesSchema.nullable().exactOptional(),
    context: contextUseSchema.nullable().exactOptional(),
    kind: z.literal("turn_completed"),
    stop: turnStopSchema.nullable().exactOptional(),
    turn_id: turnIdSchema,
    undo: turnUndoInfoSchema.nullable().exactOptional(),
    usage: usageSchema.nullable().exactOptional(),
  }),
  z.object({
    changes: fileChangesSchema.nullable().exactOptional(),
    context: contextUseSchema.nullable().exactOptional(),
    error: errorBodySchema,
    kind: z.literal("turn_failed"),
    turn_id: turnIdSchema,
    undo: turnUndoInfoSchema.nullable().exactOptional(),
    usage: usageSchema.nullable().exactOptional(),
  }),
  z.object({ kind: z.literal("turn_cancelled"), turn_id: turnIdSchema }),
  z.object({
    cwd: z.string(),
    kind: z.literal("shell_started"),
    pid: z.uint32().nullable().exactOptional(),
    pty_id: ptyIdSchema,
  }),
  z.object({
    exit_code: z.int32().nullable().exactOptional(),
    kind: z.literal("shell_exited"),
    pty_id: ptyIdSchema,
  }),
  z.object({
    cwd: z.string(),
    host: z.string().nullable().exactOptional(),
    kind: z.literal("cwd_changed"),
    pty_id: ptyIdSchema,
  }),
  z.object({ kind: z.literal("login_completed"), provider: z.string() }),
  z.object({ kind: z.literal("logout_completed"), provider: z.string() }),
  z.object({
    call_id: callIdSchema,
    grants: z.array(grantSchema).exactOptional(),
    kind: z.literal("exit_requested"),
    kinds: z.array(exitKindSchema),
    record: exitRecordSchema,
    source: exitSourceSchema,
    turn_id: turnIdSchema,
  }),
  z.object({
    cached: z.boolean().exactOptional(),
    call_id: callIdSchema,
    category: z.string().nullable().exactOptional(),
    chain_step: chainStepSchema.nullable().exactOptional(),
    judge: judgeKindSchema,
    kind: z.literal("exit_judged"),
    latency_ms: z.int().nonnegative().nullable().exactOptional(),
    model: z.string().nullable().exactOptional(),
    rationale: z.string().nullable().exactOptional(),
    reasked: z.boolean().exactOptional(),
    record_sha256: z.string().nullable().exactOptional(),
    risk: riskSchema.nullable().exactOptional(),
    skipped: z.array(z.string()).exactOptional(),
    turn_id: turnIdSchema,
    user_authorization: userAuthorizationSchema.nullable().exactOptional(),
    verdict: verdictSchema,
  }),
  z.object({
    call_id: callIdSchema,
    kind: z.literal("always_rule_saved"),
    outcome: alwaysSavedSchema,
    turn_id: turnIdSchema,
  }),
  z.object({
    call_id: callIdSchema,
    kind: z.literal("exit_review_started"),
    kinds: z.array(exitKindSchema),
    turn_id: turnIdSchema,
  }),
  z.object({
    denials: z.int().min(0).max(255),
    kind: z.literal("classifier_paused"),
    reason: pauseReasonSchema,
    turn_id: turnIdSchema,
  }),
  z.object({
    conversation_id: conversationIdSchema,
    kind: z.literal("presence_changed"),
    presence: presenceSchema,
    source: presenceSourceSchema,
  }),
  z.object({
    call_id: callIdSchema,
    freed_bytes: z.int().nonnegative(),
    kind: z.literal("cache_delete_applied"),
    paths: z.array(z.string()),
    turn_id: turnIdSchema,
  }),
  z.object({
    backup: z.string().nullable().exactOptional(),
    call_id: callIdSchema,
    covered: z.boolean().exactOptional(),
    floor: z.boolean(),
    kind: z.literal("home_file_changed"),
    path: z.string(),
    real: z.string().nullable().exactOptional(),
    turn_id: turnIdSchema,
  }),
  z.object({
    call_id: callIdSchema,
    changes: z.array(surfaceChangeSchema),
    kind: z.literal("sandbox_surface_changed"),
    quarantined: z.boolean(),
    turn_id: turnIdSchema,
  }),
  z.object({
    call_id: callIdSchema,
    changes: z.array(surfaceChangeSchema),
    kind: z.literal("surface_question_requested"),
    question_id: questionIdSchema,
    turn_id: turnIdSchema,
  }),
  z.object({
    keep: z.boolean(),
    kind: z.literal("surface_question_answered"),
    origin: originSchema.nullable().exactOptional(),
    question_id: questionIdSchema,
    turn_id: turnIdSchema,
  }),
  z.object({
    files: z.array(reportedFileSchema),
    kind: z.literal("turn_surface_report"),
    turn_id: turnIdSchema,
  }),
  compactionSchema.extend({ kind: z.literal("conversation_compacted") }),
  z.object({ kind: z.literal("sandbox_unavailable"), reason: z.string() }),
  z.object({
    commit: z.string(),
    files: z.uint32(),
    kind: z.literal("snapshot_taken"),
    ms: z.int().nonnegative(),
    phase: snapshotPhaseSchema,
    root: z.string(),
    skipped: z.uint32(),
    turn_id: turnIdSchema,
  }),
  z.object({
    conflicts: z.uint32(),
    conversation_id: conversationIdSchema,
    deleted: z.uint32(),
    kind: z.literal("undo_applied"),
    not_covered: z.uint32(),
    origin: originSchema,
    redo: z.boolean().exactOptional(),
    refs: z.array(refChangeSchema).exactOptional(),
    restored: z.uint32(),
    stopped: z.boolean().exactOptional(),
    trash: z.string().nullable().exactOptional(),
    trashed: z.uint32(),
    undo_id: undoIdSchema,
    undone_turn_id: turnIdSchema,
  }),
  z.object({
    call_id: callIdSchema,
    job: jobInfoSchema,
    kind: z.literal("job_started"),
    launcher_pid: z.uint32(),
    launcher_start: z.int().nonnegative(),
    turn_id: turnIdSchema,
  }),
  z.object({
    job_id: jobIdSchema,
    kind: z.literal("job_watch_matched"),
    line: z.string(),
    wakes_left: z.int().min(0).max(255),
  }),
  z.object({
    by_job: jobIdSchema.nullable().exactOptional(),
    job_id: jobIdSchema,
    kind: z.literal("job_port_forwarded"),
    listen: z.string(),
    outcome: forwardOutcomeSchema,
    port: z.int().min(0).max(65535),
    reason: z.string().nullable().exactOptional(),
  }),
  z.object({
    blocked: z.array(blockedSchema).exactOptional(),
    blocked_more: z.uint32().exactOptional(),
    bus_refused: z.array(busRefusalSchema).exactOptional(),
    bus_refused_more: z.uint32().exactOptional(),
    exit_code: z.int32().nullable().exactOptional(),
    job_id: jobIdSchema,
    kind: z.literal("job_ended"),
    launcher_lost: z.boolean().exactOptional(),
    output_bytes: z.int().nonnegative(),
    output_truncated: z.boolean().exactOptional(),
    quarantined: z.boolean().exactOptional(),
    ran_secs: z.int().nonnegative().exactOptional(),
    state: jobStateSchema,
    stopped_by: jobStopBySchema.nullable().exactOptional(),
    surface_changes: z.array(surfaceChangeSchema).exactOptional(),
    survivors: z.array(z.string()).exactOptional(),
    tail: z.string(),
  }),
  z.object({
    conversation_id: conversationIdSchema,
    job_id: jobIdSchema,
    kind: z.literal("job_notice"),
    not_user_input: z.boolean(),
    notice_kind: noticeKindSchema,
    source_seq: seqSchema,
    text: z.string(),
    turn_id: turnIdSchema,
    woke: z.boolean(),
  }),
  z.object({
    job_id: jobIdSchema,
    kind: z.literal("job_notice_held"),
    reason: heldReasonSchema,
    source_seq: seqSchema,
  }),
  z.object({
    approved_by: approvedBySchema.nullable().exactOptional(),
    call_id: callIdSchema,
    command: z.string(),
    contained: z.boolean(),
    input: inputWaitSchema,
    kind: z.literal("shell_command_detached"),
    looks_secret: z.boolean().exactOptional(),
    prompt: z.string().exactOptional(),
    since: z.iso.datetime({ offset: true }),
  }),
  z.object({
    call_id: callIdSchema,
    contained: z.boolean(),
    input: inputWaitSchema,
    kind: z.literal("shell_command_input_changed"),
    looks_secret: z.boolean().exactOptional(),
    prompt: z.string().exactOptional(),
  }),
  z.object({
    call_id: callIdSchema,
    exit_code: z.int32().nullable().exactOptional(),
    how: shellCommandEndSchema,
    kind: z.literal("shell_command_ended"),
  }),
  z.object({ kind: z.literal("conversation_deleted"), origin: originSchema }),
]) satisfies z.ZodType<Event>;

export const eventEnvelopeSchema = z.object({
  at: z.iso.datetime({ offset: true }),
  conversation_id: conversationIdSchema.nullable().exactOptional(),
  event: eventSchema,
  seq: seqSchema,
}) satisfies z.ZodType<EventEnvelope>;

export const conversationHistoryResultSchema = z.object({
  events: z.array(eventEnvelopeSchema),
  next_cursor: pageCursorSchema.nullable().exactOptional(),
}) satisfies z.ZodType<ConversationHistoryResult>;

export const routeLabelSchema = z.object({
  dir: z.string(),
  name: z.string().nullable().exactOptional(),
  scope: routeScopeSchema,
}) satisfies z.ZodType<RouteLabel>;

export const conversationResumeResultSchema = z.discriminatedUnion("kind", [
  z.object({
    continues_here: z.boolean().exactOptional(),
    conversation_id: conversationIdSchema,
    here: z.boolean().exactOptional(),
    kind: z.literal("resumed"),
    scope: routeLabelSchema,
    shell_dir: z.string().nullable().exactOptional(),
    title: z.string().nullable().exactOptional(),
    updated_at: z.iso.datetime({ offset: true }),
  }),
  z.object({
    kind: z.literal("held"),
    title: z.string().nullable().exactOptional(),
    tty: z.string(),
  }),
  z.object({ kind: z.literal("unknown") }),
]) satisfies z.ZodType<ConversationResumeResult>;

export const searchKindSchema = z.enum([
  "title",
  "prompt",
  "answer",
  "command",
  "unknown",
]) satisfies z.ZodType<SearchKind>;

export const textRangeSchema = z.object({
  end: z.uint32(),
  start: z.uint32(),
}) satisfies z.ZodType<TextRange>;

export const searchLineSchema = z.object({
  ranges: z.array(textRangeSchema).exactOptional(),
  text: z.string(),
}) satisfies z.ZodType<SearchLine>;

export const searchHitSchema = z.object({
  at: z.iso.datetime({ offset: true }),
  history_cursor: pageCursorSchema,
  kind: searchKindSchema,
  lines: z.array(searchLineSchema),
  seq: seqSchema,
  turn_id: turnIdSchema.nullable().exactOptional(),
}) satisfies z.ZodType<SearchHit>;

export const searchPartialSchema = z.enum([
  "row_cap",
  "time_cap",
  "unknown",
]) satisfies z.ZodType<SearchPartial>;

export const conversationSearchResultSchema = z.object({
  hits: z.array(searchHitSchema),
  partial: searchPartialSchema.nullable().exactOptional(),
  title: searchLineSchema.nullable().exactOptional(),
}) satisfies z.ZodType<ConversationSearchResult>;

export const conversationStatusSchema = z.enum([
  "idle",
  "running",
  "awaiting_approval",
]) satisfies z.ZodType<ConversationStatus>;

export const conversationSummarySchema = z.object({
  created_at: z.iso.datetime({ offset: true }),
  cwd: z.string().nullable().exactOptional(),
  id: conversationIdSchema,
  last_seq: seqSchema,
  scope: scopeSchema.nullable().exactOptional(),
  status: conversationStatusSchema,
  title: z.string().nullable().exactOptional(),
  tty: z.string().nullable().exactOptional(),
  updated_at: z.iso.datetime({ offset: true }),
}) satisfies z.ZodType<ConversationSummary>;

export const liveTurnSchema = z.object({
  after_seq: seqSchema,
  index: z.uint32(),
  reasoning: z.string().nullable().exactOptional(),
  reasoning_title: z.string().nullable().exactOptional(),
  text: z.string(),
  text_bytes: z.int().nonnegative(),
  turn_id: turnIdSchema,
}) satisfies z.ZodType<LiveTurn>;

export const conversationSnapshotSchema = z.object({
  conversation: conversationSummarySchema,
  events: z.array(eventEnvelopeSchema),
  history_cursor: pageCursorSchema.nullable().exactOptional(),
  hwm: seqSchema,
  live: liveTurnSchema.nullable().exactOptional(),
}) satisfies z.ZodType<ConversationSnapshot>;

export const draftPartSchema = z.discriminatedUnion("kind", [
  z.object({
    delta: z.string(),
    index: z.uint32(),
    kind: z.literal("text"),
    offset: z.int().nonnegative(),
  }),
  z.object({
    delta: z.string(),
    kind: z.literal("reasoning"),
    offset: z.int().nonnegative(),
    title: z.string().nullable().exactOptional(),
  }),
  z.object({
    bytes: z.int().nonnegative(),
    call: z.uint32(),
    kind: z.literal("tool_input"),
    tool: z.string(),
  }),
  contextUseSchema.extend({ kind: z.literal("context") }),
  z.object({ kind: z.literal("compacting"), trigger: compactionTriggerSchema }),
]) satisfies z.ZodType<DraftPart>;

export const draftSchema = z.object({
  after_seq: seqSchema,
  draft: draftPartSchema,
  turn_id: turnIdSchema,
}) satisfies z.ZodType<Draft>;

export const conversationSubscribeItemSchema = z.discriminatedUnion("kind", [
  eventEnvelopeSchema.extend({ kind: z.literal("event") }),
  conversationSnapshotSchema.extend({ kind: z.literal("snapshot") }),
  draftSchema.extend({ kind: z.literal("draft") }),
]) satisfies z.ZodType<ConversationSubscribeItem>;

export const conversationsListResultSchema = z.object({
  conversations: z.array(conversationSummarySchema),
  next_cursor: pageCursorSchema.nullable().exactOptional(),
}) satisfies z.ZodType<ConversationsListResult>;

export const recentConversationSchema = z.object({
  here: z.boolean().exactOptional(),
  open_in: z.string().nullable().exactOptional(),
  project: z.string().nullable().exactOptional(),
  route: routeScopeSchema,
  summary: conversationSummarySchema,
  turns: z.uint32(),
}) satisfies z.ZodType<RecentConversation>;

export const conversationsRecentResultSchema = z.object({
  conversations: z.array(recentConversationSchema),
  current: conversationIdSchema.nullable().exactOptional(),
  scope: routeLabelSchema.nullable().exactOptional(),
}) satisfies z.ZodType<ConversationsRecentResult>;

export const searchIndexStateSchema = z.object({
  percent: z.int().min(0).max(255),
}) satisfies z.ZodType<SearchIndexState>;

export const searchRowSchema = z.object({
  conversation: recentConversationSchema,
  hit: searchHitSchema.nullable().exactOptional(),
  short_id: z.string(),
  title_ranges: z.array(textRangeSchema).exactOptional(),
}) satisfies z.ZodType<SearchRow>;

export const searchStatsSchema = z.object({
  conversations_scored: z.uint32(),
  elapsed_us: z.int().nonnegative(),
  rows_scanned: z.uint32(),
  titles_scored: z.uint32(),
}) satisfies z.ZodType<SearchStats>;

export const conversationsSearchResultSchema = z.object({
  current: conversationIdSchema.nullable().exactOptional(),
  index: searchIndexStateSchema.nullable().exactOptional(),
  partial: searchPartialSchema.nullable().exactOptional(),
  rows: z.array(searchRowSchema),
  scope: routeLabelSchema.nullable().exactOptional(),
  stats: searchStatsSchema.nullable().exactOptional(),
}) satisfies z.ZodType<ConversationsSearchResult>;

export const cursorSchema = z.object({
  col: z.int().min(0).max(65535),
  hidden: z.boolean().exactOptional(),
  row: z.int().min(0).max(65535),
}) satisfies z.ZodType<Cursor>;

export const daemonPathsSchema = z.object({
  data_dir: z.string(),
  scratch_root: z.string(),
}) satisfies z.ZodType<DaemonPaths>;

export const scopeNameSchema = z.enum([
  "read",
  "operate",
  "approve",
  "terminal",
  "admin",
]) satisfies z.ZodType<ScopeName>;

export const deviceEnrollResultSchema = z.object({
  device_id: deviceIdSchema,
  scopes: z.array(scopeNameSchema),
}) satisfies z.ZodType<DeviceEnrollResult>;

export const deviceInfoSchema = z.object({
  created_at: z.iso.datetime({ offset: true }),
  device_id: deviceIdSchema,
  key_fingerprint: z.string(),
  last_seen_at: z.iso.datetime({ offset: true }).nullable().exactOptional(),
  name: z.string(),
  node: z.string(),
  revoked_at: z.iso.datetime({ offset: true }).nullable().exactOptional(),
  scopes: z.array(scopeNameSchema),
}) satisfies z.ZodType<DeviceInfo>;

export const deviceListResultSchema = z.object({
  devices: z.array(deviceInfoSchema),
}) satisfies z.ZodType<DeviceListResult>;

export const devicePairConfirmResultSchema = z.object({
  device_id: deviceIdSchema.nullable().exactOptional(),
}) satisfies z.ZodType<DevicePairConfirmResult>;

export const pairEndSchema = z.enum([
  "refused",
  "expired",
  "attempts",
  "unknown",
]) satisfies z.ZodType<PairEnd>;

export const pairingQrSchema = z.object({
  daemon_id: daemonIdSchema,
  fqdn: z.string().nullable().exactOptional(),
  host: z.ipv4(),
  port: z.int().min(0).max(65535),
  token: secretTextSchema,
}) satisfies z.ZodType<PairingQr>;

export const devicePairStartItemSchema = z.discriminatedUnion("kind", [
  z.object({
    expires_at: z.iso.datetime({ offset: true }),
    kind: z.literal("started"),
    pairing_id: pairingIdSchema,
    qr: pairingQrSchema,
  }),
  z.object({
    device_name: z.string(),
    key_fingerprint: z.string(),
    kind: z.literal("pending"),
    node_name: z.string(),
    os: z.string().nullable().exactOptional(),
    pairing_id: pairingIdSchema,
    user_login: z.string(),
  }),
  z.object({ device_id: deviceIdSchema, kind: z.literal("paired"), pairing_id: pairingIdSchema }),
  z.object({ kind: z.literal("ended"), pairing_id: pairingIdSchema, reason: pairEndSchema }),
]) satisfies z.ZodType<DevicePairStartItem>;

export const deviceProveResultSchema = z.object({
  scopes: z.array(scopeNameSchema),
}) satisfies z.ZodType<DeviceProveResult>;

export const deviceRevokeResultSchema = z.object({
  revoked_at: z.iso.datetime({ offset: true }),
}) satisfies z.ZodType<DeviceRevokeResult>;

export const errorFrameSchema = z.object({
  error: errorBodySchema,
  id: requestIdSchema.nullable().exactOptional(),
}) satisfies z.ZodType<ErrorFrame>;

export const proveContextSchema = z.object({
  daemon_node: z.string(),
  daemon_tailnet: z.string(),
  peer_node: z.string(),
}) satisfies z.ZodType<ProveContext>;

export const helloResultSchema = z.object({
  capabilities: capabilitiesSchema.exactOptional(),
  challenge: z.string(),
  daemon_id: daemonIdSchema,
  paths: daemonPathsSchema,
  protocol: z.uint32(),
  prove: proveContextSchema.nullable().exactOptional(),
  version: z.string(),
}) satisfies z.ZodType<HelloResult>;

export const inputRespondResultSchema = z.object({}) satisfies z.ZodType<InputRespondResult>;

export const jobsListResultSchema = z.object({
  jobs: z.array(jobInfoSchema).exactOptional(),
}) satisfies z.ZodType<JobsListResult>;

export const jobsStopResultSchema = z.object({
  job: jobInfoSchema,
}) satisfies z.ZodType<JobsStopResult>;

export const leaseReportResultSchema = z.object({
  ttl_secs: z.uint32(),
}) satisfies z.ZodType<LeaseReportResult>;

export const methodNameSchema = z.enum([
  "hello",
  "conversations.list",
  "conversation.subscribe",
  "conversation.history",
  "prompt.send",
  "turn.interrupt",
  "turn.steer",
  "approval.respond",
  "pty.attach",
  "pty.write",
  "pty.resize",
  "input.respond",
  "lease.report",
  "models.list",
  "projects.list",
  "admin.project_add",
  "admin.project_remove",
  "admin.status",
  "admin.config_reload",
  "admin.login_openai",
  "admin.login_api_key",
  "admin.logout",
  "sandbox.explain",
  "sandbox.surface_respond",
  "admin.sandbox_check",
  "conversation.diff",
  "prompt.withdraw",
  "conversation.compact",
  "presence.set",
  "turn.undo",
  "jobs.list",
  "jobs.stop",
  "conversation.context",
  "conversations.recent",
  "conversation.resume",
  "shell.interrupt",
  "shell.status",
  "call.background",
  "conversations.search",
  "conversation.search",
  "conversation.delete",
  "device.pair_start",
  "device.pair_confirm",
  "device.enroll",
  "device.prove",
  "device.list",
  "device.revoke",
]) satisfies z.ZodType<MethodName>;

export const methodParamsSchema = z.strictObject({
  "admin.config_reload": adminConfigReloadSchema,
  "admin.login_api_key": adminLoginApiKeySchema,
  "admin.login_openai": adminLoginOpenAiSchema,
  "admin.logout": adminLogoutSchema,
  "admin.project_add": adminProjectAddSchema,
  "admin.project_remove": adminProjectRemoveSchema,
  "admin.sandbox_check": adminSandboxCheckSchema,
  "admin.status": adminStatusSchema,
  "approval.respond": approvalRespondSchema,
  "call.background": callBackgroundSchema,
  "conversation.compact": conversationCompactSchema,
  "conversation.context": conversationContextSchema,
  "conversation.delete": conversationDeleteSchema,
  "conversation.diff": conversationDiffSchema,
  "conversation.history": conversationHistorySchema,
  "conversation.resume": conversationResumeSchema,
  "conversation.search": conversationSearchSchema,
  "conversation.subscribe": conversationSubscribeSchema,
  "conversations.list": conversationsListSchema,
  "conversations.recent": conversationsRecentSchema,
  "conversations.search": conversationsSearchSchema,
  "device.enroll": deviceEnrollSchema,
  "device.list": deviceListSchema,
  "device.pair_confirm": devicePairConfirmSchema,
  "device.pair_start": devicePairStartSchema,
  "device.prove": deviceProveSchema,
  "device.revoke": deviceRevokeSchema,
  hello: helloSchema,
  "input.respond": inputRespondSchema,
  "jobs.list": jobsListSchema,
  "jobs.stop": jobsStopSchema,
  "lease.report": leaseReportSchema,
  "models.list": modelsListSchema,
  "presence.set": presenceSetSchema,
  "projects.list": projectsListSchema,
  "prompt.send": promptSendSchema,
  "prompt.withdraw": promptWithdrawSchema,
  "pty.attach": ptyAttachSchema,
  "pty.resize": ptyResizeSchema,
  "pty.write": ptyWriteSchema,
  "sandbox.explain": sandboxExplainSchema,
  "sandbox.surface_respond": sandboxSurfaceRespondSchema,
  "shell.interrupt": shellInterruptSchema,
  "shell.status": shellStatusSchema,
  "turn.interrupt": turnInterruptSchema,
  "turn.steer": turnSteerSchema,
  "turn.undo": turnUndoSchema,
}) satisfies z.ZodType<MethodParams>;

export const modelSourceSchema = z.enum(["builtin", "config"]) satisfies z.ZodType<ModelSource>;

export const modelInfoSchema = z.object({
  context_window: z.int().nonnegative().nullable().exactOptional(),
  default: z.boolean().exactOptional(),
  default_effort: z.string().nullable().exactOptional(),
  efforts: z.array(z.string()).exactOptional(),
  id: z.string(),
  max_context_window: z.int().nonnegative().nullable().exactOptional(),
  prefer_websockets: z.boolean().exactOptional(),
  source: modelSourceSchema,
}) satisfies z.ZodType<ModelInfo>;

export const modelsListResultSchema = z.object({
  catalog: catalogStatusSchema.nullable().exactOptional(),
  models: z.array(modelInfoSchema),
}) satisfies z.ZodType<ModelsListResult>;

export const presenceSetResultSchema = z.object({
  changed: z.array(conversationIdSchema).exactOptional(),
  presence: presenceSchema,
  seq: seqSchema.nullable().exactOptional(),
}) satisfies z.ZodType<PresenceSetResult>;

export const projectsListResultSchema = z.object({
  file: z.string(),
  projects: z.array(projectInfoSchema),
}) satisfies z.ZodType<ProjectsListResult>;

export const repoHintSchema = z.object({ root: z.string() }) satisfies z.ZodType<RepoHint>;

export const newReasonSchema = z.discriminatedUnion("kind", [
  z.object({ kind: z.literal("scope") }),
  z.object({ hours: z.int().nonnegative(), kind: z.literal("idle") }),
  z.object({
    kind: z.literal("moved"),
    title: z.string().nullable().exactOptional(),
    tty: z.string(),
  }),
  z.object({ kind: z.literal("unknown") }),
]) satisfies z.ZodType<NewReason>;

export const switchSchema = z.discriminatedUnion("kind", [
  z.object({
    kind: z.literal("back"),
    title: z.string().nullable().exactOptional(),
    updated_at: z.iso.datetime({ offset: true }),
  }),
  z.object({
    kind: z.literal("picked_up"),
    title: z.string().nullable().exactOptional(),
    updated_at: z.iso.datetime({ offset: true }),
  }),
  z.object({
    from: routeLabelSchema.nullable().exactOptional(),
    kind: z.literal("new"),
    reason: newReasonSchema,
  }),
  z.object({ kind: z.literal("unknown") }),
]) satisfies z.ZodType<Switch>;

export const routedSchema = z.object({
  hint: repoHintSchema.nullable().exactOptional(),
  scope: routeLabelSchema,
  switch: switchSchema.nullable().exactOptional(),
}) satisfies z.ZodType<Routed>;

export const promptSendResultSchema = z.object({
  conversation_id: conversationIdSchema,
  queued: z.boolean(),
  route: routedSchema.nullable().exactOptional(),
  seq: seqSchema,
  settings: effectiveSettingsSchema.nullable().exactOptional(),
  turn_id: turnIdSchema,
}) satisfies z.ZodType<PromptSendResult>;

export const withdrawnPromptSchema = z.object({
  seq: seqSchema,
  text: z.string(),
  turn_id: turnIdSchema,
}) satisfies z.ZodType<WithdrawnPrompt>;

export const promptWithdrawResultSchema = z.object({
  withdrawn: withdrawnPromptSchema,
}) satisfies z.ZodType<PromptWithdrawResult>;

export const rowCellsSchema = z.object({
  cells: z.array(cellSchema),
  wrapped: z.boolean().exactOptional(),
}) satisfies z.ZodType<RowCells>;

export const screenSnapshotSchema = z.object({
  alternate_screen: z.boolean().exactOptional(),
  cursor: cursorSchema,
  rows: z.array(rowCellsSchema),
  scrollback: z.array(rowCellsSchema).exactOptional(),
  size: sizeSchema,
  title: z.string().nullable().exactOptional(),
}) satisfies z.ZodType<ScreenSnapshot>;

export const ptyAttachItemSchema = z.discriminatedUnion("kind", [
  z.object({ kind: z.literal("snapshot"), seq: seqSchema, snapshot: screenSnapshotSchema }),
  z.object({ data: base64BytesSchema, kind: z.literal("output"), seq: seqSchema }),
  z.object({ kind: z.literal("resized"), seq: seqSchema, size: sizeSchema }),
]) satisfies z.ZodType<PtyAttachItem>;

export const ptyResizeResultSchema = z.object({
  size: sizeSchema,
}) satisfies z.ZodType<PtyResizeResult>;

export const ptyWriteResultSchema = z.object({}) satisfies z.ZodType<PtyWriteResult>;

export const sandboxPathRoleSchema = z.enum([
  "write_root",
  "cache_overlay",
  "private_tmp",
  "read_only",
  "masked",
  "floor",
  "granted",
]) satisfies z.ZodType<SandboxPathRole>;

export const sandboxExplainResultSchema = z.object({
  mode: modeSchema,
  path: z.string(),
  project: z.string().nullable().exactOptional(),
  read: z.boolean(),
  reason: z.string(),
  role: sandboxPathRoleSchema,
  write: z.boolean(),
  write_exit: exitKindSchema.nullable().exactOptional(),
}) satisfies z.ZodType<SandboxExplainResult>;

export const sandboxSurfaceRespondResultSchema = z.object({
  seq: seqSchema,
}) satisfies z.ZodType<SandboxSurfaceRespondResult>;

export const shellInterruptResultSchema = z.object({}) satisfies z.ZodType<ShellInterruptResult>;

export const shellStatusResultSchema = z.object({
  busy: busyShellSchema.nullable().exactOptional(),
}) satisfies z.ZodType<ShellStatusResult>;

export const resentSteersSchema = z.object({
  seq: seqSchema,
  steers: z.array(seqSchema),
  turn_id: turnIdSchema,
}) satisfies z.ZodType<ResentSteers>;

export const withdrawnSteerSchema = z.object({
  seq: seqSchema,
  text: z.string(),
}) satisfies z.ZodType<WithdrawnSteer>;

export const turnInterruptResultSchema = z.object({
  resent: resentSteersSchema.nullable().exactOptional(),
  seq: seqSchema,
  turn_id: turnIdSchema,
  withdrawn: z.array(withdrawnPromptSchema).exactOptional(),
  withdrawn_steers: z.array(withdrawnSteerSchema).exactOptional(),
}) satisfies z.ZodType<TurnInterruptResult>;

export const turnSteerResultSchema = z.object({
  queued: z.boolean().exactOptional(),
  seq: seqSchema,
  turn_id: turnIdSchema,
}) satisfies z.ZodType<TurnSteerResult>;

export const undoActionSchema = z.enum([
  "restore",
  "trash",
  "delete",
  "skip",
  "not_covered",
  "command",
]) satisfies z.ZodType<UndoAction>;

export const undoEntrySchema = z.object({
  action: undoActionSchema,
  path: z.string(),
  reason: undoReasonSchema.nullable().exactOptional(),
}) satisfies z.ZodType<UndoEntry>;

export const undoableTurnSchema = z.object({
  auto: z.boolean().exactOptional(),
  ended: z.iso.datetime({ offset: true }).nullable().exactOptional(),
  full: z.boolean().exactOptional(),
  stopped: z.boolean().exactOptional(),
  title: z.string().nullable().exactOptional(),
  turn_id: turnIdSchema,
  undone: z.boolean().exactOptional(),
}) satisfies z.ZodType<UndoableTurn>;

export const turnUndoResultSchema = z.object({
  applied: z.boolean().exactOptional(),
  conversation_id: conversationIdSchema,
  entries: z.array(undoEntrySchema).exactOptional(),
  entries_more: z.uint32().exactOptional(),
  notes: z.array(z.string()).exactOptional(),
  refs: z.array(refChangeSchema).exactOptional(),
  resumed: z.boolean().exactOptional(),
  stopped: z.boolean().exactOptional(),
  title: z.string().nullable().exactOptional(),
  trash: z.string().nullable().exactOptional(),
  turn_id: turnIdSchema.nullable().exactOptional(),
  turns: z.array(undoableTurnSchema).exactOptional(),
  undo_id: undoIdSchema.nullable().exactOptional(),
}) satisfies z.ZodType<TurnUndoResult>;

export const methodResultsSchema = z.strictObject({
  "admin.config_reload": adminConfigReloadResultSchema,
  "admin.login_api_key": adminLoginApiKeyResultSchema,
  "admin.login_openai": adminLoginOpenAiItemSchema,
  "admin.logout": adminLogoutResultSchema,
  "admin.project_add": adminProjectAddResultSchema,
  "admin.project_remove": adminProjectRemoveResultSchema,
  "admin.sandbox_check": adminSandboxCheckResultSchema,
  "admin.status": adminStatusResultSchema,
  "approval.respond": approvalRespondResultSchema,
  "call.background": callBackgroundResultSchema,
  "conversation.compact": conversationCompactResultSchema,
  "conversation.context": conversationContextResultSchema,
  "conversation.delete": conversationDeleteResultSchema,
  "conversation.diff": conversationDiffResultSchema,
  "conversation.history": conversationHistoryResultSchema,
  "conversation.resume": conversationResumeResultSchema,
  "conversation.search": conversationSearchResultSchema,
  "conversation.subscribe": conversationSubscribeItemSchema,
  "conversations.list": conversationsListResultSchema,
  "conversations.recent": conversationsRecentResultSchema,
  "conversations.search": conversationsSearchResultSchema,
  "device.enroll": deviceEnrollResultSchema,
  "device.list": deviceListResultSchema,
  "device.pair_confirm": devicePairConfirmResultSchema,
  "device.pair_start": devicePairStartItemSchema,
  "device.prove": deviceProveResultSchema,
  "device.revoke": deviceRevokeResultSchema,
  hello: helloResultSchema,
  "input.respond": inputRespondResultSchema,
  "jobs.list": jobsListResultSchema,
  "jobs.stop": jobsStopResultSchema,
  "lease.report": leaseReportResultSchema,
  "models.list": modelsListResultSchema,
  "presence.set": presenceSetResultSchema,
  "projects.list": projectsListResultSchema,
  "prompt.send": promptSendResultSchema,
  "prompt.withdraw": promptWithdrawResultSchema,
  "pty.attach": ptyAttachItemSchema,
  "pty.resize": ptyResizeResultSchema,
  "pty.write": ptyWriteResultSchema,
  "sandbox.explain": sandboxExplainResultSchema,
  "sandbox.surface_respond": sandboxSurfaceRespondResultSchema,
  "shell.interrupt": shellInterruptResultSchema,
  "shell.status": shellStatusResultSchema,
  "turn.interrupt": turnInterruptResultSchema,
  "turn.steer": turnSteerResultSchema,
  "turn.undo": turnUndoResultSchema,
}) satisfies z.ZodType<MethodResults>;

export const protocolVersionSchema = z.literal(1) satisfies z.ZodType<ProtocolVersion>;

export const serverAckFrameSchema = z.strictObject({
  ack: requestIdSchema,
}) satisfies z.ZodType<ServerAckFrame>;

export const serverEndFrameSchema = z.strictObject({
  end: z.literal(true),
  id: requestIdSchema,
}) satisfies z.ZodType<ServerEndFrame>;

export const serverItemFrameSchema = z.strictObject({
  id: requestIdSchema,
  item: z.unknown(),
}) satisfies z.ZodType<ServerItemFrame>;

export const serverFrameSchema = z.union([
  serverItemFrameSchema,
  serverEndFrameSchema,
  errorFrameSchema,
  serverAckFrameSchema,
]) satisfies z.ZodType<ServerFrame>;

export const streamMethodNameSchema = z.enum([
  "conversation.subscribe",
  "pty.attach",
  "admin.login_openai",
  "device.pair_start",
]) satisfies z.ZodType<StreamMethodName>;

/** The schema of the result of each method; for a stream method, of each item. */
export const METHOD_RESULT_SCHEMAS: {
  readonly [M in MethodName]: z.ZodType<MethodResults[M]>;
} = methodResultsSchema.shape;

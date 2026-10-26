// Generated from docs/protocol.schema.json by `pnpm gen:protocol` (scripts/gen-protocol.mjs).
// Do not edit it by hand. Change efr-protocol, run `cargo xtask protocol-schema` at the repo
// top, then run `pnpm gen:protocol` in app/.
//
// Numbers: a u64 of the protocol is a JS number here. It is exact up to 2^53, and the
// envelope guard (src/lib/protocol/envelope.ts) refuses a larger value. Every offset into text
// counts bytes of UTF-8, not UTF-16 units: compare it with utf8Length (src/lib/stream/utf8.ts),
// never with `text.length`.
//
// An event, draft or item kind that this file does not list can come from a newer efrd.
// The envelope guard decodes it to an "other" wrapper, and the app skips it.

/**
 * A frame from a client.
 */
export type ClientFrame = ClientRequest | ClientCancel;
/**
 * Start a request: `id` and a method with its params.
 */
export type ClientRequest = {
  id: RequestId;
} & Method;
/**
 * The id that a client gives a request. It is unique among the requests in flight on
 * one connection; every server frame about the request carries it, and a cancel frame
 * names it.
 *
 * It is a number, not a UUID, because it only has to be unique per connection and a
 * counter is the cheapest way to get that.
 */
export type RequestId = number;
/**
 * A request: the wire method name and its params.
 *
 * On the wire this is the `method` and `params` members of a request frame, such as
 * `{"id": 1, "method": "conversations.list", "params": {}}`. `params` is always an
 * object, even for a method that takes nothing.
 *
 * Deliberately not `#[non_exhaustive]`, unlike every other wire enum: the daemon matches
 * it exhaustively, so a new method cannot ship without a scope and a handler. The
 * matches in this file are exhaustive for the same reason.
 */
export type Method =
  | {
      method: "hello";
      params: Hello;
    }
  | {
      method: "conversations.list";
      params: ConversationsList;
    }
  | {
      method: "conversation.subscribe";
      params: ConversationSubscribe;
    }
  | {
      method: "conversation.history";
      params: ConversationHistory;
    }
  | {
      method: "prompt.send";
      params: PromptSend;
    }
  | {
      method: "turn.interrupt";
      params: TurnInterrupt;
    }
  | {
      method: "turn.steer";
      params: TurnSteer;
    }
  | {
      method: "approval.respond";
      params: ApprovalRespond;
    }
  | {
      method: "pty.attach";
      params: PtyAttach;
    }
  | {
      method: "pty.write";
      params: PtyWrite;
    }
  | {
      method: "pty.resize";
      params: PtyResize;
    }
  | {
      method: "input.respond";
      params: InputRespond;
    }
  | {
      method: "lease.report";
      params: LeaseReport;
    }
  | {
      method: "models.list";
      params: ModelsList;
    }
  | {
      method: "projects.list";
      params: ProjectsList;
    }
  | {
      method: "admin.project_add";
      params: AdminProjectAdd;
    }
  | {
      method: "admin.project_remove";
      params: AdminProjectRemove;
    }
  | {
      method: "admin.status";
      params: AdminStatus;
    }
  | {
      method: "admin.config_reload";
      params: AdminConfigReload;
    }
  | {
      method: "admin.login_openai";
      params: AdminLoginOpenAi;
    }
  | {
      method: "admin.login_api_key";
      params: AdminLoginApiKey;
    }
  | {
      method: "admin.logout";
      params: AdminLogout;
    }
  | {
      method: "sandbox.explain";
      params: SandboxExplain;
    }
  | {
      method: "sandbox.surface_respond";
      params: SandboxSurfaceRespond;
    }
  | {
      method: "admin.sandbox_check";
      params: AdminSandboxCheck;
    }
  | {
      method: "conversation.diff";
      params: ConversationDiff;
    }
  | {
      method: "prompt.withdraw";
      params: PromptWithdraw;
    }
  | {
      method: "conversation.compact";
      params: ConversationCompact;
    }
  | {
      method: "presence.set";
      params: PresenceSet;
    }
  | {
      method: "turn.undo";
      params: TurnUndo;
    }
  | {
      method: "jobs.list";
      params: JobsList;
    }
  | {
      method: "jobs.stop";
      params: JobsStop;
    }
  | {
      method: "conversation.context";
      params: ConversationContext;
    }
  | {
      method: "conversations.recent";
      params: ConversationsRecent;
    }
  | {
      method: "conversation.resume";
      params: ConversationResume;
    }
  | {
      method: "shell.interrupt";
      params: ShellInterrupt;
    }
  | {
      method: "shell.status";
      params: ShellStatus;
    }
  | {
      method: "call.background";
      params: CallBackground;
    }
  | {
      method: "conversations.search";
      params: ConversationsSearch;
    }
  | {
      method: "conversation.search";
      params: ConversationSearch;
    }
  | {
      method: "conversation.delete";
      params: ConversationDelete;
    }
  | {
      method: "device.pair_start";
      params: DevicePairStart;
    }
  | {
      method: "device.pair_confirm";
      params: DevicePairConfirm;
    }
  | {
      method: "device.enroll";
      params: DeviceEnroll;
    }
  | {
      method: "device.prove";
      params: DeviceProve;
    }
  | {
      method: "device.list";
      params: DeviceList;
    }
  | {
      method: "device.revoke";
      params: DeviceRevoke;
    };
/**
 * An enrolled device such as a phone. The Unix socket does not use it.
 */
export type DeviceId = string;
/**
 * The kind of surface that a request or a turn comes from. The permission engine is
 * stricter for some origins: a turn from the phone needs approval for anything outside
 * `$SCRATCH`.
 */
export type Origin = "shell" | "cli" | "proxy" | "phone";
/**
 * An opaque position in a paged list. A client passes it back unchanged to get the
 * next page and never looks inside.
 */
export type PageCursor = string;
/**
 * A position in an ordered stream: the global sequence number of the event log, or the
 * byte offset in a PTY recording.
 *
 * Event sequence numbers start at 1 and only grow, so a client resumes a subscription
 * from the largest one it has seen.
 */
export type Seq = number;
/**
 * One conversation: the unit a `,` line talks to and a subscription follows.
 */
export type ConversationId = string;
/**
 * A client-generated id that makes a write idempotent. The daemon stores a receipt
 * per command id, and a retry with the same id returns the stored result.
 */
export type CommandId = string;
/**
 * The permission mode of a turn: the base policy, which the user's own rules follow and
 * win over where they match.
 *
 * Some floors hold in every mode: a privileged program such as `sudo` asks, secrets
 * are denied unless a user rule names them, and no tool writes efr's config. Full mode
 * keeps these floors and the other questions that `docs/permissions.md` names in its
 * section "Full mode"; the other modes also ask for an interactive call. A turn from a
 * remote origin runs with at most [`Mode::Cautious`].
 *
 * The variants are ordered from the strictest to the loosest, so the remote cap is
 * `mode.min(Mode::Cautious)`.
 */
export type Mode = "manual" | "cautious" | "auto" | "full";
/**
 * What the daemon does with a late steer, instead of refusing it.
 *
 * On the wire an object whose `kind` names the choice, such as `{"kind": "queue"}`.
 */
export type LateSteer = {
  /**
   * The user's shell when the steer was sent, for the prompt.
   */
  context?: ShellContext | null;
  kind: "queue";
  /**
   * The last command line from the user's shell history, for the turn's
   * live-state preamble only; never recorded, as in `prompt.send`.
   */
  last_command?: string | null;
  /**
   * The recent commands of the terminal, oldest first, for the turn's live-state
   * preamble only; never recorded, as in `prompt.send`. Absent means none.
   */
  recent_commands?: RecentCommand[];
  /**
   * The settings that the prompt asks for. Absent means none.
   */
  settings?: TurnSettings;
};
/**
 * One turn of a conversation: a prompt and everything the model does for it.
 */
export type TurnId = string;
/**
 * One tool call inside a turn. The daemon mints it; the provider's own call id stays
 * in the provider's raw items.
 */
export type CallId = string;
/**
 * The user's answer to an approval request.
 */
export type ApprovalDecision = "allow" | "deny" | "allow_always";
/**
 * One pseudo-terminal: a hidden conversation shell or, later, a proxied terminal.
 */
export type PtyId = string;
export type Base64Bytes = string;
/**
 * Text that a user typed and that can be a secret, such as a password typed for `sudo`.
 *
 * On the wire it is a plain JSON string. `Debug` prints a placeholder and never the
 * text, because requests are logged. The value's own buffer is overwritten with zeros
 * when it is dropped, and so is each clone's, which is a buffer of its own. Copies made
 * outside the value are not: [`SecretText::new`] copies text given as a `&str` (a
 * `String` moves in), serde_json unescapes a string that holds an escape (a quote, a
 * backslash, `\u`) in a scratch buffer that it frees without zeroing, and the encoded
 * frames are the business of both sides of `efr-transport`, whose README says which
 * of their buffers they zero. The one place that must use the text reads it with
 * [`SecretText::expose_secret`].
 */
export type SecretText = string;
/**
 * A question to the user that is not an approval of a tool call, such as whether
 * to keep a git setting that a sandboxed call changed.
 */
export type QuestionId = string;
/**
 * Which waiting prompt `prompt.withdraw` takes back.
 *
 * On the wire an object whose `kind` names the choice, such as
 * `{"kind": "turn", "turn_id": "..."}`.
 */
export type WithdrawTarget =
  | {
      kind: "turn";
      /**
       * The turn of the prompt.
       */
      turn_id: TurnId;
    }
  | {
      kind: "newest_from_tty";
      /**
       * The terminal, such as `/dev/pts/3`.
       */
      tty: string;
    };
/**
 * Whether a person is at a conversation.
 */
export type Presence = "present" | "away";
/**
 * The id of a job of one conversation, such as j3.
 */
export type JobId = string;
/**
 * One pairing of a device that `device.pair_start` opened: its QR code, the
 * phone's enroll and the confirmation at the terminal.
 */
export type PairingId = string;
/**
 * What went wrong with a request, as a closed set that every client can handle.
 *
 * The set is closed on the wire: a new code is a breaking change and bumps
 * [`PROTOCOL_VERSION`](crate::PROTOCOL_VERSION). The mapping from the daemon's own
 * errors to these codes lives in one place, `efr-daemon/src/error.rs`.
 */
export type ErrorCode =
  | "protocol_mismatch"
  | "unauthorized"
  | "forbidden"
  | "not_found"
  | "conflict"
  | "busy"
  | "overflow"
  | "cancelled"
  | "invalid"
  | "internal";
/**
 * Something that happened, as the event log records it and subscribers receive it.
 *
 * Variants are named `<Noun><PastParticiple>`. Each one carries the full state of what it
 * reports, never a delta against an earlier event, so a subscriber can resume from any
 * sequence number without reading anything else.
 *
 * On the wire an event is one object whose `kind` member is the snake_case variant
 * name, such as `{"kind": "turn_started", ...}`. A kind that this build does not know
 * decodes as [`Event::Unknown`] and encodes back to the same JSON, so an old reader
 * keeps advancing its cursor past events from a newer daemon. A known kind with a
 * malformed body is an error, not `Unknown`.
 *
 * A new variant needs a fixture in `fixtures/v1/events/`; a test fails until it has one.
 */
export type Event =
  | {
      kind: "conversation_created";
      /**
       * The surface that started it.
       */
      origin: Origin;
      /**
       * The scope key of its first prompt: with `tty`, the terminal's slot that it
       * takes. Absent in conversations from before routing by scope, and when the
       * first prompt had no working directory.
       */
      route?: RouteScope | null;
      /**
       * The terminal it is the active conversation of, when it came from a shell.
       */
      tty?: string | null;
    }
  | {
      /**
       * How it came there.
       */
      how: AttachHow;
      kind: "terminal_attached";
      /**
       * The scope key of the slot.
       */
      route: RouteScope;
      /**
       * The pid of the shell that took it, when it said.
       */
      shell_pid?: number | null;
      /**
       * The terminal.
       */
      tty: string;
    }
  | {
      /**
       * The client's id for the `prompt.send` that carried it.
       */
      command_id: CommandId;
      /**
       * The user's shell when the prompt was sent. A [`ShellContext`] holds no command
       * line, so this event, which the log keeps forever and every subscriber receives,
       * never carries the last command of `prompt.send`.
       */
      context?: ShellContext | null;
      kind: "prompt_queued";
      /**
       * The surface that sent it.
       */
      origin: Origin;
      /**
       * The settings that the prompt asked for, kept with it while it waits, so a held
       * prompt runs with them too. Absent means none.
       */
      settings?: TurnSettings;
      /**
       * The `turn_steered` events whose text this prompt carries, when an interrupt
       * sent unread steers again as a prompt (`turn.interrupt` with
       * `resend_steers`). A client shows those steers as this prompt from now on.
       * Absent means none.
       */
      steers?: Seq[];
      /**
       * The prompt text.
       */
      text: string;
      /**
       * The turn that will answer the prompt.
       */
      turn_id: TurnId;
    }
  | {
      kind: "prompt_withdrawn";
      /**
       * The surface that withdrew it.
       */
      origin: Origin;
      /**
       * The turn of the withdrawn prompt.
       */
      turn_id: TurnId;
    }
  | {
      kind: "prompt_held";
      /**
       * The turn of the held prompt.
       */
      turn_id: TurnId;
    }
  | {
      /**
       * The user's working directory when the prompt was sent.
       */
      cwd: string;
      kind: "turn_started";
      /**
       * The scope derived from that directory.
       */
      scope: Scope;
      /**
       * The settings that the turn runs with until it ends. Absent in turns recorded
       * before turn settings, whose model and effort no event names.
       */
      settings?: EffectiveSettings | null;
      /**
       * The turn.
       */
      turn_id: TurnId;
    }
  | {
      /**
       * The scope before.
       */
      from: Scope;
      kind: "scope_changed";
      /**
       * The scope now.
       */
      to: Scope;
      /**
       * The turn whose working directory produced the new scope.
       */
      turn_id: TurnId;
    }
  | {
      /**
       * The text added at `offset`.
       */
      delta: string;
      /**
       * The position of the message among the turn's assistant messages that have
       * text, from 0. A model answer without text takes no position.
       */
      index: number;
      kind: "assistant_message_updated";
      /**
       * The length of the message's text before `delta`, in bytes of UTF-8: not a
       * count of characters or of UTF-16 units.
       */
      offset: number;
      /**
       * The turn.
       */
      turn_id: TurnId;
    }
  | {
      /**
       * The position of the message among the turn's assistant messages that have
       * text, from 0. A model answer without text takes no position.
       */
      index: number;
      kind: "assistant_message_completed";
      /**
       * The whole text.
       */
      text: string;
      /**
       * The turn.
       */
      turn_id: TurnId;
    }
  | {
      /**
       * The call.
       */
      call_id: CallId;
      /**
       * True when the model wrote the input as plain text for a freeform tool, such
       * as the patch of an `apply_patch` call for an OpenAI model: `input` is then a
       * JSON string with that text, and the next request sends the call back in its
       * freeform form. False when absent: `input` is the JSON that the model wrote,
       * or a JSON string with arguments that were not JSON.
       */
      freeform?: boolean;
      /**
       * The tool's input as the model wrote it. For a call with `freeform`, a JSON
       * string that holds the text.
       */
      input: {
        [k: string]: unknown;
      };
      kind: "tool_call_started";
      /**
       * How a `shell` call runs: typed into the hidden shell, in the `auto` sandbox
       * with its grants, or in the exit child. Absent for other tools and in calls
       * recorded before the sandbox.
       */
      launch?: Launch | null;
      /**
       * True when the call takes an input that the user chooses to type while it
       * reports no wait (`input.respond` with `manual`), as a `shell` call does whose
       * command does not type into a shell that reads command lines. A client offers
       * such an input only for a call with this flag. False when absent.
       */
      manual_input?: boolean;
      /**
       * The tool's registered name, such as `shell` or `read_file`.
       */
      tool: string;
      /**
       * The turn.
       */
      turn_id: TurnId;
    }
  | {
      /**
       * The size of the whole output so far, in bytes.
       */
      bytes: number;
      /**
       * The call.
       */
      call_id: CallId;
      kind: "tool_call_output_updated";
      /**
       * The end of the output so far, bounded in size.
       */
      tail: string;
      /**
       * The turn.
       */
      turn_id: TurnId;
    }
  | {
      /**
       * The call.
       */
      call_id: CallId;
      /**
       * What the call waits for now.
       */
      input: InputWait;
      kind: "tool_call_input_changed";
      /**
       * True for a `visible` wait whose prompt line looks like a password prompt
       * (`password`, `passphrase`, `passcode`, `PIN`, `verification code` or
       * `one-time code`) while the terminal is not in line mode, as behind a relay such
       * as `sudo`'s own pty, `ssh` or `docker exec`. The program on the inner terminal
       * decides whether the answer is shown, so a client hides what the user types
       * and says so; the answer is still sent as a visible one. False when absent.
       */
      looks_secret?: boolean;
      /**
       * The turn.
       */
      turn_id: TurnId;
    }
  | {
      /**
       * The call.
       */
      call_id: CallId;
      /**
       * The files that the call changed: for a file tool, the files it wrote, deleted
       * or moved; for a `shell` call, what changed in the turn's registered project, `$SCRATCH` and,
       * in `auto`, the registered projects that the line names, as efr's own
       * snapshots before and after the call show it. Absent when nothing changed
       * there, for a tool that cannot write, and when efr took no snapshot.
       */
      changes?: FileChanges | null;
      /**
       * The unified diff of a file tool's call: the diff of each file it changed, in
       * the order it changed them, one after the other, each with its two header
       * lines (`/dev/null` for a new or a deleted file), together at most
       * [`MAX_CALL_DIFF_LINES`](crate::MAX_CALL_DIFF_LINES) lines and then a line
       * `... N more lines`. Absent for other tools, for a binary file and when no
       * content changed.
       */
      diff?: string | null;
      /**
       * The exit code, for a tool that runs a command.
       */
      exit_code?: number | null;
      /**
       * True when the tool failed or the call was denied.
       */
      is_error: boolean;
      kind: "tool_call_completed";
      /**
       * True when a `shell` call answered the model while its command still runs in
       * the conversation's hidden shell, such as after its timeout. The shell takes
       * no other call until that command ends; `shell_command_detached` and
       * `shell_command_ended` of the same `call_id` tell more. A client shows that
       * the command still runs, not that it ended. False when absent.
       */
      left_running?: boolean;
      /**
       * The output that the model sees, truncated in the middle when it is long.
       */
      output: string;
      /**
       * Why efr refused the call before it ran, in a few words for a person, such as
       * `efr's config (floor)`. Absent for a call that ran, and for one that the user
       * denied or that no answer reached.
       */
      refusal?: string | null;
      /**
       * What a call through the sandbox's launcher did to the state around it: names
       * only, never values. Absent for a call that did not use the launcher.
       */
      sandbox?: SandboxSummary | null;
      /**
       * True when `output` was truncated; the full output stays in the recording.
       */
      truncated: boolean;
      /**
       * The turn.
       */
      turn_id: TurnId;
    }
  | {
      /**
       * The call that waits.
       */
      call_id: CallId;
      /**
       * A diff of the change, for a call that writes a file.
       */
      diff_preview?: string | null;
      /**
       * What the call would do outside the `auto` sandbox, for an exit. A client that
       * does not know it shows `summary` as before.
       */
      exit?: ExitInfo | null;
      /**
       * How many seconds the question waits before the call does not run, for a
       * question with a fixed limit, such as a delete that full mode guards. Absent:
       * no fixed limit.
       */
      expires_in_secs?: number | null;
      /**
       * True when the call may wait for input at the terminal, such as a `sudo`
       * password: a client whose user approves it may keep the keys typed while it
       * runs for the answer it asks for. False when absent.
       */
      interactive?: boolean;
      kind: "approval_requested";
      /**
       * True when only the user's own machine may answer, as for a privilege exit,
       * a delete that full mode guards or a job's question: the daemon refuses an
       * answer from a phone, and the question stays open. False when absent, also
       * from a daemon that does not announce `approval_local_only`.
       */
      local_only?: boolean;
      /**
       * What the call would do, in one line.
       */
      summary: string;
      /**
       * The turn, which waits for the answer.
       */
      turn_id: TurnId;
    }
  | {
      /**
       * The call.
       */
      call_id: CallId;
      /**
       * The answer.
       */
      decision: ApprovalDecision;
      kind: "approval_resolved";
      /**
       * The surface that answered.
       */
      origin: Origin;
      /**
       * The turn.
       */
      turn_id: TurnId;
    }
  | {
      /**
       * True when nobody answered while the user was away (`presence.away_wait_secs`).
       */
      away?: boolean;
      /**
       * The call.
       */
      call_id: CallId;
      kind: "approval_expired";
      /**
       * The turn.
       */
      turn_id: TurnId;
    }
  | {
      kind: "turn_steered";
      /**
       * The guidance.
       */
      text: string;
      /**
       * The turn.
       */
      turn_id: TurnId;
    }
  | {
      kind: "steering_delivered";
      /**
       * The `turn_steered` events that the call read, in sequence order.
       */
      steers: Seq[];
      /**
       * The turn.
       */
      turn_id: TurnId;
    }
  | {
      kind: "steering_withdrawn";
      /**
       * The surface that took them back.
       */
      origin: Origin;
      /**
       * The `turn_steered` events that the user took back, in sequence order.
       */
      steers: Seq[];
      /**
       * The turn.
       */
      turn_id: TurnId;
    }
  | {
      kind: "turn_interrupt_requested";
      /**
       * The surface that asked.
       */
      origin: Origin;
      /**
       * The turn.
       */
      turn_id: TurnId;
    }
  | {
      /**
       * The files that the turn changed before it stopped, as for
       * [`Event::TurnCompleted`]. Absent when nothing changed, and in turns recorded
       * before efr reported it.
       */
      changes?: FileChanges | null;
      /**
       * How full the model's context was when the turn stopped. Absent in turns
       * recorded before efr counted it.
       */
      context?: ContextUse | null;
      kind: "turn_interrupted";
      /**
       * The turn.
       */
      turn_id: TurnId;
      /**
       * What `efr undo` can take back of the turn. Absent when the turn has no
       * snapshot, and in turns recorded before undo.
       */
      undo?: TurnUndoInfo | null;
      /**
       * The tokens that the turn used before it stopped, when the provider reported
       * them.
       */
      usage?: Usage | null;
    }
  | {
      /**
       * The files that the turn changed: its first snapshot against its last, in the
       * turn's registered project, `$SCRATCH` and the registered projects that its
       * calls wrote. Absent when nothing changed there. `conversation.diff` returns
       * the diff.
       */
      changes?: FileChanges | null;
      /**
       * How full the model's context was when the turn ended. Absent in turns
       * recorded before efr counted it.
       */
      context?: ContextUse | null;
      kind: "turn_completed";
      /**
       * Why the model stopped, when its last call gave no text and no tool call, or
       * stopped for another reason than the end of its answer. Absent for a normal
       * answer, and in turns recorded before efr reported it.
       */
      stop?: TurnStop | null;
      /**
       * The turn.
       */
      turn_id: TurnId;
      /**
       * What `efr undo` can take back of the turn. Absent when the turn has no
       * snapshot, and in turns recorded before undo.
       */
      undo?: TurnUndoInfo | null;
      /**
       * The tokens that the turn used, when the provider reported them.
       */
      usage?: Usage | null;
    }
  | {
      /**
       * The files that the turn changed before it failed, as for
       * [`Event::TurnCompleted`]. Absent when nothing changed, and in turns recorded
       * before efr reported it.
       */
      changes?: FileChanges | null;
      /**
       * How full the model's context was when the turn failed. Absent in turns
       * recorded before efr counted it.
       */
      context?: ContextUse | null;
      /**
       * What went wrong.
       */
      error: ErrorBody;
      kind: "turn_failed";
      /**
       * The turn.
       */
      turn_id: TurnId;
      /**
       * What `efr undo` can take back of the turn. Absent when the turn has no
       * snapshot, and in turns recorded before undo.
       */
      undo?: TurnUndoInfo | null;
      /**
       * The tokens that the turn used before it failed, when the provider reported
       * them.
       */
      usage?: Usage | null;
    }
  | {
      kind: "turn_cancelled";
      /**
       * The turn.
       */
      turn_id: TurnId;
    }
  | {
      /**
       * The directory the shell started in.
       */
      cwd: string;
      kind: "shell_started";
      /**
       * The shell's process id.
       */
      pid?: number | null;
      /**
       * The shell's PTY, which `pty.attach` takes.
       */
      pty_id: PtyId;
    }
  | {
      /**
       * The exit code; absent when a signal ended the shell.
       */
      exit_code?: number | null;
      kind: "shell_exited";
      /**
       * The shell's PTY.
       */
      pty_id: PtyId;
    }
  | {
      /**
       * The new directory.
       */
      cwd: string;
      /**
       * The host in the report, when it named one.
       */
      host?: string | null;
      kind: "cwd_changed";
      /**
       * The shell's PTY.
       */
      pty_id: PtyId;
    }
  | {
      kind: "login_completed";
      /**
       * The provider, such as `openai`.
       */
      provider: string;
    }
  | {
      kind: "logout_completed";
      /**
       * The provider, such as `anthropic-api`.
       */
      provider: string;
    }
  | {
      /**
       * The call.
       */
      call_id: CallId;
      /**
       * Exactly what an approval opens.
       */
      grants?: Grant[];
      kind: "exit_requested";
      /**
       * The exits of the call.
       */
      kinds: ExitKind[];
      /**
       * The facts of the exit. Boxed, because it is much larger than every other
       * event.
       */
      record: ExitRecord;
      /**
       * Where the exits came from.
       */
      source: ExitSource;
      /**
       * The turn.
       */
      turn_id: TurnId;
    }
  | {
      /**
       * True when the verdict came from the turn's cache.
       */
      cached?: boolean;
      /**
       * The call.
       */
      call_id: CallId;
      /**
       * The classifier's policy category, or the floor's rule.
       */
      category?: string | null;
      /**
       * The step of the classifier's chain that gave the verdict.
       */
      chain_step?: ChainStep | null;
      /**
       * Who judged.
       */
      judge: JudgeKind;
      kind: "exit_judged";
      /**
       * How long the classifier took, in milliseconds.
       */
      latency_ms?: number | null;
      /**
       * The classifier's model.
       */
      model?: string | null;
      /**
       * The classifier's reason.
       */
      rationale?: string | null;
      /**
       * True when the classifier model wrote no real reason (empty, too short or a
       * placeholder) and the judge asked it once more. Absent when false, and from an
       * earlier daemon.
       */
      reasked?: boolean;
      /**
       * The SHA-256 of the record the classifier saw, in hex.
       */
      record_sha256?: string | null;
      /**
       * The classifier's risk.
       */
      risk?: Risk | null;
      /**
       * The models that the classifier's chain skipped, each as `<model>: <reason>`.
       */
      skipped?: string[];
      /**
       * The turn.
       */
      turn_id: TurnId;
      /**
       * The classifier's view of the user's authorization.
       */
      user_authorization?: UserAuthorization | null;
      /**
       * The verdict.
       */
      verdict: Verdict;
    }
  | {
      /**
       * The call whose question offered the rule.
       */
      call_id: CallId;
      kind: "always_rule_saved";
      /**
       * The saved rule, or why there is none.
       */
      outcome: AlwaysSaved;
      /**
       * The turn.
       */
      turn_id: TurnId;
    }
  | {
      /**
       * The call.
       */
      call_id: CallId;
      kind: "exit_review_started";
      /**
       * The exits of the call.
       */
      kinds: ExitKind[];
      /**
       * The turn.
       */
      turn_id: TurnId;
    }
  | {
      /**
       * The denials that the limit counted: in a row, or in the turn.
       */
      denials: number;
      kind: "classifier_paused";
      /**
       * Why.
       */
      reason: PauseReason;
      /**
       * The turn.
       */
      turn_id: TurnId;
    }
  | {
      /**
       * The conversation.
       */
      conversation_id: ConversationId;
      kind: "presence_changed";
      /**
       * The new state.
       */
      presence: Presence;
      /**
       * What changed it.
       */
      source: PresenceSource;
    }
  | {
      /**
       * The call that deleted them in its layer.
       */
      call_id: CallId;
      /**
       * The bytes that the deletes freed.
       */
      freed_bytes: number;
      kind: "cache_delete_applied";
      /**
       * The deleted paths, at most [`SandboxSummary::MAX_CACHE_DELETES`].
       */
      paths: string[];
      /**
       * The turn.
       */
      turn_id: TurnId;
    }
  | {
      /**
       * The copy of the file before the call, in efr's phase A backup store. Absent
       * from phase B.
       */
      backup?: string | null;
      /**
       * The call.
       */
      call_id: CallId;
      /**
       * True when efr copied the file into its home store before the call, so
       * `efr undo` can bring it back.
       */
      covered?: boolean;
      /**
       * True when the file is a floor.
       */
      floor: boolean;
      kind: "home_file_changed";
      /**
       * The file, absolute.
       */
      path: string;
      /**
       * The real file, absolute, when `path` reaches it through a link, such as a
       * dotfile link into `~/dotfiles`. Absent when `path` is the real file, and in
       * events recorded before efr reported it.
       */
      real?: string | null;
      /**
       * The turn.
       */
      turn_id: TurnId;
    }
  | {
      /**
       * The call that made the changes.
       */
      call_id: CallId;
      /**
       * The changes.
       */
      changes: SurfaceChange[];
      kind: "sandbox_surface_changed";
      /**
       * True when at least one change went to quarantine.
       */
      quarantined: boolean;
      /**
       * The turn.
       */
      turn_id: TurnId;
    }
  | {
      /**
       * The call that made the changes; it does not wait.
       */
      call_id: CallId;
      /**
       * The quarantined changes.
       */
      changes: SurfaceChange[];
      kind: "surface_question_requested";
      /**
       * The question, which `sandbox.surface_respond` answers.
       */
      question_id: QuestionId;
      /**
       * The turn, which waits for the answer.
       */
      turn_id: TurnId;
    }
  | {
      /**
       * True when the changes moved back; false leaves them in quarantine.
       */
      keep: boolean;
      kind: "surface_question_answered";
      /**
       * The surface that answered; absent when the question expired or the turn was
       * interrupted.
       */
      origin?: Origin | null;
      /**
       * The question.
       */
      question_id: QuestionId;
      /**
       * The turn.
       */
      turn_id: TurnId;
    }
  | {
      /**
       * The files.
       */
      files: ReportedFile[];
      kind: "turn_surface_report";
      /**
       * The turn.
       */
      turn_id: TurnId;
    }
  | (Compaction & {
      kind: "conversation_compacted";
    })
  | {
      kind: "sandbox_unavailable";
      /**
       * Why, in one sentence.
       */
      reason: string;
    }
  | {
      /**
       * The snapshot's commit in efr's store, in hex.
       */
      commit: string;
      /**
       * The files in the snapshot.
       */
      files: number;
      kind: "snapshot_taken";
      /**
       * How long the snapshot took, in milliseconds.
       */
      ms: number;
      /**
       * Before or after.
       */
      phase: SnapshotPhase;
      /**
       * The root as a client shows it: the project's name, `$SCRATCH`, or `home` for
       * a copy of files outside the roots.
       */
      root: string;
      /**
       * The files that the snapshot left out, such as large or ignored files.
       */
      skipped: number;
      /**
       * The turn.
       */
      turn_id: TurnId;
    }
  | {
      /**
       * The paths that the undo skipped, because they changed after the turn.
       */
      conflicts: number;
      /**
       * The conversation.
       */
      conversation_id: ConversationId;
      /**
       * The links and empty entries that the undo removed.
       */
      deleted: number;
      kind: "undo_applied";
      /**
       * The paths that no snapshot covers, so the undo left them as they are.
       */
      not_covered: number;
      /**
       * The surface that asked.
       */
      origin: Origin;
      /**
       * True for a redo. False when absent.
       */
      redo?: boolean;
      /**
       * The refs that the undo moved, or tried to. Absent when none.
       */
      refs?: RefChange[];
      /**
       * The files that the undo wrote back.
       */
      restored: number;
      /**
       * True when the undo stopped at a write that failed, after it changed some
       * paths; the next `efr undo` finishes it. False when absent.
       */
      stopped?: boolean;
      /**
       * The directory of the undo trash. Absent when the undo put nothing there.
       */
      trash?: string | null;
      /**
       * The files that the undo moved to the undo trash.
       */
      trashed: number;
      /**
       * The undo.
       */
      undo_id: UndoId;
      /**
       * The turn whose changes the undo took back.
       */
      undone_turn_id: TurnId;
    }
  | {
      /**
       * The call.
       */
      call_id: CallId;
      /**
       * The job as it started.
       */
      job: JobInfo;
      kind: "job_started";
      /**
       * The process id of the job's launcher, which efrd started.
       */
      launcher_pid: number;
      /**
       * The start time of the launcher's process, in clock ticks after boot (the
       * 22nd field of `/proc/<pid>/stat`), so that a later efrd signals the launcher
       * and never another process that got its pid.
       */
      launcher_start: number;
      /**
       * The turn of the call.
       */
      turn_id: TurnId;
    }
  | {
      /**
       * The job.
       */
      job_id: JobId;
      kind: "job_watch_matched";
      /**
       * The line, at most 4 KiB.
       */
      line: string;
      /**
       * How many more matches may wake the model.
       */
      wakes_left: number;
    }
  | {
      /**
       * For `in_use`: the job of the same conversation that holds the port.
       */
      by_job?: JobId | null;
      /**
       * The job.
       */
      job_id: JobId;
      kind: "job_port_forwarded";
      /**
       * The address that the job listens on, as text: `127.0.0.1`, `::1`.
       */
      listen: string;
      /**
       * What efr did with the port.
       */
      outcome: ForwardOutcome;
      /**
       * The port.
       */
      port: number;
      /**
       * For `refused` and `limit`: a short reason, such as `a service port`.
       */
      reason?: string | null;
    }
  | {
      /**
       * The connections that efr's proxy refused while the job ran, at most
       * [`SandboxSummary::MAX_BLOCKED`]. Absent when none.
       */
      blocked?: Blocked[];
      /**
       * The refusals that `blocked` leaves out. 0 when absent.
       */
      blocked_more?: number;
      /**
       * The bus calls that the bus filter refused while the job ran (phase E), at
       * most [`SandboxSummary::MAX_BUS_REFUSED`]. Absent when none.
       */
      bus_refused?: BusRefusal[];
      /**
       * The refusals that `bus_refused` leaves out. 0 when absent.
       */
      bus_refused_more?: number;
      /**
       * The exit status of its command, when it exited. Absent otherwise.
       */
      exit_code?: number | null;
      /**
       * The job.
       */
      job_id: JobId;
      kind: "job_ended";
      /**
       * True when the job's launcher ended without its result, so the job's
       * processes may still run. False when absent.
       */
      launcher_lost?: boolean;
      /**
       * The bytes that the job wrote to its output.
       */
      output_bytes: number;
      /**
       * True when efr could not write all of the output to the output file, such as
       * on a full disk; the tail is whole. False when absent.
       */
      output_truncated?: boolean;
      /**
       * True when at least one of those changes went to quarantine. False when
       * absent.
       */
      quarantined?: boolean;
      /**
       * How long the job ran, in seconds.
       */
      ran_secs?: number;
      /**
       * How it ended.
       */
      state: JobState;
      /**
       * Who or what stopped it. Absent for a job that ended by itself.
       */
      stopped_by?: JobStopBy | null;
      /**
       * The git settings and other files that run code, which the job changed.
       * Absent when none.
       */
      surface_changes?: SurfaceChange[];
      /**
       * The names of the job's processes that efr could not end, which may still
       * run. Absent when none.
       */
      survivors?: string[];
      /**
       * The end of the output: at most 50 lines and 8 KiB. The job wrote it, so a
       * client shows it as text that a program wrote.
       */
      tail: string;
    }
  | {
      /**
       * The conversation.
       */
      conversation_id: ConversationId;
      /**
       * The job.
       */
      job_id: JobId;
      kind: "job_notice";
      /**
       * Always true: a notice is never the user's input.
       */
      not_user_input: boolean;
      /**
       * What the notice tells. Not named `kind`, which names the event's kind.
       */
      notice_kind: NoticeKind;
      /**
       * The sequence number of the `job_ended` or `job_watch_matched` event that the
       * notice tells about.
       */
      source_seq: Seq;
      /**
       * The text of the notice, as the model read it.
       */
      text: string;
      /**
       * The turn that took the notice.
       */
      turn_id: TurnId;
      /**
       * True when the notice started the turn; false when it rode in a turn that a
       * prompt started.
       */
      woke: boolean;
    }
  | {
      /**
       * The job.
       */
      job_id: JobId;
      kind: "job_notice_held";
      /**
       * Why no turn started.
       */
      reason: HeldReason;
      /**
       * The sequence number of the event that the notice tells about.
       */
      source_seq: Seq;
    }
  | {
      /**
       * Who approved the call. Absent when the call needed no approval.
       */
      approved_by?: ApprovedBy | null;
      /**
       * The call whose command still runs.
       */
      call_id: CallId;
      /**
       * The first 200 characters of the command line. The model wrote it, so a client
       * shows it as text that a program wrote, on one line.
       */
      command: string;
      /**
       * True when the command runs in the sandbox. A client never asks the user for a
       * hidden answer, an answer to a wait that looks secret, or a manual answer to a
       * command in the sandbox.
       */
      contained: boolean;
      /**
       * What the command waits for when its call answered.
       */
      input: InputWait;
      kind: "shell_command_detached";
      /**
       * True for a `visible` wait whose prompt line looks like a password prompt, as
       * in [`Event::ToolCallInputChanged`]. False when absent.
       */
      looks_secret?: boolean;
      /**
       * The row of the screen that holds the cursor while the command waits, at most
       * 200 characters, such as `u@h's password:`. The program wrote it, so a client
       * shows it as text that a program wrote, on one line. Absent when the command
       * does not wait.
       */
      prompt?: string;
      /**
       * When the command started.
       */
      since: string;
    }
  | {
      /**
       * The call whose command still runs.
       */
      call_id: CallId;
      /**
       * True when the command runs in the sandbox.
       */
      contained: boolean;
      /**
       * What the command waits for now.
       */
      input: InputWait;
      kind: "shell_command_input_changed";
      /**
       * True for a `visible` wait whose prompt line looks like a password prompt.
       * False when absent.
       */
      looks_secret?: boolean;
      /**
       * The row of the screen that holds the cursor, at most 200 characters. Absent
       * when the command does not wait.
       */
      prompt?: string;
    }
  | {
      /**
       * The call whose command ended.
       */
      call_id: CallId;
      /**
       * The exit status of the command, when the shell reported it. Absent otherwise.
       */
      exit_code?: number | null;
      /**
       * How it ended.
       */
      how: ShellCommandEnd;
      kind: "shell_command_ended";
    }
  | {
      kind: "conversation_deleted";
      /**
       * The surface that asked.
       */
      origin: Origin;
    };
/**
 * The scope key of a prompt: the registered project whose root holds the prompt's
 * working directory, or `outside` for every other place (`~`, `/etc`, a git
 * repository that is not registered).
 *
 * On the wire: `{"kind": "project", "value": "<project id>"}` or
 * `{"kind": "outside"}`. A kind that this build does not know reads as `unknown`,
 * with or without a `value`.
 */
export type RouteScope =
  | {
      kind: "project";
      value: ProjectId;
    }
  | {
      kind: "outside";
    }
  | {
      kind: "unknown";
    };
/**
 * A project that the user registered explicitly in `projects.toml`. A `.git`
 * directory alone never makes a project, because `$HOME` may be a dotfiles work tree.
 */
export type ProjectId = string;
/**
 * How a conversation came to a terminal (`terminal_attached`).
 */
export type AttachHow = "picked_up" | "resumed" | "unknown";
/**
 * What a turn is about, derived again from the shell's working directory on every turn.
 *
 * On the wire: `{"kind": "machine"}`, `{"kind": "path", "value": "/etc/nixos"}` or
 * `{"kind": "project", "value": "<project id>"}`.
 */
export type Scope =
  | {
      kind: "machine";
    }
  | {
      kind: "path";
      value: string;
    }
  | {
      kind: "project";
      value: ProjectId;
    };
/**
 * How a shell call runs.
 *
 * On the wire: `{"kind": "direct"}`, `{"kind": "contained", "grants": [...]}` or
 * `{"kind": "unsandboxed"}`. `grants` is left out when empty.
 */
export type Launch =
  | {
      kind: "direct";
    }
  | {
      /**
       * The approved widenings; none for a routine call.
       */
      grants?: Grant[];
      kind: "contained";
    }
  | {
      kind: "unsandboxed";
    };
/**
 * One widening of the sandbox for one call, after the user (or, from phase A, the
 * classifier) approved an exit. A grant never lifts a mask of an engine secret and
 * never crosses a floor.
 *
 * On the wire an object with a `kind` member, such as
 * `{"kind": "write", "path": "/home/u/notes"}` or `{"kind": "open_network"}`.
 */
export type Grant =
  | {
      kind: "write";
      /**
       * The path, absolute.
       */
      path: string;
    }
  | {
      /**
       * The host name, in lower case.
       */
      host: string;
      kind: "host";
      /**
       * The port.
       */
      port: number;
    }
  | {
      kind: "open_network";
    }
  | {
      kind: "socket";
      /**
       * The socket, absolute.
       */
      path: string;
    }
  | {
      /**
       * Which bus.
       */
      bus: BusKind;
      kind: "bus";
    }
  | {
      /**
       * Which desktop.
       */
      desktop: DesktopKind;
      kind: "desktop";
    }
  | {
      kind: "device";
      /**
       * The node, below `/dev`.
       */
      path: string;
    }
  | {
      kind: "unmask";
      /**
       * The masked path, absolute.
       */
      path: string;
    }
  | {
      kind: "net_view";
    }
  | {
      /**
       * Which bus.
       */
      bus: BusKind;
      kind: "bus_filter";
    }
  | {
      kind: "unknown";
    };
/**
 * A D-Bus message bus.
 */
export type BusKind = "system" | "session";
/**
 * A desktop whose control socket a [`Grant::Desktop`] binds: the compositor or the
 * window manager that a program such as `hyprctl` or `swaymsg` talks to.
 */
export type DesktopKind = "wayland" | "x11" | "hyprland" | "sway" | "i3" | "niri";
/**
 * Whether a running tool call's command waits for the user to type a line, and what
 * happens to that line.
 */
export type InputWait = "none" | "visible" | "hidden";
/**
 * What happened to one file.
 */
export type ChangeKind = "added" | "modified" | "deleted" | "renamed";
/**
 * Why the proxy refused a connection (phase D).
 */
export type BlockReason =
  | "not_allowed"
  | "private_address"
  | "sni_mismatch"
  | "upload_limit"
  | "method"
  | "ip_literal"
  | "not_tls"
  | "bad_request"
  | "limit"
  | "denied"
  | "unknown";
/**
 * The risk of an exit, as the classifier rates it.
 */
export type Risk = "low" | "medium" | "high" | "critical";
/**
 * How clearly the user's messages authorize an exit, as the classifier rates it.
 */
export type UserAuthorization = "none" | "low" | "medium" | "high";
/**
 * A verdict about an exit.
 */
export type Verdict = "allow" | "deny" | "ask_user";
/**
 * The kind of an exit: an action that the sandbox does not allow on its own.
 *
 * Each kind has its own launch after a "yes" and its own approvers. A floor kind
 * ([`ExitKind::is_floor`]) is denied before any question.
 */
export type ExitKind =
  | "write"
  | "host"
  | "host_view"
  | "socket"
  | "desktop_ipc"
  | "bus"
  | "device"
  | "masked_read"
  | "destructive"
  | "outside"
  | "privilege"
  | "persistence"
  | "upload"
  | "synced_write"
  | "above_root"
  | "secret"
  | "config";
/**
 * Why a model call stopped, as the provider reported it.
 */
export type StopReason = "end_turn" | "max_tokens" | "refusal" | "content_filter" | "pause_turn";
/**
 * Where an exit came from.
 */
export type ExitSource = "predicted" | "needs" | "cache_delete";
/**
 * The engine's path classes, as the protocol names them (`efr-protocol` cannot name
 * `efr_permissions::PathClass`).
 */
export type PathClassName =
  | "scratch"
  | "user_config"
  | "user_data"
  | "system"
  | "secrets"
  | "cache"
  | "build_output"
  | "trash";
/**
 * The step of the classifier's chain that gave a verdict: its own model, the turn's
 * model, or the user when each model step failed.
 */
export type ChainStep = "classifier_model" | "turn_model" | "user";
/**
 * Who judged an exit.
 */
export type JudgeKind = "classifier" | "user" | "floor" | "always" | "presence_expired" | "full";
/**
 * What became of the rule of an [`ApprovalDecision::AllowAlways`] answer (phase E).
 *
 * On the wire an object with a `kind` member, such as
 * `{"kind": "saved", "index": 2, "rule": "write under ~/notes"}` or
 * `{"kind": "not_saved", "reason": "read_only"}`.
 */
export type AlwaysSaved =
  | {
      /**
       * The rule's index in `sandbox.allow`.
       */
      index: number;
      kind: "saved";
      /**
       * The rule in words, such as `write under ~/notes`.
       */
      rule: string;
    }
  | {
      kind: "not_saved";
      /**
       * Why.
       */
      reason: NotSavedReason;
    }
  | {
      kind: "unknown";
    };
/**
 * Why the rule of an [`ApprovalDecision::AllowAlways`] answer was not saved (phase E).
 */
export type NotSavedReason =
  | "changed"
  | "read_only"
  | "load_check"
  | "io"
  | "reload_timeout"
  | "unknown";
/**
 * Why the classifier stops for the rest of a turn.
 */
export type PauseReason = "in_a_row" | "per_turn";
/**
 * What changed the presence of a conversation.
 */
export type PresenceSource = "input" | "user" | "timeout";
/**
 * One compaction of a conversation's context: a `conversation_compacted` event.
 */
export type CompactionId = string;
/**
 * What started a compaction.
 */
export type CompactionTrigger = "auto" | "manual" | "overflow";
/**
 * Which snapshot of a turn a [`Event::SnapshotTaken`](crate::Event::SnapshotTaken)
 * reports.
 */
export type SnapshotPhase = "pre" | "post";
/**
 * Why `turn.undo` skips a path or a ref, or why no snapshot covers it.
 */
export type UndoReason =
  | "changed_after_turn"
  | "no_record_after_turn"
  | "left_out"
  | "masked"
  | "over_limit"
  | "new_in_turn"
  | "later_turn_not_undone"
  | "no_mirror"
  | "objects_missing"
  | "outside_turn_calls"
  | "already_done"
  | "root_replaced"
  | "unknown"
  | "call_unfinished"
  | "snapshot_failed"
  | "store_changed"
  | "store_full"
  | "staged_only";
/**
 * One undo of a turn, or one redo: the name of its refs in efr's snapshot stores
 * and of its directory in the undo trash.
 */
export type UndoId = string;
/**
 * Where a job is in its life.
 */
export type JobState = "running" | "exited" | "stopped" | "timed_out" | "failed" | "unknown";
/**
 * Who or what stopped a job.
 */
export type JobStopBy =
  | "model"
  | "user"
  | "time_limit"
  | "daemon_stop"
  | "daemon_restart"
  | "guard"
  | "full_off"
  | "unknown";
/**
 * What efr did with a loopback port that a contained job listens on.
 */
export type ForwardOutcome = "forwarded" | "in_use" | "refused" | "limit" | "stopped" | "unknown";
/**
 * What a job notice tells the model.
 */
export type NoticeKind = "exited" | "watch" | "unknown";
/**
 * Why a job notice did not start a turn of its own: it waits for the next turn that a
 * prompt starts.
 */
export type HeldReason =
  | "not_auto"
  | "wake_limit"
  | "wake_turn_job"
  | "daemon_restart"
  | "wakes_off"
  | "unknown";
/**
 * Who approved a `shell` call whose command still runs in the hidden shell. A client
 * names it beside the command, so the user knows who let it run.
 */
export type ApprovedBy = "you" | "your_rule" | "reviewer" | "full_mode" | "unknown";
/**
 * How a command that a `shell` call left running in the hidden shell ended
 * ([`Event::ShellCommandEnded`]).
 */
export type ShellCommandEnd =
  | "ended"
  | "stopped"
  | "limit"
  | "closed"
  | "shell_exited"
  | "moved"
  | "unknown";
/**
 * The wire name of a method.
 */
export type MethodName =
  | "hello"
  | "conversations.list"
  | "conversation.subscribe"
  | "conversation.history"
  | "prompt.send"
  | "turn.interrupt"
  | "turn.steer"
  | "approval.respond"
  | "pty.attach"
  | "pty.write"
  | "pty.resize"
  | "input.respond"
  | "lease.report"
  | "models.list"
  | "projects.list"
  | "admin.project_add"
  | "admin.project_remove"
  | "admin.status"
  | "admin.config_reload"
  | "admin.login_openai"
  | "admin.login_api_key"
  | "admin.logout"
  | "sandbox.explain"
  | "sandbox.surface_respond"
  | "admin.sandbox_check"
  | "conversation.diff"
  | "prompt.withdraw"
  | "conversation.compact"
  | "presence.set"
  | "turn.undo"
  | "jobs.list"
  | "jobs.stop"
  | "conversation.context"
  | "conversations.recent"
  | "conversation.resume"
  | "shell.interrupt"
  | "shell.status"
  | "call.background"
  | "conversations.search"
  | "conversation.search"
  | "conversation.delete"
  | "device.pair_start"
  | "device.pair_confirm"
  | "device.enroll"
  | "device.prove"
  | "device.list"
  | "device.revoke";
/**
 * One item of an `admin.login_openai` stream.
 */
export type AdminLoginOpenAiItem =
  | {
      kind: "authorize_url";
      /**
       * The URL.
       */
      url: string;
    }
  | {
      kind: "completed";
      /**
       * The provider that is now logged in.
       */
      provider: string;
    };
/**
 * The outcome of one check of the probe.
 */
export type CheckOutcome = "ok" | "warn" | "fail" | "skipped";
/**
 * Why the bus filter is off (phase E).
 */
export type BusProxyOff = "config" | "no_program" | "self_test" | "old_version" | "unknown";
/**
 * How the sandbox treats the tool caches (`~/.cargo`, `~/.cache` and the others of
 * `sandbox.caches`).
 */
export type CacheMode = "overlay" | "tmp" | "readonly";
/**
 * What network a contained call has.
 */
export type NetworkMode = "none" | "proxy";
/**
 * Where a model catalog came from.
 */
export type CatalogOrigin = "backend" | "cache" | "builtin" | "missing";
/**
 * The identity of one daemon installation, created at its first start. Clients pin
 * it, so they notice when a socket path leads to a different daemon.
 */
export type DaemonId = string;
/**
 * Why the daemon refuses full mode although `permissions.allow_full` is true.
 */
export type FullRefusal = "root" | "snapshot_off" | "undo_off" | "no_zsh";
/**
 * How a provider is logged in.
 */
export type LoginKind = "subscription" | "api_key";
/**
 * Where a root directory's path came from, in the order the daemon looks.
 */
export type RootSource = "dir_variable" | "efr_home" | "xdg" | "run_user";
/**
 * The result of `call.background`.
 */
export type CallBackgroundResult =
  | {
      /**
       * The command line, as the model wrote it.
       */
      command: string;
      /**
       * The exits of the job decision, such as a delete that undo does not cover as a
       * job. Absent when none.
       */
      exits?: ExitInfo[];
      kind: "preview";
      /**
       * The time limit of the job, in minutes, from the move.
       */
      max_minutes: number;
      /**
       * The token that a confirm sends back in `confirm`.
       */
      token: number;
    }
  | {
      /**
       * The new job.
       */
      job_id: JobId;
      kind: "moved";
    }
  | {
      kind: "unknown";
    };
/**
 * A kind of content in the request.
 */
export type ContextPartKind =
  | "system"
  | "tools"
  | "summary"
  | "prompts"
  | "preambles"
  | "notices"
  | "answers"
  | "tool_calls"
  | "tool_outputs"
  | "pruned"
  | "unknown";
/**
 * The result of `conversation.delete`.
 */
export type ConversationDeleteResult =
  | {
      /**
       * The conversation.
       */
      conversation_id: ConversationId;
      /**
       * The background jobs that efrd stopped first. Absent when none.
       */
      jobs_stopped?: number;
      kind: "deleted";
      /**
       * The files and dirs of the conversation that efrd could not remove. The next
       * start of efrd tries again. Absent when none.
       */
      not_removed?: string[];
      /**
       * Its title, for the line of the client.
       */
      title?: string | null;
    }
  | {
      kind: "held";
      /**
       * The conversation's title, for the question.
       */
      title?: string | null;
      /**
       * That terminal.
       */
      tty: string;
    }
  | {
      kind: "unknown";
    };
/**
 * The result of `conversation.resume`.
 */
export type ConversationResumeResult =
  | {
      /**
       * True when a prompt from the working directory of the params continues it:
       * the directory is in its scope, or `conversation.follow_project` is off.
       */
      continues_here?: boolean;
      /**
       * The conversation.
       */
      conversation_id: ConversationId;
      /**
       * True when the terminal held it already: the resume only made its slot the
       * current one.
       */
      here?: boolean;
      kind: "resumed";
      /**
       * Its scope: the slot of the terminal that holds it now.
       */
      scope: RouteLabel;
      /**
       * The directory of its hidden shell: where efrd started it, or where the
       * shell that runs is. Absent when no shell could start.
       */
      shell_dir?: string | null;
      /**
       * Its title.
       */
      title?: string | null;
      /**
       * When it was last active, before the resume.
       */
      updated_at: string;
    }
  | {
      kind: "held";
      /**
       * The conversation's title, for the question.
       */
      title?: string | null;
      /**
       * That terminal.
       */
      tty: string;
    }
  | {
      kind: "unknown";
    };
/**
 * What the text of a hit is. A reader takes a kind that it does not know as
 * `unknown`.
 */
export type SearchKind = "title" | "prompt" | "answer" | "command" | "unknown";
/**
 * Why a search stopped before it saw every match. It is no error: the rows are the
 * best of what it saw. A reader takes a reason that it does not know as `unknown`.
 */
export type SearchPartial = "row_cap" | "time_cap" | "unknown";
/**
 * One item of a `conversation.subscribe` stream.
 */
export type ConversationSubscribeItem =
  | (EventEnvelope & {
      kind: "event";
    })
  | (ConversationSnapshot & {
      kind: "snapshot";
    })
  | (Draft & {
      kind: "draft";
    });
/**
 * What a conversation is doing.
 */
export type ConversationStatus = "idle" | "running" | "awaiting_approval";
/**
 * The part of a running turn that a [`Draft`] carries.
 */
export type DraftPart =
  | {
      /**
       * The text after `offset`.
       */
      delta: string;
      /**
       * The position of the message among the assistant messages of the turn that
       * have text, from 0. A model answer without text takes no position.
       */
      index: number;
      kind: "text";
      /**
       * The offset of `delta` in the text of the message, in bytes of UTF-8. It is
       * not a count of characters or of UTF-16 units: a JavaScript client compares
       * it with the UTF-8 length of its text, never with `text.length`.
       */
      offset: number;
    }
  | {
      /**
       * The reasoning text after `offset`.
       */
      delta: string;
      kind: "reasoning";
      /**
       * The offset of `delta` in the reasoning text of the whole turn, in bytes of
       * UTF-8. The reasoning of a later model call in the same turn continues the
       * text after a blank line.
       */
      offset: number;
      /**
       * The title of the newest reasoning section: the last line of the reasoning
       * so far that is all bold (`**Title**`), without the stars. Absent until
       * such a line comes.
       */
      title?: string | null;
    }
  | {
      /**
       * How many bytes of input the model has written so far: of the JSON
       * arguments, or of the text of a freeform tool such as `apply_patch`.
       */
      bytes: number;
      /**
       * The position of the call among the tool calls of this answer of the model,
       * from 0, so a client can tell two calls of one answer apart.
       */
      call: number;
      kind: "tool_input";
      /**
       * The name of the tool.
       */
      tool: string;
    }
  | (ContextUse & {
      kind: "context";
    })
  | {
      kind: "compacting";
      /**
       * What started the compaction: `auto` or `overflow`.
       */
      trigger: CompactionTrigger;
    };
/**
 * A permission that a connection holds. The daemon checks the scope of every method
 * before it runs; [`ScopeName::for_method`] is the table.
 *
 * | Scope | Allows | Phone default |
 * |---|---|---|
 * | `read` | `hello`, list, subscribe, history, `lease.report`, `models.list`, `sandbox.explain` | yes |
 * | `operate` | send and withdraw prompts, interrupt, steer, compact the context, set the presence, undo a turn | yes |
 * | `approve` | answer approvals and quarantine questions | yes |
 * | `terminal` | attach to, write to and resize a PTY | no, an explicit opt-in |
 * | `admin` | status, login, config reload, the project registry, the sandbox check, the pairing, list and revoke of devices, and later settings | never; Unix socket only |
 */
export type ScopeName = "read" | "operate" | "approve" | "terminal" | "admin";
/**
 * One item of a `device.pair_start` stream.
 */
export type DevicePairStartItem =
  | {
      /**
       * When the token expires.
       */
      expires_at: string;
      kind: "started";
      /**
       * The pairing, for `device.pair_confirm`.
       */
      pairing_id: PairingId;
      /**
       * What the QR code holds; [`PairingQr::url`] is its text.
       */
      qr: PairingQr;
    }
  | {
      /**
       * The name that the phone sent.
       */
      device_name: string;
      /**
       * The fingerprint of the phone's key, which the phone shows too.
       */
      key_fingerprint: string;
      kind: "pending";
      /**
       * The name of the phone's Tailscale node.
       */
      node_name: string;
      /**
       * The operating system of the phone's node, such as `iOS`, when Tailscale
       * knows it.
       */
      os?: string | null;
      /**
       * The pairing.
       */
      pairing_id: PairingId;
      /**
       * The login of the Tailscale user of the phone's node, such as
       * `anna@example.com`. For display only.
       */
      user_login: string;
    }
  | {
      /**
       * The new device.
       */
      device_id: DeviceId;
      kind: "paired";
      /**
       * The pairing.
       */
      pairing_id: PairingId;
    }
  | {
      kind: "ended";
      /**
       * The pairing.
       */
      pairing_id: PairingId;
      /**
       * Why.
       */
      reason: PairEnd;
    };
/**
 * Why a pairing ended without a device, the `reason` of the `ended` item of
 * `device.pair_start`.
 */
export type PairEnd = "refused" | "expired" | "attempts" | "unknown";
/**
 * Where a model of the list comes from.
 */
export type ModelSource = "builtin" | "config";
/**
 * How the conversation of a terminal changed with a prompt.
 */
export type Switch =
  | {
      kind: "back";
      /**
       * Its title.
       */
      title?: string | null;
      /**
       * When it was last active, before this prompt.
       */
      updated_at: string;
    }
  | {
      kind: "picked_up";
      /**
       * Its title.
       */
      title?: string | null;
      /**
       * When it was last active, before this prompt.
       */
      updated_at: string;
    }
  | {
      /**
       * The scope of the terminal's conversation before. Absent when the reason is
       * not a change of scope.
       */
      from?: RouteLabel | null;
      kind: "new";
      /**
       * Why a new conversation started.
       */
      reason: NewReason;
    }
  | {
      kind: "unknown";
    };
/**
 * Why a prompt started a new conversation.
 */
export type NewReason =
  | {
      kind: "scope";
    }
  | {
      /**
       * How many whole hours it was idle.
       */
      hours: number;
      kind: "idle";
    }
  | {
      kind: "moved";
      /**
       * The title of the conversation that moved.
       */
      title?: string | null;
      /**
       * That terminal.
       */
      tty: string;
    }
  | {
      kind: "unknown";
    };
/**
 * One item of a `pty.attach` stream.
 */
export type PtyAttachItem =
  | {
      kind: "snapshot";
      /**
       * The recording offset that the snapshot reflects.
       */
      seq: Seq;
      /**
       * The screen.
       */
      snapshot: ScreenSnapshot;
    }
  | {
      /**
       * The bytes.
       */
      data: Base64Bytes;
      kind: "output";
      /**
       * The recording offset of the first byte of `data`.
       */
      seq: Seq;
    }
  | {
      kind: "resized";
      /**
       * The recording offset at which the new size applies.
       */
      seq: Seq;
      /**
       * The new size.
       */
      size: Size;
    };
/**
 * A cell colour. On the wire: `{"indexed": 1}` or `{"rgb": [255, 128, 0]}`.
 */
export type Color =
  | {
      indexed: number;
    }
  | {
      /**
       * @minItems 3
       * @maxItems 3
       */
      rgb: [number, number, number];
    };
/**
 * What the sandbox does with a path.
 */
export type SandboxPathRole =
  | "write_root"
  | "cache_overlay"
  | "private_tmp"
  | "read_only"
  | "masked"
  | "floor"
  | "granted";
/**
 * What `turn.undo` does with one path.
 */
export type UndoAction = "restore" | "trash" | "delete" | "skip" | "not_covered" | "command";
/**
 * The protocol version that both sides send in `hello`.
 */
export type ProtocolVersion = 1;
/**
 * Every request gets zero or more item frames and then one end or error frame.
 */
export type ServerFrame = ServerItemFrame | ServerEndFrame | ErrorFrame | ServerAckFrame;
/**
 * The wire name of a method that streams: it may send many items.
 */
export type StreamMethodName =
  | "conversation.subscribe"
  | "pty.attach"
  | "admin.login_openai"
  | "device.pair_start";

/**
 * The efr protocol, generated from efr-protocol by `cargo xtask protocol-schema`; never edit it by hand. Frames are JSON objects: length-prefixed on the Unix socket, one text message each on the WebSocket. Every offset into text counts bytes of UTF-8. An event of a kind not listed here must be accepted and skipped.
 */
export interface EfrProtocol {
  client_frame?: ClientFrame;
  error_code?: ErrorCode;
  event?: Event;
  event_envelope?: EventEnvelope;
  method_name?: MethodName;
  method_params?: MethodParams;
  method_results?: MethodResults;
  protocol?: ProtocolVersion;
  scope_name?: ScopeName;
  server_frame?: ServerFrame;
  stream_method_name?: StreamMethodName;
}
/**
 * The params of `hello`, the first request on every connection.
 *
 * The daemon answers a `protocol` other than its own with `protocol_mismatch` before any
 * other method runs, and it records the rest as the connection's context for logs and
 * for the origin of the prompts that the connection sends.
 */
export interface Hello {
  /**
   * What the client supports.
   */
  capabilities?: Capabilities;
  /**
   * The client program and its version, such as `efr 0.1.0`, for logs.
   */
  client?: string | null;
  /**
   * The enrolled device that is connecting. Only remote connections send it.
   */
  device_id?: DeviceId | null;
  /**
   * The kind of surface the client is. A connection on the tailnet listener is always
   * treated as [`Origin::Phone`], whatever it claims.
   */
  origin: Origin;
  /**
   * The client's process id.
   */
  pid?: number | null;
  /**
   * The client's [`PROTOCOL_VERSION`](crate::PROTOCOL_VERSION).
   */
  protocol: number;
  /**
   * The client's terminal, when it runs in one. The daemon ignores it on the
   * tailnet listener: a remote device sits at no terminal of the computer.
   */
  tty?: string | null;
}
/**
 * Optional features that one side of a connection supports, announced in `hello`.
 *
 * Every key is optional, so a new key is an additive change: an old peer ignores it,
 * and a new peer reads its absence as "not supported". Keys that this build does not
 * know are kept in [`extra`](Self::extra), so a proxy can pass them on unchanged.
 */
export interface Capabilities {
  /**
   * The daemon accepts the admin methods (`admin.*`) on this connection. Only the
   * Unix socket ever offers them.
   */
  admin?: boolean | null;
  /**
   * The daemon sets `local_only` in `approval_requested`, so a client knows which
   * questions only the user's own machine may answer. Absent: a client cannot tell,
   * and the daemon refuses an answer that it may not give.
   */
  approval_local_only?: boolean | null;
  /**
   * The daemon takes `device.enroll` and `device.prove` on this connection. Only a
   * connection of the tailnet listener offers them.
   */
  device_prove?: boolean | null;
  /**
   * The daemon merges the drafts of each part that a slow subscriber could not take
   * yet, and sends the merged draft when the subscriber has room, so a client gets
   * fewer drafts on a slow link but no gaps. Absent: a draft can be lost.
   */
  drafts_coalesced?: boolean | null;
  /**
   * The daemon starts `pty.attach` with a screen snapshot; from a client, the client
   * can render one.
   */
  screen_snapshots?: boolean | null;
  /**
   * The daemon answers `conversations.search` and `conversation.search`. A client
   * reads its absence as a daemon from before the search.
   */
  search?: boolean | null;
  /**
   * The daemon fills `live` in the snapshot of `conversation.subscribe`: the text
   * and the reasoning of the running turn so far.
   */
  snapshot_live?: boolean | null;
  [k: string]: unknown;
}
/**
 * The params of `conversations.list`.
 */
export interface ConversationsList {
  /**
   * Where to continue: the `next_cursor` of the previous page. Absent for the first
   * page.
   */
  cursor?: PageCursor | null;
  /**
   * The most conversations to return; the daemon picks a default and a maximum.
   */
  limit?: number | null;
  /**
   * Only the conversations of this terminal, its current one first: the one of its
   * last prompt, `,new` or resume. One page holds them all, with no cursor. A daemon
   * from before routing by scope ignores it and lists every conversation.
   */
  tty?: string | null;
}
/**
 * The params of `conversation.subscribe`, a streaming method.
 *
 * With `after_seq`, the daemon replays the events after it when the gap is small (at
 * most 128 events or 1 MiB) and otherwise sends a bounded snapshot; without it, the
 * stream starts with a snapshot. Live events follow, each with a larger sequence number
 * than anything sent before. A subscriber that falls behind its bounded queue is closed
 * with `overflow` and its `last_seq`, and subscribes again from there.
 */
export interface ConversationSubscribe {
  /**
   * The largest sequence number the client has already seen.
   */
  after_seq?: Seq | null;
  /**
   * True when a person at this client can type answers to a running command that
   * waits for input, with `input.respond`. The daemon stops a command that waits for
   * hidden input, such as a password, when no live subscription of its conversation
   * can answer. Absent means false, so an older client counts as one that cannot.
   */
  answers_input?: boolean;
  /**
   * The conversation to follow.
   */
  conversation_id: ConversationId;
  /**
   * True when the client wants [`Draft`] items: the text, the reasoning and the
   * tool input of a running turn as they arrive from the model, before the event
   * log has them. Absent means false, so an older client gets no drafts.
   */
  drafts?: boolean;
}
/**
 * The params of `conversation.history`.
 */
export interface ConversationHistory {
  /**
   * The conversation.
   */
  conversation_id: ConversationId;
  /**
   * Where to continue: a `next_cursor` or a snapshot's `history_cursor`. Absent for
   * the most recent page.
   */
  cursor?: PageCursor | null;
  /**
   * The most events to return; the daemon picks a default and a maximum.
   */
  limit?: number | null;
}
/**
 * The params of `prompt.send`.
 *
 * Without `conversation_id`, the prompt goes to the active conversation of the
 * context's tty, and a new conversation starts when the tty has none. A prompt sent
 * while a turn runs queues behind it.
 *
 * `Debug` leaves out `last_command` and `recent_commands`, because a command line can
 * hold a secret and requests are logged.
 */
export interface PromptSend {
  /**
   * Makes the send idempotent: a retry with the same id returns the first result.
   */
  command_id: CommandId;
  /**
   * The user's shell when the prompt was sent.
   */
  context?: ShellContext | null;
  /**
   * The conversation to send to.
   */
  conversation_id?: ConversationId | null;
  /**
   * The last command line from the user's shell history, for the turn's live-state
   * preamble only.
   *
   * It is not part of [`ShellContext`], because a command line can hold a secret such
   * as `export TOKEN=...`. The daemon hands it to the turn in memory and never records
   * it in an event, which the event log keeps forever and every subscriber receives.
   */
  last_command?: string | null;
  /**
   * Start a new conversation and make it the tty's active one (`,new`). The daemon
   * rejects it together with `conversation_id`.
   */
  new_conversation?: boolean;
  /**
   * The recent commands of the terminal, oldest first, for the turn's live-state
   * preamble only, as `last_command`. Absent means none, as from an older client.
   */
  recent_commands?: RecentCommand[];
  /**
   * The mode, model and effort that the prompt asks for its turn; the config gives
   * the rest. Absent means none, as from a client older than turn settings.
   */
  settings?: TurnSettings;
  /**
   * The prompt.
   */
  text: string;
}
/**
 * What the zsh plugin observed in the user's shell when it sent a prompt.
 *
 * The plugin sends it with every `,` line and Ctrl+Space line, and the daemon computes
 * the git root and the [`Scope`](crate::Scope) from it, so the plugin stays fast. It is
 * context for the model, never a sandbox root or a process working directory.
 *
 * It holds no command line. The last command can hold a secret such as
 * `export TOKEN=...`, and a context is recorded in the `prompt_queued` event, which the
 * event log keeps forever and every subscriber receives. The last command therefore
 * travels as [`PromptSend::last_command`](crate::PromptSend::last_command) and reaches
 * only the turn's preamble. A `last_command` member that a client still puts here is
 * ignored like any unknown member, so it can never reach an event.
 */
export interface ShellContext {
  /**
   * The variables that name the desktop of the terminal, so that a desktop grant
   * reaches the session of this prompt. Absent when the shell has none of them. They
   * never go into the model's preamble. Boxed, so a method that carries a context
   * stays small.
   */
  desktop?: DesktopEnv | null;
  /**
   * The host name of the machine the shell runs on.
   */
  hostname?: string | null;
  /**
   * The exit status of the last command, `$?`.
   */
  last_status?: number | null;
  /**
   * The shell's `$OLDPWD`.
   */
  oldpwd?: string | null;
  /**
   * The shell's `$PWD`.
   */
  pwd: string;
  /**
   * The process id of the user's shell, `$$`.
   */
  shell_pid?: number | null;
  /**
   * The nesting depth of the shell, `$SHLVL`.
   */
  shlvl?: number | null;
  /**
   * `$SSH_CONNECTION`, set when the shell runs over SSH.
   */
  ssh_connection?: string | null;
  /**
   * The terminal device, `$TTY`. The daemon keeps one active conversation per tty.
   */
  tty?: string | null;
}
/**
 * The desktop variables of the user's shell, as the shell has them. Only these six
 * names, never another variable, and whether the shell has `$XAUTHORITY`. efrd checks
 * each value before it binds a socket.
 */
export interface DesktopEnv {
  /**
   * `$DISPLAY`, such as `:0`.
   */
  display?: string | null;
  /**
   * True when the shell has `$XAUTHORITY`, so its X server needs a cookie. Only
   * that the variable is set, never its path: the cookie file stays masked.
   */
  has_xauthority?: boolean;
  /**
   * `$HYPRLAND_INSTANCE_SIGNATURE`.
   */
  hyprland_instance_signature?: string | null;
  /**
   * `$I3SOCK`.
   */
  i3sock?: string | null;
  /**
   * `$NIRI_SOCKET`.
   */
  niri_socket?: string | null;
  /**
   * `$SWAYSOCK`.
   */
  swaysock?: string | null;
  /**
   * `$WAYLAND_DISPLAY`: a socket name below `$XDG_RUNTIME_DIR`, or an absolute path.
   */
  wayland_display?: string | null;
}
/**
 * A command line that the user ran in the terminal before a prompt: one entry of the
 * zsh plugin's list of the terminal's recent commands.
 *
 * It travels like [`PromptSend::last_command`]: never in a [`ShellContext`], never in
 * an event, and only to the turn's preamble. `Debug` shows the command only by its
 * length.
 */
export interface RecentCommand {
  /**
   * The command line as the user typed it.
   */
  command: string;
  /**
   * The shell's `$PWD` when it started.
   */
  cwd?: string | null;
  /**
   * How long it ran, in milliseconds.
   */
  duration_ms?: number | null;
  /**
   * When it ended.
   */
  ended_at?: string | null;
  /**
   * Its exit status.
   */
  status?: number | null;
}
/**
 * The settings that a prompt asks for its turn. Each one it leaves out comes from the
 * daemon's config when the turn starts.
 *
 * The daemon checks each given value at `prompt.send` and again when the turn starts:
 * the mode by name, the model against `models.list`, the effort against the efforts
 * that `models.list` names for the turn's model.
 */
export interface TurnSettings {
  /**
   * The reasoning effort, such as `high`. A plain string, because the efforts belong to
   * the model and the backend, so a new effort needs no protocol change.
   */
  effort?: string | null;
  /**
   * The permission mode.
   */
  mode?: Mode | null;
  /**
   * The model id, such as `gpt-5.5`.
   */
  model?: string | null;
}
/**
 * The params of `turn.interrupt`.
 *
 * Interrupting has two phases: the daemon records the request at once
 * (`turn_interrupt_requested`) and records `turn_interrupted` when the model's stream
 * has really stopped.
 *
 * Esc and Ctrl+C in the input row of a turn also hand back what the client sent for
 * that turn: `resend_steers`, `withdraw_steers` and `withdraw`. The daemon does all of
 * it in the one actor step that records `turn_interrupt_requested`, so no queued
 * prompt can start and no model call can read a steer in between. It records, in this
 * order: `turn_interrupt_requested`, one `prompt_withdrawn` for each withdrawn prompt
 * in queue order, the `steering_withdrawn` of the steers taken back, then the
 * `prompt_queued` of the resent steers. When the turn is not running (`conflict`), it
 * does none of it.
 */
export interface TurnInterrupt {
  /**
   * Makes the request idempotent.
   */
  command_id: CommandId;
  /**
   * The conversation whose running turn to stop.
   */
  conversation_id: ConversationId;
  /**
   * How the daemon queues the prompt of `resend_steers`: `queue` with the context,
   * the last command and the settings of the terminal that interrupts, as a late
   * steer of `turn.steer` would go. Absent: the context and the settings of the
   * interrupted turn's prompt. The resend never makes the interrupt fail: the
   * prompt does not count against `max_queued`, and settings that cannot work now
   * give way to those of the interrupted turn's prompt. Boxed, so the method stays
   * small.
   */
  resend_as?: LateSteer | null;
  /**
   * The `turn_steered` events of this turn, by sequence number, whose text the
   * client wants sent again as a new prompt. The daemon takes those that no
   * `steering_delivered` names yet and skips the others. It joins their texts in
   * sequence order with newlines into one prompt, which runs next, before the
   * prompts that wait in the queue. The prompt has the context and the settings of
   * the interrupted turn's prompt, and its `prompt_queued` names the steers and has
   * this request's `command_id`. Absent means none.
   */
  resend_steers?: Seq[];
  /**
   * The turn the client means. When it is given and is not the running turn, the
   * daemon answers `conflict` instead of stopping a different turn.
   */
  turn_id?: TurnId | null;
  /**
   * Queued prompts of this conversation to withdraw, by turn. The daemon withdraws
   * those that still wait and skips the others: a prompt that started, ended or was
   * withdrawn, and a turn that it does not know. Absent means none.
   */
  withdraw?: TurnId[];
  /**
   * The `turn_steered` events of this turn, by sequence number, that the client takes
   * back without sending them again (Ctrl+C). The daemon takes those that no
   * `steering_delivered` names yet and that `resend_steers` does not list, records
   * one `steering_withdrawn` for them, and returns their texts. No model call reads
   * them. Absent means none.
   */
  withdraw_steers?: Seq[];
}
/**
 * The params of `turn.steer` (`,!` in the shell, Enter in the input row of a turn).
 * Unlike `prompt.send`, it does not queue: the text joins the running turn, and the
 * next model call of the turn reads it.
 *
 * A steer is late when the turn will make no more model calls: the model answered
 * without a tool call, the user asked to interrupt the turn, `turn_id` names a turn
 * that is not running, or no turn runs. Without `if_late`, the daemon refuses a late
 * steer with `conflict`, as a daemon from before `if_late` does. A late steer is never
 * recorded as `turn_steered`.
 */
export interface TurnSteer {
  /**
   * Makes the request idempotent.
   */
  command_id: CommandId;
  /**
   * The conversation whose running turn to steer.
   */
  conversation_id: ConversationId;
  /**
   * What the daemon does with a late steer. Absent: it refuses it with `conflict`.
   */
  if_late?: LateSteer | null;
  /**
   * The guidance.
   */
  text: string;
  /**
   * The turn the client means. When it is given and is not the running turn, the
   * steer is late.
   */
  turn_id?: TurnId | null;
}
/**
 * The params of `approval.respond`. The daemon answers `not_found` for a call that has
 * no pending approval, which includes one that expired at a restart.
 */
export interface ApprovalRespond {
  /**
   * The tool call that waits for approval.
   */
  call_id: CallId;
  /**
   * Makes the answer idempotent.
   */
  command_id: CommandId;
  /**
   * The conversation of the waiting turn.
   */
  conversation_id: ConversationId;
  /**
   * The answer.
   */
  decision: ApprovalDecision;
}
/**
 * The params of `pty.attach`, a streaming method. Sequence numbers in this stream are
 * byte offsets in the PTY's recording.
 *
 * With `since_seq`, the stream starts with the output after that offset, if the
 * recording still holds it, with each size that the PTY took from that offset on as
 * `resized` at its place. Otherwise it starts with a screen snapshot, which has the
 * size. Live output follows either way. Attaching is idempotent and never changes the
 * PTY.
 */
export interface PtyAttach {
  /**
   * The PTY to watch.
   */
  pty_id: PtyId;
  /**
   * How many rows of scrollback a snapshot includes; the daemon caps it.
   */
  scrollback_rows?: number | null;
  /**
   * The recording offset the client has already rendered up to.
   */
  since_seq?: Seq | null;
}
/**
 * The params of `pty.write`: input bytes for the PTY, as if typed. Only input goes
 * this way; answers to terminal queries come from the daemon's own screen.
 */
export interface PtyWrite {
  /**
   * The bytes.
   */
  data: Base64Bytes;
  /**
   * The PTY to write to.
   */
  pty_id: PtyId;
}
/**
 * The params of `pty.resize`.
 */
export interface PtyResize {
  /**
   * The PTY to resize.
   */
  pty_id: PtyId;
  /**
   * The size the client wants.
   */
  size: Size;
}
/**
 * A terminal size in character cells.
 */
export interface Size {
  /**
   * Columns.
   */
  cols: number;
  /**
   * Rows.
   */
  rows: number;
}
/**
 * The params of `input.respond`: one line that the user typed for a running tool call
 * whose last `tool_call_input_changed` event said that it waits for input.
 *
 * The daemon writes `text` and then a carriage return to the call's PTY, but only
 * while that call's command runs, the wait it reported last is of the answer's kind
 * (hidden with `hidden`, visible without) and the job that waited still holds the
 * terminal, and with `hidden` only while the PTY also reads a line with echo off. A
 * `manual` answer needs no reported wait: it is written while that call's command runs
 * and the job that the daemon's last look saw, not the shell itself, holds the
 * terminal, and never for a call whose command types into a shell that reads command
 * lines (a `nested_shell` call, or a command that starts a shell or a REPL), where it
 * would run as a command line. The daemon answers
 * `not_found` for an unknown conversation or when no call's command runs in it (the
 * call ended, so an answer after its `tool_call_completed` always gets `not_found`, or
 * its command was left at its timeout and goes on without a call), `conflict` when
 * another call's command runs or the call's command does not wait for that input, and
 * `invalid` for a text that is not one line. Then it writes nothing.
 *
 * There is no command id: like `pty.write`, an answer is never retried, so the daemon
 * keeps no receipt, which would also store the text. `Debug` never shows the text.
 */
export interface InputRespond {
  /**
   * The call that waits for input.
   */
  call_id: CallId;
  /**
   * The conversation of the running call.
   */
  conversation_id: ConversationId;
  /**
   * True when the client asked the user for hidden input, because the event said
   * `hidden`; false for `visible`. The daemon refuses an answer of the other kind. With
   * `hidden` it also requires the PTY's echo to be off when it writes, so the terminal
   * does not put the text in the output that the model reads.
   */
  hidden: boolean;
  /**
   * True when the user chose to type an input for a command that reported no wait,
   * such as one that printed nothing for a while (`Ctrl+\` in `efr`). The daemon
   * then skips the check against a reported wait and its kind, and keeps the others.
   * False when absent.
   */
  manual?: boolean;
  /**
   * The line without its line ending: at most [`InputRespond::MAX_TEXT_BYTES`] bytes
   * and no control characters (U+0000 to U+001F and U+007F). The daemon never logs or
   * records it.
   */
  text: SecretText;
}
/**
 * The params of `lease.report`. A client reports a lease about every 25 seconds,
 * naming what it wants kept warm, and the daemon uses the leases to stop rendering
 * unwatched screens and to decide whether to notify. A new report replaces the
 * connection's previous one.
 */
export interface LeaseReport {
  /**
   * The conversations the client shows.
   */
  conversations?: ConversationId[];
  /**
   * The PTYs whose screens the client shows.
   */
  ptys?: PtyId[];
  /**
   * True when the client is in the foreground and a person can see it.
   */
  visible: boolean;
}
/**
 * The params of `models.list`. It takes none.
 */
export interface ModelsList {}
/**
 * The params of `projects.list`. It takes none.
 */
export interface ProjectsList {}
/**
 * The params of `admin.project_add`, an admin method (Unix socket only).
 *
 * The daemon adds the project to `projects.toml` in its config root, keeping the
 * file's comments, and reloads, so the permission engine trusts the project from the
 * next tool call on.
 */
export interface AdminProjectAdd {
  /**
   * True to register the root of the git work tree that holds `path` instead, or
   * `path` itself when none does. The daemon then refuses a root that is the home
   * directory, `/` or a directory above the home directory, which only an explicit
   * `path` may register. False when absent.
   */
  git_root?: boolean;
  /**
   * A name for people; absent means the last component of the root.
   */
  name?: string | null;
  /**
   * An absolute directory. The daemon registers it with symbolic links resolved.
   */
  path: string;
}
/**
 * The params of `admin.project_remove`, an admin method (Unix socket only).
 */
export interface AdminProjectRemove {
  /**
   * The root of the project, absolute. It matches a registered root as given, and
   * with symbolic links resolved when the directory still exists.
   */
  path: string;
}
/**
 * The params of `admin.status`, an admin method (Unix socket only). It takes none.
 */
export interface AdminStatus {}
/**
 * The params of `admin.config_reload`, an admin method (Unix socket only). It takes
 * none.
 *
 * The daemon also reloads on its own when the file changes and on `SIGHUP`; this
 * method reloads at once and answers with the outcome.
 */
export interface AdminConfigReload {}
/**
 * The params of `admin.login_openai`, a streaming admin method (Unix socket only). It
 * takes none.
 *
 * The daemon starts the login, sends the authorize URL as the first item, waits for
 * the browser to come back to its loopback listener, stores the credentials and ends
 * the stream. A second login while one is running gets `busy`.
 */
export interface AdminLoginOpenAi {}
/**
 * The params of `admin.login_api_key`, a unary admin method (Unix socket only).
 *
 * The daemon refuses a key that is empty or holds whitespace or a character outside
 * ASCII, checks the key with one request to the provider that runs no model unless
 * `check` is false, stores it as the provider's credential, records
 * `login_completed` and answers. The key never reaches an event, a log line or the
 * text of an error.
 */
export interface AdminLoginApiKey {
  /**
   * True to check the key with the provider before the daemon stores it. True when
   * absent.
   */
  check?: boolean;
  /**
   * The key, as the provider issued it.
   */
  key: SecretText;
  /**
   * The provider of the key: `openai-api` or `anthropic-api`.
   */
  provider: string;
}
/**
 * The params of `admin.logout`, a unary admin method (Unix socket only).
 *
 * The daemon deletes the provider's stored credentials, records `logout_completed`,
 * and the provider's next request fails as not logged in. The provider keeps an API
 * key valid: only its owner can revoke it at the provider.
 */
export interface AdminLogout {
  /**
   * The provider: `openai-subscription`, `openai-api` or `anthropic-api`.
   */
  provider: string;
}
/**
 * The params of `sandbox.explain`, a read method.
 *
 * It is not an `admin.*` method on purpose: it changes nothing and reveals no more
 * than the user's own config does, so `read` is enough.
 */
export interface SandboxExplain {
  /**
   * The directory the question is asked from, which picks the project. Absent: the
   * daemon's view of the machine scope.
   */
  cwd?: string | null;
  /**
   * The path to explain: absolute, or relative to `cwd`.
   */
  path: string;
}
/**
 * The params of `sandbox.surface_respond`. The daemon refuses it from a phone and
 * from a process that the model's commands started, and answers `not_found` for a
 * question that is not pending.
 */
export interface SandboxSurfaceRespond {
  /**
   * Makes the answer idempotent.
   */
  command_id: CommandId;
  /**
   * The conversation of the waiting turn.
   */
  conversation_id: ConversationId;
  /**
   * True moves the changes back from quarantine; false leaves them there.
   */
  keep: boolean;
  /**
   * The question.
   */
  question_id: QuestionId;
}
/**
 * The params of `admin.sandbox_check`, an admin method (Unix socket only). It takes
 * none.
 */
export interface AdminSandboxCheck {}
/**
 * The params of `conversation.diff`, a read method.
 *
 * The daemon compares the turn's first snapshot (before its first call that can
 * write) with its last (at turn end), in each registered project and `$SCRATCH` that
 * the turn wrote through its calls.
 */
export interface ConversationDiff {
  /**
   * The conversation. Absent: the conversation of `turn_id`, else the active
   * conversation of the terminal that the connection's `hello` named.
   */
  conversation_id?: ConversationId | null;
  /**
   * True asks for the list of files only, without the unified diff.
   */
  stat?: boolean;
  /**
   * The turn. Absent: the newest finished turn of the conversation. A turn that ran
   * no call that can write has no snapshots, and its list of files is empty.
   */
  turn_id?: TurnId | null;
}
/**
 * The params of `prompt.withdraw` (Alt+Up in the input row of a turn).
 *
 * The daemon removes one waiting prompt from the queue, records `prompt_withdrawn`
 * and returns the prompt's text, so the client can put it back where the user types.
 * The turn of a withdrawn prompt never starts; `prompt_withdrawn` is its last event.
 * A prompt that started, ended or was withdrawn gets `conflict`. A turn that this
 * conversation never queued, and a terminal without a waiting prompt, get
 * `not_found`.
 */
export interface PromptWithdraw {
  /**
   * Makes the request idempotent: a retry with the same id returns the first result.
   */
  command_id: CommandId;
  /**
   * The conversation whose queue holds the prompt.
   */
  conversation_id: ConversationId;
  /**
   * Which prompt to withdraw.
   */
  target: WithdrawTarget;
}
/**
 * The params of `conversation.compact` (`efr compact [focus]`, `,compact [focus]`).
 *
 * efrd compacts the context of an idle conversation: it prunes old tool output and,
 * when that does not free enough room, writes a summary of the history before the
 * verbatim tail. It records `conversation_compacted` and answers when the compaction
 * is done. It never starts a turn.
 *
 * Refusals, each with nothing recorded:
 *
 * - `conflict` while a turn of the conversation runs: the turn compacts on its own
 *   when it needs to, so efrd does not wait for it;
 * - `conflict` when nothing lies before the verbatim tail, so a compaction would free
 *   no room;
 * - `not_found` for a conversation that does not exist.
 *
 * A prompt that arrives while the compaction runs waits in the queue and starts after
 * it.
 */
export interface ConversationCompact {
  /**
   * Makes the request idempotent: a retry with the same id returns the first result.
   */
  command_id: CommandId;
  /**
   * The conversation to compact.
   */
  conversation_id: ConversationId;
  /**
   * What the summary must keep, in the user's words, such as `the failing test and
   * its fix`. Absent: the summary keeps what its fixed sections ask for.
   */
  focus?: string | null;
}
/**
 * The params of `presence.set`. Any later input of an attached client sets the
 * conversation present again. The daemon refuses it from a process that the model's
 * commands started, so sandboxed code cannot make questions expire.
 */
export interface PresenceSet {
  /**
   * Makes the change idempotent.
   */
  command_id: CommandId;
  /**
   * The conversation. Absent: every conversation that had input since the daemon
   * started.
   */
  conversation_id?: ConversationId | null;
  /**
   * The new presence.
   */
  presence: Presence;
}
/**
 * The params of `turn.undo`.
 *
 * `efr undo` sends it twice: first with `dry_run` for the preview, then without it to
 * apply. The apply checks each path again. A phone cannot undo.
 */
export interface TurnUndo {
  /**
   * Makes the undo idempotent.
   */
  command_id: CommandId;
  /**
   * The conversation. Absent: the conversation of `turn_id`, else the active
   * conversation of the terminal that the connection's `hello` named.
   */
  conversation_id?: ConversationId | null;
  /**
   * True shows what the undo does and changes nothing. False when absent.
   */
  dry_run?: boolean;
  /**
   * True also overwrites a path that changed after the turn, and undoes a turn while
   * a newer turn is not undone. False when absent.
   */
  force?: boolean;
  /**
   * True lists the turns that can be undone in `turns` and changes nothing. False
   * when absent.
   */
  list?: boolean;
  /**
   * True takes back the newest undo of the conversation instead. False when absent.
   */
  redo?: boolean;
  /**
   * The turn. Absent: the newest turn of the conversation that is not undone.
   */
  turn_id?: TurnId | null;
}
/**
 * The params of `jobs.list`.
 */
export interface JobsList {
  /**
   * The conversation. Absent: the jobs of every conversation.
   */
  conversation_id?: ConversationId | null;
}
/**
 * The params of `jobs.stop`.
 *
 * The daemon refuses it from a process that the model's commands started: the model
 * stops its jobs with the `job_stop` tool, which records the stop as the model's.
 */
export interface JobsStop {
  /**
   * Makes the stop idempotent.
   */
  command_id: CommandId;
  /**
   * The conversation of the job. Absent: the active conversation of the terminal
   * that the connection's `hello` named. Job ids repeat across conversations.
   */
  conversation_id?: ConversationId | null;
  /**
   * The job.
   */
  job_id: JobId;
}
/**
 * The params of `conversation.context`, a read method.
 *
 * efrd builds the history that the next request of the conversation would send, with
 * the system prompt and the tool definitions, and measures each part. It never sends
 * the text of a message: only sizes, counts and the labels that `efr history` shows
 * already (the command line of a shell call, the path of a file tool, the id of a
 * job). While a turn runs, the answer holds the turn's messages up to now, from the
 * log.
 *
 * Refusals: `not_found` for a conversation that does not exist.
 */
export interface ConversationContext {
  /**
   * The conversation.
   */
  conversation_id: ConversationId;
  /**
   * The model to measure for. Absent: the model of the newest turn, else the
   * default model. The model decides the window and whether the provider's own
   * items of earlier turns go back.
   */
  model?: string | null;
}
/**
 * The params of `conversations.recent`, a read method.
 *
 * efrd lists the conversations whose scope key is the key of `pwd` first, newest
 * first by their last activity, then all others, newest first. Only efrd knows the
 * key of a directory and which terminal holds a conversation, so the order is
 * efrd's.
 */
export interface ConversationsRecent {
  /**
   * The most conversations to return; efrd picks a default of 10 and a maximum.
   */
  limit?: number | null;
  /**
   * The working directory that decides which scope comes first. Absent: newest
   * first only.
   */
  pwd?: string | null;
  /**
   * The terminal that asks, such as `/dev/pts/3`: its conversations show as `here`,
   * and `current` names its current one. Absent for a client without a terminal,
   * such as the phone.
   */
  tty?: string | null;
}
/**
 * The params of `conversation.resume`, an operate method.
 *
 * The conversation goes into the slot of its own scope key in the terminal of
 * `context`, and that slot becomes the terminal's current one: efrd records
 * `terminal_attached` (`how: resumed`). Another terminal that held it lets it go, and
 * its next prompt there says where it went. When the conversation has no hidden shell
 * that runs, efrd starts one in its last directory.
 *
 * Refusals: `invalid` for a context without a tty, `forbidden` for a connection that
 * is not the shell's or `efr`'s on that terminal (a phone, a made-up tty), and
 * `not_found` for a conversation that does not exist.
 */
export interface ConversationResume {
  /**
   * Makes the resume idempotent.
   */
  command_id: CommandId;
  /**
   * The terminal's shell, as the zsh plugin observed it: its `tty` and `shell_pid`
   * name the terminal and the shell that hold the conversation, and its `pwd` says
   * whether the next prompt continues it.
   */
  context: ShellContext;
  /**
   * The conversation.
   */
  conversation_id: ConversationId;
  /**
   * Move the conversation also when another open terminal holds it. Without it,
   * efrd moves nothing and answers `held`.
   */
  take?: boolean;
}
/**
 * The params of `shell.interrupt`.
 *
 * Without `close`, the daemon sends SIGINT to the process group of the command that
 * `call_id` left running in the conversation's hidden shell, and only while that group
 * holds the shell's terminal and is not the shell's own group. With `close`, it closes
 * the hidden shell (SIGHUP, then SIGKILL), for a command that ignores SIGINT; the next
 * call starts a new shell. The command's end then comes as `shell_command_ended` with
 * `how` `stopped` or `closed`.
 *
 * The daemon answers `not_found` for an unknown conversation and when the call's
 * command no longer runs, and `conflict` when the shell itself holds the terminal.
 * It needs the scope `terminal`, as an answer to the command does.
 */
export interface ShellInterrupt {
  /**
   * The call whose command still runs.
   */
  call_id: CallId;
  /**
   * True: close the hidden shell (SIGHUP, then SIGKILL), for a command that ignores
   * SIGINT. False: SIGINT to the command's group. False when absent.
   */
  close?: boolean;
  /**
   * Makes the stop idempotent.
   */
  command_id: CommandId;
  /**
   * The conversation of the hidden shell.
   */
  conversation_id: ConversationId;
}
/**
 * The params of `shell.status`.
 */
export interface ShellStatus {
  /**
   * The conversation of the hidden shell.
   */
  conversation_id: ConversationId;
}
/**
 * The params of `call.background`.
 *
 * The daemon checks the call before it moves it: the call is a running `shell` call of
 * the conversation, jobs are on and have a free place, the line can run as a job, the
 * job decision allows it, and the command does not wait for input now. A refusal is
 * `invalid` with a short reason for a person, such as `the command waits for input`.
 *
 * Without `confirm`, a call that runs in the sandbox moves at once: it keeps its
 * sandbox, so the move opens nothing new, and the key press is the user's answer. The
 * answer is then `moved`. A call that runs outside the sandbox (in the exit child, in
 * full mode, or a line typed into the hidden shell in `cautious` and `manual`) does not
 * move yet: the answer is a `preview`, and the client asks the user one short question. Only a yes sends `call.background` again with the token of
 * the preview in `confirm`. The daemon checks the call again then, and refuses when the
 * job decision changed since the preview.
 *
 * `call_id` may also name a call whose command was left running in the hidden shell
 * (`shell_command_detached`): the daemon then moves that command, with the same checks
 * and the same preview, while it waits for no input. The command's
 * `shell_command_ended` then says `moved`.
 *
 * It needs the scope `terminal`, so the phone and the model's own commands can never
 * move a call.
 */
export interface CallBackground {
  /**
   * The running `shell` call.
   */
  call_id: CallId;
  /**
   * Makes the move idempotent.
   */
  command_id: CommandId;
  /**
   * Absent: check the call; move it when it runs in the sandbox, else answer a
   * preview and move nothing. Present: the token of the preview that the user
   * answered with yes; the call moves.
   */
  confirm?: number | null;
  /**
   * The conversation of the call.
   */
  conversation_id: ConversationId;
}
/**
 * The params of `conversations.search`, a read method for user-side peers: one row per
 * conversation.
 *
 * efrd searches an index of the titles, the user's prompts, the model's answers and
 * the commands that the agent ran. The index holds no command output and no file
 * contents, and its text has the secrets that efr recognises replaced. efrd parses
 * `query` itself, so a client never sends search syntax: words split on white space,
 * a `"double quoted"` part is one phrase, every word must match in one prompt, answer
 * or command, and only the last word matches as a prefix. A query of more than 256
 * characters or 16 words is `invalid`.
 */
export interface ConversationsSearch {
  /**
   * The most rows to return: 50 when absent, 200 at most.
   */
  limit?: number | null;
  /**
   * The working directory that decides the current scope, for an empty query.
   * Absent: no scope comes first.
   */
  pwd?: string | null;
  /**
   * The words to find. Empty after a trim: the recent conversations, in the order of
   * `conversations.recent`.
   */
  query: string;
  /**
   * Also return the measures of the search in `stats`. False when absent.
   */
  stats?: boolean;
  /**
   * The terminal that asks, such as `/dev/pts/3`: its conversations show as `here`,
   * and `current` names its current one. Absent on the phone.
   */
  tty?: string | null;
}
/**
 * The params of `conversation.search`, a read method for user-side peers: the hits in
 * one conversation.
 *
 * efrd reads `query` as `conversations.search` does. `not_found` for a conversation
 * that does not exist.
 */
export interface ConversationSearch {
  /**
   * The conversation.
   */
  conversation_id: ConversationId;
  /**
   * The most hits to return: 20 when absent, 50 at most.
   */
  limit?: number | null;
  /**
   * The words to find. Empty after a trim: the newest prompts and the start of the
   * newest answer, with no ranges.
   */
  query: string;
}
/**
 * The params of `conversation.delete`, a terminal method.
 *
 * efrd stops the conversation's background jobs and waits for their end, closes its
 * hidden shell, then records `conversation_deleted` with the receipt. From that event
 * on, no list, search or read shows the conversation, and the store refuses every new
 * event of it. Then efrd removes its events, its search index rows, its saved
 * messages, its receipts, its recordings, its `$SCRATCH`, its sandbox dirs, its
 * snapshot refs and its undo trash. A stop of efrd in the middle leaves no part of the
 * conversation visible, and the next start of efrd finishes the removal.
 *
 * Refusals: `busy` while a turn runs, a prompt waits in its queue, a compaction or an
 * undo runs, unless `stop` is set; `not_found` for a conversation that does not exist
 * or that is deleted already.
 */
export interface ConversationDelete {
  /**
   * Makes the delete idempotent.
   */
  command_id: CommandId;
  /**
   * The conversation.
   */
  conversation_id: ConversationId;
  /**
   * Delete the conversation also when another open terminal holds it. Without it,
   * efrd deletes nothing and answers `held`.
   */
  force?: boolean;
  /**
   * Stop the running turn, the queued prompts, a compaction or an undo first.
   * Without it, efrd deletes nothing and answers `busy` while one of them runs.
   */
  stop?: boolean;
}
/**
 * The params of `device.pair_start`, a streaming admin method (Unix socket only).
 *
 * The daemon makes a one-time token and sends the `started` item with the QR code.
 * When a phone sends that token with `device.enroll`, the daemon sends `pending`
 * with the facts of the phone, and the terminal answers with `device.pair_confirm`.
 * The stream then sends `paired` or `ended` and ends. A cancel of the request, or the
 * end of the connection, ends the pairing: the token is then good for nothing.
 */
export interface DevicePairStart {
  /**
   * The name to give the device. Absent: the name that the phone sends.
   */
  name?: string | null;
}
/**
 * The params of `device.pair_confirm`, an admin method (Unix socket only). It answers
 * the `pending` item of a `device.pair_start` stream.
 *
 * A pairing that is not pending, or that ended, is `not_found`.
 */
export interface DevicePairConfirm {
  /**
   * True to enroll the device, false to refuse it.
   */
  accept: boolean;
  /**
   * The pairing.
   */
  pairing_id: PairingId;
}
/**
 * The params of `device.enroll`, a remote method that runs before the proof.
 *
 * The daemon checks the token, shows the phone's facts at the terminal and answers
 * only after the terminal confirms. A wrong or used token is `unauthorized`; a no at
 * the terminal, or a pairing that expires, is `forbidden`. The phone checks
 * `daemon_id` and `protocol` of the `hello` result against the QR code before it
 * sends the token. The Unix socket refuses the method.
 */
export interface DeviceEnroll {
  /**
   * The name of the device, such as `Anna's iPhone`, for the terminal and for
   * `device.list`.
   */
  name: string;
  /**
   * The device's P-256 public key: the 65-byte uncompressed point (`04`, then X and
   * Y), as standard base64.
   */
  public_key: Base64Bytes;
  /**
   * The token of the QR code.
   */
  token: SecretText;
}
/**
 * The params of `device.prove`, a remote method that runs before the proof.
 *
 * The device signs [`ProveMessage::to_bytes`](crate::ProveMessage::to_bytes) of the
 * `hello` result's `challenge`, `daemon_id`, `protocol` and `prove` facts. Before this
 * method succeeds, a remote connection may call only `hello`, `device.enroll` and
 * `device.prove`. A wrong signature, or a device that is revoked or that enrolled from
 * another node, is `unauthorized`; the daemon closes the connection after a few
 * failed proofs. The Unix socket refuses the method.
 */
export interface DeviceProve {
  /**
   * The device. When the `hello` sent a `device_id`, it is the same.
   */
  device_id: DeviceId;
  /**
   * The ECDSA P-256 signature with SHA-256 of the message, in DER, as standard
   * base64.
   */
  signature: Base64Bytes;
}
/**
 * The params of `device.list`, an admin method (Unix socket only). It takes none.
 */
export interface DeviceList {}
/**
 * The params of `device.revoke`, an admin method (Unix socket only).
 *
 * The daemon marks the device revoked and closes its open connections at once. A
 * device that does not exist is `not_found`. A revoke of a revoked device changes
 * nothing.
 */
export interface DeviceRevoke {
  /**
   * The device.
   */
  device_id: DeviceId;
}
/**
 * Cancel a request in flight.
 */
export interface ClientCancel {
  cancel: RequestId;
}
/**
 * The settings that a turn runs with: the prompt's [`TurnSettings`] over the config's
 * defaults, checked by the daemon.
 */
export interface EffectiveSettings {
  /**
   * The reasoning effort; absent when none is sent, so the backend uses its own
   * default for the model.
   */
  effort?: string | null;
  /**
   * Why the turn runs with a stricter mode than it asked for, such as `auto` without
   * a working sandbox; `mode` is the mode it runs with. Absent when it runs with the
   * mode it asked for.
   */
  fallback?: ModeFallback | null;
  /**
   * The permission mode. For a turn from a remote origin it is at most `cautious`,
   * whatever the prompt or the config asked for.
   */
  mode: Mode;
  /**
   * The model id.
   */
  model: string;
  /**
   * Which values came from the prompt rather than from the config. Absent when none
   * did.
   */
  overridden?: OverriddenSettings;
}
/**
 * Why a turn that asked for one mode runs with a stricter one: `auto` without a
 * working sandbox, or in a project at the home directory, runs as `cautious`, and a
 * `full` turn that a job notice starts runs with at most `auto`.
 */
export interface ModeFallback {
  /**
   * The mode that the prompt or the config asked for.
   */
  asked: Mode;
  /**
   * Why it was not used, in one sentence, such as `Landlock ABI 6 found; auto needs
   * 9 (Linux 7.1)`.
   */
  reason: string;
}
/**
 * Which of a turn's [`EffectiveSettings`] the prompt set; the others are the config's.
 * Each flag is left out when it is false.
 */
export interface OverriddenSettings {
  /**
   * The effort came from the prompt.
   */
  effort?: boolean;
  /**
   * The mode came from the prompt.
   */
  mode?: boolean;
  /**
   * The model came from the prompt.
   */
  model?: boolean;
}
/**
 * The files that changed in the roots of a call or a turn while it ran. efr compares
 * its own snapshots, so a file that another conversation or the user changed at the
 * same time is listed too.
 */
export interface FileChanges {
  /**
   * Lines added in all changed files, the ones left out too.
   */
  added?: number;
  /**
   * The changed files, at most [`MAX_LISTED_FILES`], sorted by path.
   */
  files: FileChange[];
  /**
   * How many changed files `files` leaves out.
   */
  more?: number;
  /**
   * Lines removed in all changed files, the ones left out too.
   */
  removed?: number;
}
/**
 * One changed file.
 */
export interface FileChange {
  /**
   * Lines added; 0 for a binary file.
   */
  added: number;
  /**
   * True when the file is binary, so no lines are counted. False when absent.
   */
  binary?: boolean;
  /**
   * Where a renamed file was, in the same form as `path`.
   */
  from?: string | null;
  /**
   * What happened.
   */
  kind: ChangeKind;
  /**
   * The file as a client shows it: relative to the turn's project root, under
   * `$SCRATCH/` for the conversation's scratch directory, under `~/` in the home
   * directory, otherwise absolute.
   */
  path: string;
  /**
   * Lines removed; 0 for a binary file.
   */
  removed: number;
}
/**
 * What a contained or unsandboxed call did to the state around it. Names only: no
 * value of a variable and no file content.
 */
export interface SandboxSummary {
  /**
   * The programs of background jobs that stopped when the call ended.
   */
  background_stopped?: string[];
  /**
   * Connections that the proxy refused (phase D), at most
   * [`MAX_BLOCKED`](Self::MAX_BLOCKED): one entry for each host, port and reason.
   */
  blocked?: Blocked[];
  /**
   * The refusals that `blocked` leaves out (phase D2).
   */
  blocked_more?: number;
  /**
   * Bus calls that the bus filter refused (phase E), at most
   * [`MAX_BUS_REFUSED`](Self::MAX_BUS_REFUSED): one entry for each bus, name,
   * member and path.
   */
  bus_refused?: BusRefusal[];
  /**
   * The refusals that `bus_refused` leaves out (phase E).
   */
  bus_refused_more?: number;
  /**
   * The paths in the tool caches that the call deleted in its private layer, at most
   * [`MAX_CACHE_DELETES`](Self::MAX_CACHE_DELETES) and
   * [`MAX_CACHE_DELETE_BYTES`](Self::MAX_CACHE_DELETE_BYTES). efr asks whether to
   * delete them in the real cache too.
   */
  cache_deletes?: string[];
  /**
   * The cache deletes that `cache_deletes` leaves out: over its limits, a name that
   * is not UTF-8, or a whole directory hidden at once.
   */
  cache_deletes_more?: number;
  /**
   * True when the call ran in the sandbox; false for the exit child.
   */
  confined: boolean;
  /**
   * True when the hidden shell's working directory changed.
   */
  cwd_changed?: boolean;
  /**
   * Exported names that were dropped everywhere, such as `LD_PRELOAD`.
   */
  dropped?: string[];
  /**
   * Exported names that stay in the sandbox's state only.
   */
  kept_out?: string[];
  /**
   * Exported names that reached the hidden shell.
   */
  promoted?: string[];
  /**
   * True when the call ran with a proxy plan (phase D2). The CLI's trace reads it.
   */
  proxied?: boolean;
  /**
   * Why the sandbox could not start, such as bubblewrap's setup message. The command
   * did not run, and efr checks the sandbox again.
   */
  setup_error?: string | null;
  /**
   * Git settings and other files that run code, which the call changed.
   */
  surface_changes?: SurfaceChange[];
  /**
   * The programs of an approved exit's descendants that efr could not end, such as a
   * process that `sudo` left running as root. They may still hold the hidden
   * shell's terminal, so efr starts a new shell for the next call.
   */
  survivors?: string[];
}
/**
 * A connection that the proxy refused (phase D).
 */
export interface Blocked {
  /**
   * How many connections of the call the proxy refused for this host, port and
   * reason. 1 when absent.
   */
  count?: number;
  /**
   * The host, or `<invalid>` when the request named no valid host.
   */
  host: string;
  /**
   * The port.
   */
  port: number;
  /**
   * Why.
   */
  reason: BlockReason;
}
/**
 * One bus call that the bus filter refused (phase E), with how often. The names come
 * from the command's own messages, so a client shows them as text that a program
 * wrote.
 */
export interface BusRefusal {
  /**
   * Which bus.
   */
  bus: BusKind;
  /**
   * How many such calls the filter refused. 1 when absent.
   */
  count?: number;
  /**
   * `interface.member`, the member alone for a call with no interface, or empty for
   * a refusal of the bus itself that names no member.
   */
  member: string;
  /**
   * The destination, as the client wrote it, or `<invalid>`.
   */
  name: string;
  /**
   * The object path, or `<invalid>`.
   */
  path: string;
}
/**
 * One change to a git setting or another file that runs code, found by the surface
 * guard after a call.
 */
export interface SurfaceChange {
  /**
   * The code key it sets, such as `core.fsmonitor`.
   */
  key?: string | null;
  /**
   * The file or directory that changed.
   */
  path: string;
  /**
   * True when the launcher moved it to quarantine, where nothing reads it.
   */
  quarantined: boolean;
  /**
   * The rule it broke, such as `commondir_in_main_git_dir`.
   */
  rule: string;
}
/**
 * What an approval question for an exit shows, on top of its one-line summary.
 */
export interface ExitInfo {
  /**
   * The rule that an `allow_always` answer saves, in words, such as
   * `write under ~/notes` or `host download.example.org:443` (phase E). Absent when
   * no rule may cover the call; a client offers the answer only when it is set.
   */
  always?: string | null;
  /**
   * Facts that efr collected itself, one line each, such as `runs /usr/bin/python,
   * not .venv/bin/python` or `untrusted: written in the sandbox`.
   */
  facts?: string[];
  /**
   * Exactly what a "yes" opens; empty for an unsandboxed launch.
   */
  grants?: Grant[];
  /**
   * The classifier's answer, when it judged first and asked the user (phase 3).
   */
  judged?: Judgement | null;
  /**
   * The exits of the call, each kind once.
   */
  kinds: ExitKind[];
  /**
   * How the call runs after a "yes".
   */
  launch: Launch;
  /**
   * The model's reason from `needs`, shown labelled as the model's.
   */
  model_reason?: string | null;
  /**
   * True when only the user may approve this exit.
   */
  user_only?: boolean;
}
/**
 * The classifier's answer about one exit (phase A).
 */
export interface Judgement {
  /**
   * The policy category, at most 40 characters.
   */
  category: string;
  /**
   * Why, at most 300 characters.
   */
  rationale: string;
  /**
   * How risky the action is.
   */
  risk: Risk;
  /**
   * How clearly a user message authorizes it.
   */
  user_authorization: UserAuthorization;
  /**
   * Allow, deny or ask the user.
   */
  verdict: Verdict;
}
/**
 * How full the model's context is, against the point where efrd compacts it.
 *
 * A client shows `ctx N%`, where N is `tokens` as a percent of `limit`: 100% means that
 * a compaction runs now.
 */
export interface ContextUse {
  /**
   * The tokens at which a client shows 100%: the auto compaction trigger
   * (`compaction.auto_at` percent of `window`), or the hard cap (95% of `window`) when
   * `compaction.auto` is off.
   */
  limit: number;
  /**
   * The tokens in the context: after a model call, its input plus its output; before
   * a call, efrd's estimate of the request.
   */
  tokens: number;
  /**
   * The context window of the turn's model, in tokens. For a model whose window efr
   * does not know, the default that efrd counts with.
   */
  window: number;
}
/**
 * What `efr undo` can take back of a turn, at the turn's end: the roots with the
 * files that changed, the files outside the roots, and what no snapshot covers.
 */
export interface TurnUndoInfo {
  /**
   * The files outside the roots that the turn changed and that efr copied before
   * the write. Absent when none.
   */
  home_files?: number;
  /**
   * What undo cannot bring back, each with the reason in a few words, at most
   * [`MAX_NOT_COVERED`]: `target/ (ignored, large)`, `refs of app (shallow)`,
   * `app (a snapshot failed)`. Absent when none.
   */
  not_covered?: string[];
  /**
   * How many names `not_covered` leaves out. Absent when none.
   */
  not_covered_more?: number;
  /**
   * The roots whose files the turn changed. Absent when none.
   */
  roots?: RootFiles[];
  /**
   * True when `turn.undo` can take back the turn now: a snapshot from before the
   * turn is kept, `undo.enabled` is true and no call of the turn still runs. False
   * when absent.
   */
  undoable?: boolean;
}
/**
 * How many files of one root a turn changed, for the line at the end of the turn.
 */
export interface RootFiles {
  /**
   * The files of the root that undo can bring back.
   */
  files: number;
  /**
   * The refs of the root's project that the turn moved (branches, tags, the stash and
   * `HEAD`) and that undo can move back. 0 when absent.
   */
  refs?: number;
  /**
   * The root as a client shows it: the project's name (`eggfriedrice.code`),
   * `$SCRATCH`, or the name of another project that the turn wrote.
   */
  shown: string;
}
/**
 * Tokens that a turn used, as the provider reported them.
 *
 * `input_tokens`, `output_tokens`, `cached_input_tokens`, `reasoning_tokens`,
 * `cache_write_tokens` and `cache_write_1h_tokens` are sums over the model calls of the
 * turn. `context_tokens` is not a sum: it is the size of the context at the last call.
 * The cache counts are parts of `input_tokens` for every provider, so
 * `cached_input_tokens / input_tokens` is the part of the input that the cache served.
 */
export interface Usage {
  /**
   * The part of `cache_write_tokens` written with a time to live of one hour. Absent
   * when none, and always for a provider without such a choice, such as OpenAI.
   */
  cache_write_1h_tokens?: number;
  /**
   * The part of `input_tokens` that the provider wrote to its prompt cache, with
   * every time to live. Absent when none, or when the provider did not say.
   */
  cache_write_tokens?: number;
  /**
   * The part of `input_tokens` that the provider served from its prompt cache. Absent
   * when none, or when the provider did not say.
   */
  cached_input_tokens?: number;
  /**
   * The tokens in the context at the last model call: its input plus its output.
   * Absent when no call reported its usage, and in turns recorded before efr counted
   * it.
   */
  context_tokens?: number;
  /**
   * Tokens sent to the model, cached ones included.
   */
  input_tokens: number;
  /**
   * Tokens the model produced, reasoning included.
   */
  output_tokens: number;
  /**
   * The part of `output_tokens` that the model spent on reasoning. Absent when none,
   * or when the provider did not say.
   */
  reasoning_tokens?: number;
}
/**
 * Why the model of a completed turn stopped, when a client shows it: the last model
 * call gave no answer, or it stopped for another reason than the end of its answer.
 */
export interface TurnStop {
  /**
   * True when the last model call gave no text and no tool call.
   */
  empty?: boolean;
  /**
   * The stop reason of the last model call.
   */
  reason: StopReason;
}
/**
 * The body of an error: a code to act on, a message for a person, and optional data
 * whose shape depends on the code.
 */
export interface ErrorBody {
  /**
   * What went wrong.
   */
  code: ErrorCode;
  /**
   * Structured details. [`ErrorCode::Overflow`] carries `{"last_seq": n}` and
   * [`ErrorCode::ProtocolMismatch`] carries `{"daemon": n, "client": m}`.
   */
  data?: {
    [k: string]: unknown;
  };
  /**
   * One sentence for a person. Clients show it; they never parse it.
   */
  message: string;
}
/**
 * Everything the classifier sees about one exit. Never in it: tool output, file
 * contents, assistant messages, the model's `needs.reason` or project files.
 */
export interface ExitRecord {
  /**
   * The action that leaves the sandbox.
   */
  action: ActionFacts;
  /**
   * Facts that efr collected itself.
   */
  facts: ExitFacts;
  /**
   * Every prompt of the conversation, newest last, cut to 24 KiB with the oldest
   * dropped first.
   */
  user_messages: string[];
  /**
   * The record's version, [`ExitRecord::VERSION`].
   */
  version: number;
}
/**
 * The action of an exit record.
 */
export interface ActionFacts {
  /**
   * True when the call starts a background job: its grants then last for the whole
   * job, up to [`max_minutes`](Self::max_minutes), while nobody watches it. False
   * when absent.
   */
  background?: boolean;
  /**
   * The working directory of the call.
   */
  cwd: string;
  /**
   * The payload of a `write_file`, `edit` or `apply_patch` call that edits a floor
   * file, which efr copies to its home store first: the new content, the old and
   * the new strings, or the hunks of that file. For a named persistence form of a
   * shell line that installs another file, such as a link into a unit dir: the path
   * and the text of that file, which efrd read outside every write root. At most
   * [`MAX_EDIT_BYTES`](Self::MAX_EDIT_BYTES); the user decides a longer payload. The
   * model or the files it chose wrote it, so it can hold attacker text, as `line`
   * can.
   */
  edit?: string | null;
  /**
   * The exits of the call.
   */
  exits: ExitKind[];
  /**
   * Exactly what an "allow" opens.
   */
  grants?: Grant[];
  /**
   * The exact line. The model wrote it, so it can hold attacker text: a comment or a
   * string in it never authorizes anything.
   */
  line: string;
  /**
   * The time limit of the job in minutes, after efr's own limit. Set for every
   * record with [`background`](Self::background); absent otherwise.
   */
  max_minutes?: number | null;
  /**
   * The turn's scope.
   */
  scope: Scope;
  /**
   * Where the exits came from.
   */
  source: ExitSource;
  /**
   * The tool, such as `shell`.
   */
  tool: string;
  /**
   * True when the call comes from a turn that a job notice started, not a prompt:
   * nobody asked for this step now. False when absent.
   */
  wake_turn?: boolean;
  /**
   * True when the job has a watch pattern, which wakes the model on a matching line
   * of its output. False when absent.
   */
  watch?: boolean;
}
/**
 * Facts about an exit that efr collected itself, never from the model or a tool.
 */
export interface ExitFacts {
  /**
   * Classifier denials in a row without an allow or a person's answer.
   */
  denials_in_a_row?: number;
  /**
   * Classifier denials in this turn.
   */
  denials_this_turn?: number;
  /**
   * For a `desktop_ipc` exit: true when every desktop command of the line only reads
   * the desktop's state or reloads its config, such as `hyprctl monitors`.
   */
  desktop_read?: boolean;
  /**
   * For an `outside` or `upload` exit of a git or gh line: the git code that the line
   * runs outside the sandbox, when it made the exit one that only the user may
   * approve: efr could not check it, or a sandboxed command can write a part of it.
   * Absent for any other exit, so a clean check is never a reason to allow. It holds
   * only the parts that a sandboxed command can write.
   */
  git_code?: GitCodeFacts | null;
  /**
   * For a destructive exit: the counts of the hardened `git status`.
   */
  git_status?: GitCounts | null;
  /**
   * The hosts the action reaches.
   */
  hosts?: HostFact[];
  /**
   * For an `outside` exit: the commands of the line that run outside the sandbox,
   * such as `df -h`, in the order of the line.
   */
  outside_commands?: string[];
  /**
   * The earlier exits of this turn and how each was judged: each a list of two, the
   * kind and then the verdict.
   */
  previous_exits_this_turn?: [ExitKind | Verdict, ExitKind | Verdict][];
  /**
   * The programs of the line.
   */
  programs?: ProgramFact[];
  /**
   * Floor refusals in a row without a person's answer.
   */
  refusals_in_a_row?: number;
  /**
   * True when a git setting or another file that runs code changed in this turn.
   */
  repo_surface_changed_this_turn?: boolean;
  /**
   * The names, never the values, that sandboxed calls exported.
   */
  sandbox_export_names?: string[];
  /**
   * True when efr's snapshot or its home store covers every target of the exit, so
   * `efr undo` can bring them back: the AND over the exit's needs. False is a higher
   * risk. Absent when no need has a target.
   */
  snapshot_covers?: boolean | null;
  /**
   * The paths the action writes or reads.
   */
  targets?: TargetFact[];
  /**
   * The upload patterns that the line matched, such as `git push`.
   */
  upload_patterns?: string[];
}
/**
 * The git code that a git or gh line runs outside the sandbox: the hooks, the config
 * keys that run a program, and the programs that they run, of the repository and of
 * each submodule that the line can reach.
 */
export interface GitCodeFacts {
  /**
   * False when efr could not check it: git did not answer in time or failed, efr
   * found no repository, or the submodules lie too deep or behind a link.
   */
  checked: boolean;
  /**
   * What git runs, each once.
   */
  items?: GitCodeFact[];
}
/**
 * One part of the git code of a line.
 */
export interface GitCodeFact {
  /**
   * The file that holds it, or the program that runs.
   */
  file: string;
  /**
   * What runs, such as `core.sshCommand`, `hooks/pre-push` or `ssh (PATH)`.
   */
  item: string;
  /**
   * The repository: its work tree, or its git dir (also the git dir of a submodule).
   */
  repo: string;
  /**
   * True when a contained call can write the file: it lies in a registered project,
   * in the `$SCRATCH` of a conversation, below a write rule or a write root of the
   * config, or in a write root of a running contained job, and no floor of the
   * sandbox keeps it read-only.
   */
  writable: boolean;
}
/**
 * The counts of a hardened `git status`, never the names.
 */
export interface GitCounts {
  /**
   * Modified tracked files.
   */
  modified: number;
  /**
   * Staged changes.
   */
  staged: number;
  /**
   * Untracked files.
   */
  untracked: number;
}
/**
 * A host that an exit reaches.
 */
export interface HostFact {
  /**
   * The host, in lower case.
   */
  host: string;
  /**
   * True when a user message names it.
   */
  named_in_user_messages: boolean;
  /**
   * True when the proxy's allow list holds it (phase D).
   */
  on_allow_list: boolean;
  /**
   * True when the host is the host of a remote of a pinned project git dir whose
   * URLs did not change since the conversation's first call (phase D2).
   */
  project_remote?: boolean;
  /**
   * True when the proxy refused it in this conversation's last call (phase D).
   */
  refused_by_proxy: boolean;
}
/**
 * A program word of the line.
 */
export interface ProgramFact {
  /**
   * True when the program changed in this turn.
   */
  changed_this_turn: boolean;
  /**
   * True when the program lies in a write root.
   */
  in_write_root: boolean;
  /**
   * Where it resolves: the absolute path of a file, or `builtin`, a relative path,
   * for a word that the shell runs itself, such as `cd` or `:`. Absent when it does
   * not resolve.
   */
  resolved?: string | null;
  /**
   * The word as the line has it.
   */
  word: string;
}
/**
 * A path that an exit writes or reads.
 */
export interface TargetFact {
  /**
   * True when efr copies the file into its home store before the call writes it, so
   * `efr undo` can bring it back.
   */
  backup_before_write?: boolean;
  /**
   * The engine's class of the path.
   */
  class?: PathClassName | null;
  /**
   * True when the path exists.
   */
  exists: boolean;
  /**
   * True when the path is a floor.
   */
  floor: boolean;
  /**
   * True when the path lies in a write root.
   */
  in_write_root: boolean;
  /**
   * True when a contained call reads the path as empty: a credential store, a
   * browser profile or a history (the sandbox masks). The config of some of these
   * starts a program later, such as `credential_process` in `~/.aws/config`.
   */
  masked?: boolean;
  /**
   * True when a user message names the path: its `~` form, its absolute form or its
   * last two components.
   */
  named_in_user_messages: boolean;
  /**
   * The path, absolute.
   */
  path: string;
  /**
   * True when the path lies in a synced folder.
   */
  synced: boolean;
}
/**
 * A file that the turn changed and that runs code later outside the sandbox, for the
 * turn-end report.
 */
export interface ReportedFile {
  /**
   * What runs, such as `scripts` or `build.rustc-wrapper`.
   */
  detail?: string | null;
  /**
   * The file, relative to its write root when it lies in one.
   */
  path: string;
}
/**
 * One compaction of a conversation's context, as `conversation_compacted` records it.
 *
 * The compaction covers the model's history up to a cut: every message of the turns
 * before `through_turn`, and the messages of `through_turn` before `through_message`
 * (all of them when `through_message` is absent). The messages after the cut stay
 * verbatim. With a `summary`, the summary replaces everything before the cut. Without
 * one (only pruning ran), every tool result before the cut whose output is longer
 * than the stub reads the stub, and the summary of an earlier compaction stays.
 */
export interface Compaction {
  /**
   * The compaction.
   */
  compaction_id: CompactionId;
  /**
   * What the user asked the summary to keep, for a manual compaction that named it.
   */
  focus?: string | null;
  /**
   * The fresh context block that every request after this compaction sends before
   * the summary: what efrd read from disk when the compaction ended, such as the
   * directories, the git status and the `AGENTS.md` files. Stored, so a request after
   * a restart sends the same bytes. Absent without a summary, and in a compaction
   * from before the field; efrd then reads the block from disk again.
   */
  fresh?: string | null;
  /**
   * The turns that the verbatim tail keeps, in full or in part.
   */
  kept_turns: number;
  /**
   * The tokens at which a client shows 100%, as [`ContextUse::limit`].
   */
  limit: number;
  /**
   * The model that wrote the summary: the model of the conversation's turn.
   */
  model: string;
  /**
   * The oldest messages before the cut that the summary request left out, because
   * they did not fit in the model's context, so the summary never saw them. Absent
   * when none.
   */
  omitted_messages?: number;
  /**
   * The earlier turns that the history had left out before the compaction (the
   * safety net of the history limits), so the summary never saw them. Absent when
   * none.
   */
  omitted_turns?: number;
  /**
   * The tool results whose output the pruning replaced with the stub. Absent when
   * none.
   */
  pruned_outputs?: number;
  /**
   * The estimated tokens that the pruning freed. Absent when none.
   */
  pruned_tokens?: number;
  /**
   * The summary that replaces the history before the cut. Absent when pruning alone
   * freed enough room.
   */
  summary?: string | null;
  /**
   * When present, the compaction covers only the messages of `through_turn` before
   * this index (0 is the prompt), and the verbatim tail starts at this message of the
   * same turn. A cut never falls between a tool call and its result. Absent: the
   * compaction covers the whole turn, and the tail starts with the next turn.
   */
  through_message?: number | null;
  /**
   * The newest turn whose messages the compaction covers, in full or in part.
   */
  through_turn: TurnId;
  /**
   * The estimated tokens in the context after the compaction.
   */
  tokens_after: number;
  /**
   * The tokens in the context before the compaction: the newest real count plus the
   * estimate of what came after it, or the size that the provider refused.
   */
  tokens_before: number;
  /**
   * What started it.
   */
  trigger: CompactionTrigger;
  /**
   * The turn that ran it, for an auto or an overflow compaction inside a turn. Absent
   * for a manual compaction, which runs between turns.
   */
  turn_id?: TurnId | null;
  /**
   * The tokens that the summary request used, when the provider reported them. Its
   * `output_tokens` is the size of the summary.
   */
  usage?: Usage | null;
  /**
   * The context window of that model, in tokens.
   */
  window: number;
}
/**
 * One ref of a project that an undo moves, such as a branch that the turn moved.
 */
export interface RefChange {
  /**
   * True when undo moved the ref, false when it did not. Absent in a dry run.
   */
  applied?: boolean | null;
  /**
   * The short id that the ref has now. Absent: the ref does not exist now.
   */
  from?: string | null;
  /**
   * The ref, such as `main`, `HEAD` or `refs/stash`.
   */
  name: string;
  /**
   * Why undo did not move it. Absent when it did, or will.
   */
  reason?: UndoReason | null;
  /**
   * The root of the ref, as a client shows it.
   */
  root: string;
  /**
   * The short id that undo gives it. Absent: undo removes the ref.
   */
  to?: string | null;
}
/**
 * One job, as `jobs.list`, `jobs.stop` and `job_started` show it.
 */
export interface JobInfo {
  /**
   * The call that started the job.
   */
  call_id: CallId;
  /**
   * How efrd holds the processes of the job (v0.0.5): in a cgroup of the job's own,
   * with its memory and CPU, or by the job's session only, with the reason. Absent
   * from an efrd before v0.0.5.
   */
  cgroup?: JobCgroup | null;
  /**
   * The command line, as the model wrote it.
   */
  command: string;
  /**
   * The conversation.
   */
  conversation_id: ConversationId;
  /**
   * When the job ended. Absent while it runs.
   */
  ended_at?: string | null;
  /**
   * When the job's time limit ends it, if it still runs then.
   */
  ends_by: string;
  /**
   * The exit status of the job's command, when it exited. Absent otherwise.
   */
  exit_code?: number | null;
  /**
   * The ports that efr forwards for the job. Absent when none.
   */
  forwards?: PortForward[];
  /**
   * The job's id in its conversation.
   */
  id: JobId;
  /**
   * How the job runs: in the sandbox with its grants, or in the exit child.
   */
  launch: Launch;
  /**
   * The `shell` call that the user moved into this job with Ctrl+B. Absent for a job
   * that the model started.
   */
  moved_from?: MovedCall | null;
  /**
   * The output file, `$SCRATCH/jobs/<id>.out`, as a path on this machine.
   */
  output: string;
  /**
   * The bytes that the job wrote to its output so far, also the ones that the file
   * left out in the middle.
   */
  output_bytes: number;
  /**
   * When the job started.
   */
  started_at: string;
  /**
   * Where the job is in its life.
   */
  state: JobState;
  /**
   * Who or what stopped the job. Absent for a job that runs or ended by itself.
   */
  stopped_by?: JobStopBy | null;
  /**
   * The turn whose call started the job.
   */
  turn_id: TurnId;
  /**
   * True while the command of a running job that the user moved waits for input that
   * is not a secret, such as `[Y/n]`: nobody can type it in a job (DS27). Absent when
   * false.
   */
  waits_input?: boolean;
  /**
   * How many lines of the output matched the watch pattern so far.
   */
  wakes?: number;
  /**
   * The job's watch pattern. Absent when it has none.
   */
  watch?: Watch | null;
}
/**
 * The cgroup of a job (v0.0.5): efrd makes one for each job below its own delegated
 * cgroup, so the stop of the job kills every process of the job, also one that left its
 * session, and the memory and the CPU time of the job can be seen.
 */
export interface JobCgroup {
  /**
   * The CPU time of the job's processes so far, in microseconds (`usage_usec` of
   * `cpu.stat`). Absent when the kernel does not say.
   */
  cpu_usec?: number | null;
  /**
   * The memory of the job's processes in bytes: what they use now while the job
   * runs (`memory.current`), the most that they used at its end (`memory.peak`).
   * Absent when the kernel does not say.
   */
  memory_bytes?: number | null;
  /**
   * Why the job has no cgroup of its own, and efrd holds its processes by its
   * session only, such as `efrd's cgroup is not delegated (Delegate=yes)`. Absent
   * when the job has its cgroup.
   */
  off?: string | null;
}
/**
 * A loopback port of a contained job that efr forwards to the same port of
 * `127.0.0.1` on the user's machine.
 */
export interface PortForward {
  /**
   * The address that the job listens on, as text: `127.0.0.1`, `::1`.
   */
  listen: string;
  /**
   * The port, the same in the job and on the user's machine.
   */
  port: number;
}
/**
 * The `shell` call that a job came from, when the user moved a running call into a job
 * (`call.background`). The job's `turn_id` and `call_id` name the same call.
 */
export interface MovedCall {
  /**
   * How long the call ran before the move, in seconds. The job's time limit counts
   * from the move.
   */
  after_secs: number;
  /**
   * The call that the user moved.
   */
  call_id: CallId;
  /**
   * The exits of the job decision that the user's answer to the move covered. Absent
   * when none.
   */
  exits?: ExitInfo[];
  /**
   * True when the call ran through efr's sandbox launcher, in the sandbox or in the
   * exit child; false for a line typed into the hidden shell, which has no launcher
   * to lose. Absent when false.
   */
  launcher?: boolean;
  /**
   * The turn of that call.
   */
  turn_id: TurnId;
}
/**
 * A pattern that wakes the model when a line of the job's output matches it.
 */
export interface Watch {
  /**
   * How many matches may wake the model, after efr's own limit
   * (`jobs.max_wakes_per_watch`).
   */
  max_wakes: number;
  /**
   * The pattern, a regular expression of at most
   * [`MAX_PATTERN_CHARS`](Self::MAX_PATTERN_CHARS) characters.
   */
  pattern: string;
}
/**
 * An event with its place in the log.
 */
export interface EventEnvelope {
  /**
   * When the daemon recorded the event.
   */
  at: string;
  /**
   * The conversation the event belongs to; absent for daemon-wide events such as
   * [`Event::LoginCompleted`] and [`Event::LogoutCompleted`].
   */
  conversation_id?: ConversationId | null;
  /**
   * The event.
   */
  event: Event;
  /**
   * The event's global sequence number.
   */
  seq: Seq;
}
/**
 * The params type of each method, by wire name.
 */
export interface MethodParams {
  "admin.config_reload": AdminConfigReload;
  "admin.login_api_key": AdminLoginApiKey;
  "admin.login_openai": AdminLoginOpenAi;
  "admin.logout": AdminLogout;
  "admin.project_add": AdminProjectAdd;
  "admin.project_remove": AdminProjectRemove;
  "admin.sandbox_check": AdminSandboxCheck;
  "admin.status": AdminStatus;
  "approval.respond": ApprovalRespond;
  "call.background": CallBackground;
  "conversation.compact": ConversationCompact;
  "conversation.context": ConversationContext;
  "conversation.delete": ConversationDelete;
  "conversation.diff": ConversationDiff;
  "conversation.history": ConversationHistory;
  "conversation.resume": ConversationResume;
  "conversation.search": ConversationSearch;
  "conversation.subscribe": ConversationSubscribe;
  "conversations.list": ConversationsList;
  "conversations.recent": ConversationsRecent;
  "conversations.search": ConversationsSearch;
  "device.enroll": DeviceEnroll;
  "device.list": DeviceList;
  "device.pair_confirm": DevicePairConfirm;
  "device.pair_start": DevicePairStart;
  "device.prove": DeviceProve;
  "device.revoke": DeviceRevoke;
  hello: Hello;
  "input.respond": InputRespond;
  "jobs.list": JobsList;
  "jobs.stop": JobsStop;
  "lease.report": LeaseReport;
  "models.list": ModelsList;
  "presence.set": PresenceSet;
  "projects.list": ProjectsList;
  "prompt.send": PromptSend;
  "prompt.withdraw": PromptWithdraw;
  "pty.attach": PtyAttach;
  "pty.resize": PtyResize;
  "pty.write": PtyWrite;
  "sandbox.explain": SandboxExplain;
  "sandbox.surface_respond": SandboxSurfaceRespond;
  "shell.interrupt": ShellInterrupt;
  "shell.status": ShellStatus;
  "turn.interrupt": TurnInterrupt;
  "turn.steer": TurnSteer;
  "turn.undo": TurnUndo;
}
/**
 * The result type of each method, by wire name; for a streaming method, the type of each item.
 */
export interface MethodResults {
  "admin.config_reload": AdminConfigReloadResult;
  "admin.login_api_key": AdminLoginApiKeyResult;
  "admin.login_openai": AdminLoginOpenAiItem;
  "admin.logout": AdminLogoutResult;
  "admin.project_add": AdminProjectAddResult;
  "admin.project_remove": AdminProjectRemoveResult;
  "admin.sandbox_check": AdminSandboxCheckResult;
  "admin.status": AdminStatusResult;
  "approval.respond": ApprovalRespondResult;
  "call.background": CallBackgroundResult;
  "conversation.compact": ConversationCompactResult;
  "conversation.context": ConversationContextResult;
  "conversation.delete": ConversationDeleteResult;
  "conversation.diff": ConversationDiffResult;
  "conversation.history": ConversationHistoryResult;
  "conversation.resume": ConversationResumeResult;
  "conversation.search": ConversationSearchResult;
  "conversation.subscribe": ConversationSubscribeItem;
  "conversations.list": ConversationsListResult;
  "conversations.recent": ConversationsRecentResult;
  "conversations.search": ConversationsSearchResult;
  "device.enroll": DeviceEnrollResult;
  "device.list": DeviceListResult;
  "device.pair_confirm": DevicePairConfirmResult;
  "device.pair_start": DevicePairStartItem;
  "device.prove": DeviceProveResult;
  "device.revoke": DeviceRevokeResult;
  hello: HelloResult;
  "input.respond": InputRespondResult;
  "jobs.list": JobsListResult;
  "jobs.stop": JobsStopResult;
  "lease.report": LeaseReportResult;
  "models.list": ModelsListResult;
  "presence.set": PresenceSetResult;
  "projects.list": ProjectsListResult;
  "prompt.send": PromptSendResult;
  "prompt.withdraw": PromptWithdrawResult;
  "pty.attach": PtyAttachItem;
  "pty.resize": PtyResizeResult;
  "pty.write": PtyWriteResult;
  "sandbox.explain": SandboxExplainResult;
  "sandbox.surface_respond": SandboxSurfaceRespondResult;
  "shell.interrupt": ShellInterruptResult;
  "shell.status": ShellStatusResult;
  "turn.interrupt": TurnInterruptResult;
  "turn.steer": TurnSteerResult;
  "turn.undo": TurnUndoResult;
}
/**
 * The result of `admin.config_reload`.
 */
export interface AdminConfigReloadResult {
  /**
   * True when the file was valid and its settings now apply to new turns. False when
   * it has an error; the old settings stay.
   */
  applied: boolean;
  /**
   * What is wrong with the file, when `applied` is false.
   */
  error?: ConfigFileError | null;
  /**
   * The dotted keys whose new values apply only after efrd restarts, such as
   * `screen`. The daemon keeps their old values until then.
   */
  restart_needed?: string[];
  /**
   * What the reload changed that the user should know, one sentence each, such as a
   * `[model] name` of another company than `[model] provider`, an always rule that a
   * floor disables, or a bus filter that is now off. Each warning comes once: a
   * reload that changes nothing of it warns no more. A change that the config hold
   * keeps back is in `error`, not here. Absent when there is none, and from an
   * earlier daemon.
   */
  warnings?: string[];
}
/**
 * What is wrong with `config.toml`, as `admin.status` and `admin.config_reload` report
 * it. The daemon keeps its old settings until the file is valid again.
 */
export interface ConfigFileError {
  /**
   * The column of the error, counted from 1, when the error has a place in the file.
   */
  column?: number | null;
  /**
   * The dotted path of the key that holds the error, such as `model.effort` or
   * `permissions.rules[2].effect`, when the error belongs to one key.
   */
  key?: string | null;
  /**
   * The line of the error, counted from 1, when the error has a place in the file.
   */
  line?: number | null;
  /**
   * What is wrong, in one sentence.
   */
  message: string;
}
/**
 * The result of `admin.login_api_key`.
 */
export interface AdminLoginApiKeyResult {
  /**
   * True when the provider is the one of new conversations, which use the key at
   * once. Another provider needs `[model] provider` and a restart of the daemon.
   */
  active: boolean;
  /**
   * True when the provider accepted the key; false when the check was skipped.
   */
  checked: boolean;
  /**
   * What a client may show of the stored key: its known prefix and its last four
   * characters, such as `sk-ant-...a1b2`. Never the key.
   */
  key_hint: string;
  /**
   * The provider that is now logged in.
   */
  provider: string;
}
/**
 * The result of `admin.logout`.
 */
export interface AdminLogoutResult {
  /**
   * True when the provider is the one of new conversations (`[model] provider`):
   * its turns fail until a new login. False when absent, as from an earlier daemon.
   */
  active?: boolean;
  /**
   * True when credentials were stored and are now deleted; false when there were
   * none.
   */
  logged_out: boolean;
  /**
   * The provider.
   */
  provider: string;
}
/**
 * The result of `admin.project_add`.
 */
export interface AdminProjectAddResult {
  /**
   * The file that was written: `projects.toml`, or the file behind it when it is a
   * symbolic link.
   */
  file: string;
  /**
   * The project as registered.
   */
  project: ProjectInfo;
  /**
   * The reload after the write. When `config.toml` has an error, the daemon keeps its
   * old settings and its old engine, and the project counts from the next reload
   * that succeeds; a turn's scope already sees it.
   */
  reload: AdminConfigReloadResult;
}
/**
 * One registered project.
 */
export interface ProjectInfo {
  /**
   * The id that `Scope::Project` carries.
   */
  id: ProjectId;
  /**
   * A name for people, when the registry gives one.
   */
  name?: string | null;
  /**
   * The project's root directory: absolute, in lexical normal form.
   */
  root: string;
}
/**
 * The result of `admin.project_remove`.
 */
export interface AdminProjectRemoveResult {
  /**
   * The file that was written: `projects.toml`, or the file behind it when it is a
   * symbolic link.
   */
  file: string;
  /**
   * The project that was removed.
   */
  project: ProjectInfo;
  /**
   * The reload after the write, as for `admin.project_add`.
   */
  reload: AdminConfigReloadResult;
}
/**
 * The result of `admin.sandbox_check`: the new status and every check behind it.
 */
export interface AdminSandboxCheckResult {
  /**
   * Each check with its outcome and, for a failure, its fix.
   */
  checks: SandboxCheck[];
  /**
   * The cost of one launch (bubblewrap and Landlock, no shell), in microseconds.
   */
  launch_us?: number | null;
  /**
   * The cost of one launch with the user's shell snapshot, in microseconds.
   */
  snapshot_launch_us?: number | null;
  /**
   * The status after this probe; the daemon uses it from now on.
   */
  status: SandboxStatus;
}
/**
 * One check of the probe, as `efr sandbox check` prints it.
 */
export interface SandboxCheck {
  /**
   * What it found, such as `Landlock ABI 10, errata 0xf`.
   */
  detail?: string | null;
  /**
   * What fixes a failure.
   */
  fix?: string | null;
  /**
   * The check, such as `landlock` or `bwrap`.
   */
  name: string;
  /**
   * How it went.
   */
  outcome: CheckOutcome;
}
/**
 * The result of the sandbox probe, which efrd runs at start, after a reload that
 * changes `[sandbox]`, after a failed launch, and before an `auto` turn when the last
 * probe failed. `auto` requires `available`; without it a turn runs as `cautious`.
 */
export interface SandboxStatus {
  /**
   * True when every check passed.
   */
  available: boolean;
  /**
   * True when efrd runs the filtered bus proxy: the program exists, its self-test
   * passed and `dbus.proxy` is true (phase E). False when absent.
   */
  bus_proxy?: boolean;
  /**
   * Why the bus filter is off, when it is (phase E). Absent when it is on, and from
   * an earlier daemon.
   */
  bus_proxy_off?: BusProxyOff | null;
  /**
   * The bubblewrap program.
   */
  bwrap?: string | null;
  /**
   * Its version, such as `0.13.0`.
   */
  bwrap_version?: string | null;
  /**
   * How the caches are mounted; the probe picks `tmp` when an overlay fails.
   */
  cache_mode: CacheMode;
  /**
   * The bit mask of Landlock errata that the kernel reports fixed.
   */
  errata?: number | null;
  /**
   * What fixes it, such as `Arch: pacman -S bubblewrap`.
   */
  fix?: string | null;
  /**
   * The kernel's Landlock ABI; absent when Landlock is off.
   */
  landlock_abi?: number | null;
  /**
   * True when the probe's network view self-test gave every expected refusal, so a
   * contained call can get `Grant::NetView`. False before phase D1 and when a check
   * failed.
   */
  netview?: boolean;
  /**
   * True when the user's `network.netview` is false, so no call gets the network
   * view, whatever the self-test found. efrd sets it when it sends the status; the
   * probe leaves it false.
   */
  netview_off?: boolean;
  /**
   * What network a contained call has.
   */
  network_mode: NetworkMode;
  /**
   * True when the probe's proxy self-test gave every expected result, so a contained
   * call can reach the network through efr's proxy (phase D2). False when absent.
   */
  proxy?: boolean;
  /**
   * Why the sandbox is unavailable, in one sentence, such as `bubblewrap is not
   * installed`.
   */
  reason?: string | null;
  /**
   * Things that work but need the user's attention, one sentence each.
   */
  warnings?: string[];
}
/**
 * The result of `admin.status`.
 */
export interface AdminStatusResult {
  /**
   * Where the model catalog of the active provider came from. Absent when the
   * daemon does not report it, as before the catalog came from the backend.
   */
  catalog?: CatalogStatus | null;
  /**
   * The config file that the daemon reads and the state of its last reload. Absent
   * when the daemon does not report it, as before live reload.
   */
  config?: ConfigStatus | null;
  /**
   * Conversations with a live actor.
   */
  conversations: number;
  /**
   * The daemon's identity.
   */
  daemon_id: DaemonId;
  /**
   * Whether a turn may run in full mode. Absent when the daemon does not report it,
   * as before full mode.
   */
  full_mode?: FullModeStatus | null;
  /**
   * The daemon's process id.
   */
  pid: number;
  /**
   * The daemon's [`PROTOCOL_VERSION`](crate::PROTOCOL_VERSION).
   */
  protocol: number;
  /**
   * The configured model providers.
   */
  providers: ProviderStatus[];
  /**
   * The daemon's four root directories and where each came from, so a client can
   * warn when its own differ. Absent when the daemon does not report them, as before
   * `efr paths`.
   */
  roots?: DaemonRoots | null;
  /**
   * The sandbox of the `auto` mode, as the last probe found it. Absent when the
   * daemon does not report it, as before the sandbox.
   */
  sandbox?: SandboxStatus | null;
  /**
   * Where the sandbox keeps its launcher and its files, for `efr paths`. Absent when
   * the daemon does not report it.
   */
  sandbox_paths?: SandboxPaths | null;
  /**
   * The screen backend in use: `vt100` or `ghostty`.
   */
  screen_backend: string;
  /**
   * Hidden shells that are running.
   */
  shells: number;
  /**
   * When the daemon started.
   */
  started_at: string;
  /**
   * The daemon's build version.
   */
  version: string;
}
/**
 * Where the daemon's model catalog came from, and when.
 */
export interface CatalogStatus {
  /**
   * When the backend last sent the list or said that it did not change. Absent for
   * the list built into efr.
   */
  fetched_at?: string | null;
  /**
   * Where the list came from.
   */
  origin: CatalogOrigin;
  /**
   * The provider whose catalog it is, such as `openai-subscription` or
   * `anthropic-api`. Absent when the daemon does not report it, as before a daemon
   * could run more than one kind of provider.
   */
  provider?: string | null;
}
/**
 * The config file that the daemon reads, and the state of its last reload.
 */
export interface ConfigStatus {
  /**
   * True when `path` names a file, directly or through a symlink. A missing file is
   * an empty config: every value is its default.
   */
  exists: boolean;
  /**
   * A change of the file that the daemon did not load because it loosens
   * permissions and came while full mode ran. Absent once a reload applies the file
   * or the file no longer loosens.
   */
  held?: HeldConfig | null;
  /**
   * `config.toml` in the daemon's config root.
   */
  path: string;
  /**
   * What was wrong with the file at the last reload, which kept the old settings.
   * Absent once the file loads again.
   */
  reload_error?: ConfigFileError | null;
  /**
   * The dotted keys whose new values in the file apply only after efrd restarts,
   * such as `screen`.
   */
  restart_needed?: string[];
  /**
   * The file that `path` points to when it is a symlink, as an absolute path, also
   * when that file is missing.
   */
  symlink_target?: string | null;
}
/**
 * A change of `config.toml` that the daemon holds back.
 */
export interface HeldConfig {
  /**
   * The config keys whose change loosens permissions, such as `permissions.rules`.
   */
  loosens: string[];
}
/**
 * Whether a turn may run in full mode.
 */
export interface FullModeStatus {
  /**
   * `permissions.allow_full` of the daemon's config.
   */
  allowed: boolean;
  /**
   * Why the daemon refuses full mode although it is allowed. Absent when it does
   * not refuse it.
   */
  refused?: FullRefusal | null;
}
/**
 * The state of one model provider.
 */
export interface ProviderStatus {
  /**
   * True for the provider of new conversations, `[model] provider` of the running
   * daemon. False when absent, as from an earlier daemon.
   */
  active?: boolean;
  /**
   * When the current access token expires, for a provider with refreshable tokens.
   */
  expires_at?: string | null;
  /**
   * For an API key: when the provider last answered a request with the key, refused
   * or not, since the daemon started. Absent before the first such answer, after a
   * login that did not check the key, and from an earlier daemon. With neither this
   * nor `key_refused_at`, the daemon does not know whether the provider takes the
   * key.
   */
  key_checked_at?: string | null;
  /**
   * For an API key: its known prefix and its last four characters, such as
   * `sk-ant-...a1b2`. Never the key.
   */
  key_hint?: string | null;
  /**
   * For an API key: when the provider last refused it (a 401 on a model call or on
   * the fetch of the model list). Absent while the key works, after a new login, and
   * from an earlier daemon. A request with the key that works again removes it.
   */
  key_refused_at?: string | null;
  /**
   * True when credentials are stored.
   */
  logged_in: boolean;
  /**
   * How the provider is logged in. Absent when it is not, or from an earlier daemon.
   */
  login?: LoginKind | null;
  /**
   * The provider, such as `openai-subscription`.
   */
  provider: string;
}
/**
 * The daemon's four root directories.
 */
export interface DaemonRoots {
  /**
   * The config root, which holds `config.toml` and `projects.toml`.
   */
  config: RootDir;
  /**
   * The data root: the database, recordings, scratch directories and secrets.
   */
  data: RootDir;
  /**
   * The runtime root: the socket and `daemon.json`.
   */
  runtime: RootDir;
  /**
   * The state root: the logs.
   */
  state: RootDir;
}
/**
 * One root directory and where its path came from.
 */
export interface RootDir {
  /**
   * The directory.
   */
  path: string;
  /**
   * Where the path came from.
   */
  source: RootSource;
}
/**
 * Where the sandbox keeps its files, for `efr paths`.
 */
export interface SandboxPaths {
  /**
   * The launcher that the hidden shells run, `$XDG_RUNTIME_DIR/efr/bin/efr-sbx`.
   */
  launcher?: string | null;
  /**
   * True when the copy's SHA-256 matches its source.
   */
  launcher_sha256_ok?: boolean | null;
  /**
   * The installed launcher it was copied from.
   */
  launcher_source?: string | null;
  /**
   * The sandbox runtime: call dirs, shell snapshots and sandbox state.
   */
  runtime: string;
  /**
   * The sandbox state: private `/tmp`, cache upper layers, quarantine.
   */
  state: string;
}
/**
 * The result of `approval.respond`.
 */
export interface ApprovalRespondResult {
  /**
   * The sequence number of the `approval_resolved` event.
   */
  seq: Seq;
}
/**
 * The result of `conversation.compact`.
 */
export interface ConversationCompactResult {
  /**
   * The compaction, as the event records it.
   */
  compaction: Compaction;
  /**
   * The sequence number of the `conversation_compacted` event.
   */
  seq: Seq;
}
/**
 * The result of `conversation.context`.
 *
 * Every token count in `parts`, `largest` and `estimate` is an estimate: the bytes of
 * the request's JSON divided by 4. The provider's own count (`counted`) differs, mostly
 * for reasoning and images, and a client shows it first.
 */
export interface ConversationContextResult {
  /**
   * Whether auto compaction is on (`compaction.auto`).
   */
  auto: boolean;
  /**
   * The cache use of the conversation's model calls.
   */
  cache: CacheSummary;
  /**
   * The compactions of the conversation.
   */
  compactions: CompactionSummary;
  /**
   * The provider's last real count: the context at the end of the newest turn.
   * Absent when no turn has one since the newest compaction.
   */
  counted?: CountedUse | null;
  /**
   * The estimated tokens of the next request now, without the next prompt and its
   * preamble: the sum of `parts`.
   */
  estimate: number;
  /**
   * The largest request that efrd sends: 95% of the window.
   */
  hard_cap: number;
  /**
   * The largest single items of the history, largest first: at most
   * [`MAX_LARGEST`] of them.
   */
  largest: ContextItem[];
  /**
   * The model that the numbers are for.
   */
  model: string;
  /**
   * What the next request holds, one entry per kind, in the order of
   * [`ContextPartKind::ALL`]. A kind with nothing in it is absent.
   */
  parts: ContextPart[];
  /**
   * True when a turn runs: the history holds its messages up to now.
   */
  running?: boolean;
  /**
   * The context size at which an auto compaction runs.
   */
  trigger: number;
  /**
   * The model's context window, in tokens. When efrd does not know it, the default
   * that efrd counts with.
   */
  window: number;
  /**
   * True when the model list knows the window; false when `window` is the default.
   */
  window_known: boolean;
}
/**
 * The cache use of a conversation's model calls.
 */
export interface CacheSummary {
  /**
   * The sum over every turn and every compaction since the conversation started.
   * Its `context_tokens` is absent: a context is no sum.
   */
  conversation: Usage;
  /**
   * The usage of the newest ended turn that reported one. Absent when none did.
   */
  last_turn?: Usage | null;
  /**
   * The turns whose usage is in `conversation`.
   */
  turns: number;
}
/**
 * The compactions of a conversation.
 */
export interface CompactionSummary {
  /**
   * How many compactions the conversation had.
   */
  count: number;
  /**
   * The newest one. Absent when there is none.
   */
  newest?: CompactionBrief | null;
}
/**
 * The facts of one compaction, without its summary.
 */
export interface CompactionBrief {
  /**
   * When efrd recorded it.
   */
  at: string;
  /**
   * The tool outputs that it replaced with the stub. Absent when none.
   */
  pruned_outputs?: number;
  /**
   * True when it wrote a summary; false when pruning alone freed enough room.
   */
  summarized: boolean;
  /**
   * The estimated tokens in the context after it.
   */
  tokens_after: number;
  /**
   * The tokens in the context before it.
   */
  tokens_before: number;
  /**
   * What started it.
   */
  trigger: CompactionTrigger;
}
/**
 * The provider's own count of the context at the end of a turn.
 */
export interface CountedUse {
  /**
   * When the turn ended.
   */
  at: string;
  /**
   * The tokens in the context at the last model call of the turn: its input plus its
   * output.
   */
  tokens: number;
  /**
   * The turn.
   */
  turn_id: TurnId;
}
/**
 * One large item of the history.
 */
export interface ContextItem {
  /**
   * What it is.
   */
  kind: ContextPartKind;
  /**
   * What `efr history` shows for it already, never the text of a message: the first
   * line of the command of a shell call, the path of a file tool, the files of a
   * patch, the id of a job of a notice.
   */
  label?: string | null;
  /**
   * Its estimated tokens.
   */
  tokens: number;
  /**
   * The tool, for a tool call or its output.
   */
  tool?: string | null;
  /**
   * The number of its turn in the conversation, counted from 1. Absent for the
   * summary.
   */
  turn?: number | null;
}
/**
 * The estimated size of one kind of content in the request.
 */
export interface ContextPart {
  /**
   * How many items it has: prompts, preambles, notices, answers, calls or outputs;
   * the tools for `tools`, the messages for `summary`, 1 or 0 for `system`.
   */
  count: number;
  /**
   * The kind.
   */
  kind: ContextPartKind;
  /**
   * Its estimated tokens.
   */
  tokens: number;
}
/**
 * The result of `conversation.diff`.
 */
export interface ConversationDiffResult {
  /**
   * The changed files.
   */
  changes: FileChanges;
  /**
   * The unified diff of every changed file, with the paths of
   * [`FileChange::path`](crate::FileChange::path) after `a/` and `b/`, at most
   * [`MAX_TURN_DIFF_LINES`](crate::MAX_TURN_DIFF_LINES) lines and then a line
   * `... N more lines`. Absent with `stat`.
   */
  diff?: string | null;
  /**
   * The turn that the answer is about.
   */
  turn_id: TurnId;
}
/**
 * The result of `conversation.history`.
 */
export interface ConversationHistoryResult {
  /**
   * One page of events, oldest first.
   */
  events: EventEnvelope[];
  /**
   * The cursor for the page of older events; absent when this page reaches the first
   * event.
   */
  next_cursor?: PageCursor | null;
}
/**
 * The scope of a prompt as a client shows it.
 */
export interface RouteLabel {
  /**
   * The project's root for a project; the working directory of the prompt
   * outside projects.
   */
  dir: string;
  /**
   * The project's name, for a project. Absent outside projects and for a project
   * that is no longer registered.
   */
  name?: string | null;
  /**
   * The scope key.
   */
  scope: RouteScope;
}
/**
 * The result of `conversation.search`.
 */
export interface ConversationSearchResult {
  /**
   * The hits, newest first: at most 12 lines each, and a line of at most 400
   * characters, which ends with `...` when efrd cut it.
   */
  hits: SearchHit[];
  /**
   * Why the search stopped before it saw every match. Absent: it saw every match.
   */
  partial?: SearchPartial | null;
  /**
   * The title with its secrets replaced, and the parts that match. Absent before
   * efrd indexed the conversation, and for a conversation without a title.
   */
  title?: SearchLine | null;
}
/**
 * A place in a conversation that matches the query: one prompt, answer or command.
 */
export interface SearchHit {
  /**
   * When the event happened.
   */
  at: string;
  /**
   * A `conversation.history` cursor whose page ends at this event, so a client can
   * open the conversation at the hit.
   */
  history_cursor: PageCursor;
  /**
   * What the text is. A steer is a prompt.
   */
  kind: SearchKind;
  /**
   * The text: one snippet line in `conversations.search`, whole lines in
   * `conversation.search`. Each line is plain text with no control character but the
   * tab.
   */
  lines: SearchLine[];
  /**
   * The sequence number of the event that holds the text.
   */
  seq: Seq;
  /**
   * The turn of that event. Absent for an event outside a turn.
   */
  turn_id?: TurnId | null;
}
/**
 * One line of text with the parts that match the query.
 */
export interface SearchLine {
  /**
   * The parts of `text` that match, sorted, with no overlap. Absent when none match.
   */
  ranges?: TextRange[];
  /**
   * The text, on one line.
   */
  text: string;
}
/**
 * A half-open range `[start, end)` of Unicode code points in a text, counted from 0.
 * A client cuts a range that ends after its text to the text, and ignores an empty or
 * reversed one.
 */
export interface TextRange {
  /**
   * The code point after the last one of the range.
   */
  end: number;
  /**
   * The first code point of the range.
   */
  start: number;
}
/**
 * A bounded view of a conversation: its summary and its most recent events.
 */
export interface ConversationSnapshot {
  /**
   * The conversation as a list shows it.
   */
  conversation: ConversationSummary;
  /**
   * The most recent events, oldest first.
   */
  events: EventEnvelope[];
  /**
   * The cursor for `conversation.history` to page through the events before
   * `events`; absent when `events` starts at the beginning.
   */
  history_cursor?: PageCursor | null;
  /**
   * The sequence number that the snapshot is complete up to. Live events after it
   * follow.
   */
  hwm: Seq;
  /**
   * What the running turn has so far, when a turn runs: the text of its current
   * message and its reasoning, which `events` can miss. Only a daemon that announces
   * `snapshot_live` sends it.
   */
  live?: LiveTurn | null;
}
/**
 * What a list shows about one conversation.
 */
export interface ConversationSummary {
  /**
   * When the conversation began.
   */
  created_at: string;
  /**
   * The user's working directory at the last prompt.
   */
  cwd?: string | null;
  /**
   * The conversation.
   */
  id: ConversationId;
  /**
   * The sequence number of its last event.
   */
  last_seq: Seq;
  /**
   * The scope of the last turn.
   */
  scope?: Scope | null;
  /**
   * What the conversation is doing now.
   */
  status: ConversationStatus;
  /**
   * A short title, taken from the first prompt until something better exists.
   */
  title?: string | null;
  /**
   * The terminal that holds the conversation, when one does: it is that terminal's
   * conversation for its scope.
   */
  tty?: string | null;
  /**
   * When its last event was recorded.
   */
  updated_at: string;
}
/**
 * The state of a running turn in a [`ConversationSnapshot`]: what a client needs to
 * show the text and the reasoning so far after it joins in the middle of a message.
 *
 * `text` is the whole text of message `index` so far, from its start. Drafts and
 * `assistant_message_updated` events of the same message that follow the snapshot
 * continue it: one whose `offset` is at most `text_bytes` overlaps it, and a client
 * keeps the part past `text_bytes`. `reasoning` is the reasoning of the whole turn so
 * far in the same way, for the reasoning drafts.
 */
export interface LiveTurn {
  /**
   * The sequence number of the last event that the turn recorded before this state,
   * with the meaning that it has in a [`Draft`].
   */
  after_seq: Seq;
  /**
   * The position of the current message among the assistant messages of the turn
   * that have text, from 0, as in [`DraftPart::Text`].
   */
  index: number;
  /**
   * The reasoning of the turn so far; absent when the model sent none.
   */
  reasoning?: string | null;
  /**
   * The title of the newest reasoning section, as in [`DraftPart::Reasoning`];
   * absent until such a line comes.
   */
  reasoning_title?: string | null;
  /**
   * The text of the current message so far; empty before its first text.
   */
  text: string;
  /**
   * The length of `text` in bytes of UTF-8: the offset at which the next draft or
   * update of message `index` continues.
   */
  text_bytes: number;
  /**
   * The running turn.
   */
  turn_id: TurnId;
}
/**
 * A draft: part of a running turn as it arrives from the model, before the event log
 * has it.
 *
 * Drafts are best effort. The daemon sends at most one draft of each part per
 * `conversation.draft_interval_ms` (the first one at once). When the subscriber's
 * queue is full, it merges the drafts of each part that wait and sends them at the
 * next free place (the capability `drafts_coalesced`), so a slow subscriber gets
 * fewer, larger drafts with no gap. It never closes a subscription because of a draft.
 * The events stay the source of truth: `assistant_message_updated` and
 * `assistant_message_completed` carry the same text at the same offsets.
 */
export interface Draft {
  /**
   * The sequence number of the last event that the turn recorded before it made
   * this draft. Every event of the stream at or below it comes before the draft. An
   * event of the turn with a larger number came after the draft. The daemon does
   * not send a draft that is older than an `assistant_message_completed`, a
   * `tool_call_started` or the end of a turn that it sent on the stream before.
   */
  after_seq: Seq;
  /**
   * What arrived.
   */
  draft: DraftPart;
  /**
   * The turn that the draft belongs to.
   */
  turn_id: TurnId;
}
/**
 * The result of `conversations.list`.
 */
export interface ConversationsListResult {
  /**
   * One page of conversations, the most recently updated first.
   */
  conversations: ConversationSummary[];
  /**
   * The cursor for the next page; absent on the last page.
   */
  next_cursor?: PageCursor | null;
}
/**
 * The result of `conversations.recent`.
 */
export interface ConversationsRecentResult {
  /**
   * The conversations, the ones of the scope of `pwd` first, each part newest first.
   */
  conversations: RecentConversation[];
  /**
   * The current conversation of the terminal: the one of its last prompt, `,new`
   * or resume. Absent without a `tty`, or when the terminal holds none.
   */
  current?: ConversationId | null;
  /**
   * The scope of `pwd`, whose conversations come first. Absent without a `pwd`.
   */
  scope?: RouteLabel | null;
}
/**
 * One conversation of the list of `conversations.recent`.
 */
export interface RecentConversation {
  /**
   * True when it is a conversation of the terminal that asks.
   */
  here?: boolean;
  /**
   * Another open terminal that holds it, such as `/dev/pts/4`: one of its slots
   * holds it while its shell runs, or it follows the conversation. A phone never
   * holds one. Absent when no other terminal holds it.
   */
  open_in?: string | null;
  /**
   * The name of its project. Absent outside projects and for a project that is no
   * longer registered.
   */
  project?: string | null;
  /**
   * Its scope key.
   */
  route: RouteScope;
  /**
   * What a list shows about the conversation: its id, title, status and last
   * activity.
   */
  summary: ConversationSummary;
  /**
   * Its prompt turns: a turn that a job notice started and a prompt that was taken
   * back before it ran do not count.
   */
  turns: number;
}
/**
 * The result of `conversations.search`.
 */
export interface ConversationsSearchResult {
  /**
   * The current conversation of the terminal, as in `conversations.recent`. Absent
   * without a `tty`, or when the terminal holds none.
   */
  current?: ConversationId | null;
  /**
   * How far efrd has indexed the conversations. Absent: the index is complete.
   */
  index?: SearchIndexState | null;
  /**
   * Why the search stopped before it saw every match. Absent: it saw every match.
   */
  partial?: SearchPartial | null;
  /**
   * The rows, best first. For an empty query: the ones of the scope of `pwd` first,
   * each part newest first.
   */
  rows: SearchRow[];
  /**
   * The scope of `pwd`, whose conversations come first, for an empty query only:
   * the same as in `conversations.recent`. Absent for a query and without a `pwd`.
   */
  scope?: RouteLabel | null;
  /**
   * The measures of the search, when the params asked for them.
   */
  stats?: SearchStats | null;
}
/**
 * How far efrd has indexed the conversations, while it indexes the old ones after an
 * upgrade or a change of what it replaces as secrets.
 */
export interface SearchIndexState {
  /**
   * The part of the event log that the index holds, from 0 to 100.
   */
  percent: number;
}
/**
 * One conversation of the result of `conversations.search`.
 */
export interface SearchRow {
  /**
   * The same members as a row of `conversations.recent`, but `summary.title` always
   * holds the title with its secrets replaced, the text that `title_ranges` point
   * into. Absent title: the conversation has none yet.
   */
  conversation: RecentConversation;
  /**
   * The best hit in the text of the conversation. Absent for an empty query and for
   * a match in the title only.
   */
  hit?: SearchHit | null;
  /**
   * The shortest start of the conversation id, at least 8 characters, that no other
   * conversation of the store shares. `,resume` takes it.
   */
  short_id: string;
  /**
   * The parts of the title that match the query. Absent when none match.
   */
  title_ranges?: TextRange[];
}
/**
 * The measures of one search, for benchmarks on a real store.
 */
export interface SearchStats {
  /**
   * The conversations that those rows belong to.
   */
  conversations_scored: number;
  /**
   * The time of the search in efrd, in microseconds.
   */
  elapsed_us: number;
  /**
   * The rows of the text index that the search read.
   */
  rows_scanned: number;
  /**
   * The titles that the search compared with the query.
   */
  titles_scored: number;
}
/**
 * The result of `device.enroll`.
 */
export interface DeviceEnrollResult {
  /**
   * The new device. The phone keeps it and proves its key with `device.prove` on
   * this connection and on each one after it.
   */
  device_id: DeviceId;
  /**
   * What the device may do once it proved its key.
   */
  scopes: ScopeName[];
}
/**
 * The result of `device.list`.
 */
export interface DeviceListResult {
  /**
   * Every enrolled device, the revoked ones too, the oldest first.
   */
  devices: DeviceInfo[];
}
/**
 * One enrolled device, as `device.list` shows it.
 */
export interface DeviceInfo {
  /**
   * When the terminal confirmed the device.
   */
  created_at: string;
  /**
   * The device.
   */
  device_id: DeviceId;
  /**
   * A short fingerprint of the device's public key, to compare with what the phone
   * shows.
   */
  key_fingerprint: string;
  /**
   * When the device last proved its key; absent when it never connected after it
   * enrolled.
   */
  last_seen_at?: string | null;
  /**
   * The name that the device sent when it enrolled, such as `Anna's iPhone`.
   */
  name: string;
  /**
   * The stable id of the Tailscale node that the device enrolled from. A connection
   * from another node does not prove this device.
   */
  node: string;
  /**
   * When the device was revoked; absent while it may connect.
   */
  revoked_at?: string | null;
  /**
   * What the device may do.
   */
  scopes: ScopeName[];
}
/**
 * The result of `device.pair_confirm`.
 */
export interface DevicePairConfirmResult {
  /**
   * The new device; absent when the answer was no.
   */
  device_id?: DeviceId | null;
}
/**
 * What the pairing QR code holds: where the phone finds the daemon and the one-time
 * token that it sends with `device.enroll`.
 */
export interface PairingQr {
  /**
   * The daemon that the phone must find in its `hello` result before it sends the
   * token.
   */
  daemon_id: DaemonId;
  /**
   * The MagicDNS name of the daemon's node, a fallback for `host`.
   */
  fqdn?: string | null;
  /**
   * The daemon's IPv4 address on the tailnet. Never a loopback address.
   */
  host: string;
  /**
   * The port of the daemon's tailnet listener.
   */
  port: number;
  /**
   * The one-time token, as unpadded base64url. It is good for one enroll and expires
   * after a few minutes.
   */
  token: SecretText;
}
/**
 * The result of `device.prove`.
 */
export interface DeviceProveResult {
  /**
   * What the connection may do now.
   */
  scopes: ScopeName[];
}
/**
 * The result of `device.revoke`.
 */
export interface DeviceRevokeResult {
  /**
   * When the device was revoked: now, or the time of the first revoke.
   */
  revoked_at: string;
}
/**
 * The result of `hello`.
 */
export interface HelloResult {
  /**
   * What the daemon offers on this connection.
   */
  capabilities?: Capabilities;
  /**
   * A fresh random nonce for this connection, as unpadded base64url. A remote device
   * proves its key by signing it together with `daemon_id`, `protocol` and the facts
   * of `prove` (see [`ProveMessage`](crate::ProveMessage)); the Unix socket ignores
   * it.
   */
  challenge: string;
  /**
   * The daemon's identity. Clients pin it and warn when it changes.
   */
  daemon_id: DaemonId;
  /**
   * Where the daemon keeps its files. A connection of the tailnet listener gets
   * empty paths: they name local directories that a remote device has no use for.
   */
  paths: DaemonPaths;
  /**
   * The daemon's [`PROTOCOL_VERSION`](crate::PROTOCOL_VERSION).
   */
  protocol: number;
  /**
   * The Tailscale facts that a remote device signs with `challenge` for
   * `device.prove`. Only a connection of the tailnet listener gets it.
   */
  prove?: ProveContext | null;
  /**
   * The daemon's build version, such as `0.1.0`.
   */
  version: string;
}
/**
 * Directories of the daemon that clients show to the user.
 */
export interface DaemonPaths {
  /**
   * The daemon's data directory: the database, recordings and secrets.
   */
  data_dir: string;
  /**
   * The parent of every conversation's `$SCRATCH` directory.
   */
  scratch_root: string;
}
/**
 * The facts that a remote connection gets with its `hello` result, next to
 * `challenge`. The phone cannot read its own Tailscale node, so the daemon sends what
 * its whois found, and the device signs it.
 *
 * Each value is a Tailscale stable id: a short ASCII string such as `nXXXXCNTRL` that
 * stays the same when the node changes its name or its addresses.
 */
export interface ProveContext {
  /**
   * The stable id of the daemon's own node.
   */
  daemon_node: string;
  /**
   * The stable id of the daemon's tailnet.
   */
  daemon_tailnet: string;
  /**
   * The stable id of the peer node, from the whois of the connection.
   */
  peer_node: string;
}
/**
 * The result of `input.respond`: the line was written to the call's PTY.
 */
export interface InputRespondResult {}
/**
 * The result of `jobs.list`.
 */
export interface JobsListResult {
  /**
   * The jobs, newest first, at most [`MAX_JOBS`](Self::MAX_JOBS). Absent when none.
   */
  jobs?: JobInfo[];
}
/**
 * The result of `jobs.stop`.
 */
export interface JobsStopResult {
  /**
   * The job after the stop. A job that had ended already keeps its final state.
   */
  job: JobInfo;
}
/**
 * The result of `lease.report`.
 */
export interface LeaseReportResult {
  /**
   * How long the lease lasts without another report, in seconds.
   */
  ttl_secs: number;
}
/**
 * The result of `models.list`: the daemon's effective model list, which a prompt's
 * `settings.model` must name.
 */
export interface ModelsListResult {
  /**
   * Where the provider's catalog came from. Absent when the daemon does not report
   * it, as before the catalog came from the backend.
   */
  catalog?: CatalogStatus | null;
  /**
   * Every model: the ones of the provider's catalog first, best priority first, then
   * the ones that the config adds.
   */
  models: ModelInfo[];
}
/**
 * One model of the list.
 */
export interface ModelInfo {
  /**
   * The context window in tokens that efrd uses for the model: the window of the
   * model's entry in `[openai] models`, cut down to `max_context_window`, else the
   * catalog's window. Absent when efr does not know it; efrd then counts with a
   * default window.
   */
  context_window?: number | null;
  /**
   * True for the model that a turn uses when the prompt names none: the config's
   * `[model] name`, else the daemon's default. At most one model has it.
   */
  default?: boolean;
  /**
   * The model's own default effort, when it has one.
   */
  default_effort?: string | null;
  /**
   * The reasoning efforts that the model takes, such as `low`, `medium` and `high`.
   * A prompt's `settings.effort` must be one of them. Empty for a model whose
   * efforts efr does not know.
   */
  efforts?: string[];
  /**
   * The model id, such as `gpt-5.5`.
   */
  id: string;
  /**
   * The largest window in tokens that an entry in `[openai] models` can set for the
   * model, from the catalog. Absent when the catalog does not say.
   */
  max_context_window?: number | null;
  /**
   * True when the backend prefers that a client reach the model over a WebSocket.
   */
  prefer_websockets?: boolean;
  /**
   * Where the model comes from.
   */
  source: ModelSource;
}
/**
 * The result of `presence.set`.
 */
export interface PresenceSetResult {
  /**
   * The conversations whose presence changed, each with one `presence_changed`
   * event; absent when each one had that presence already.
   */
  changed?: ConversationId[];
  /**
   * The presence that the conversations have now.
   */
  presence: Presence;
  /**
   * The sequence number of the last `presence_changed` event; absent when nothing
   * changed.
   */
  seq?: Seq | null;
}
/**
 * The result of `projects.list`: the daemon's project registry as the file holds it
 * now.
 */
export interface ProjectsListResult {
  /**
   * The registry file, `projects.toml` in the config root.
   */
  file: string;
  /**
   * Every registered project, in the order of the file.
   */
  projects: ProjectInfo[];
}
/**
 * The result of `prompt.send`.
 */
export interface PromptSendResult {
  /**
   * The conversation that took the prompt.
   */
  conversation_id: ConversationId;
  /**
   * True when another turn was running, so the prompt waits behind it.
   */
  queued: boolean;
  /**
   * Where a prompt from a terminal went: its scope, and how the terminal's
   * conversation changed. Absent for a prompt with no terminal or with a
   * conversation id, and from a daemon before routing by scope.
   */
  route?: Routed | null;
  /**
   * The sequence number of the `prompt_queued` event. A client that follows the
   * answer subscribes with this as `after_seq`.
   */
  seq: Seq;
  /**
   * The settings that the turn would run with if it started now. A queued turn
   * resolves them again when it starts, so its `turn_started` can differ after the
   * config changed. Absent when the daemon does not report them, as before turn
   * settings.
   */
  settings?: EffectiveSettings | null;
  /**
   * The turn that will answer it.
   */
  turn_id: TurnId;
}
/**
 * Where a prompt from a terminal went, for the line before the reply.
 */
export interface Routed {
  /**
   * A git repository that is not a registered project holds the prompt's working
   * directory. Absent when there is none, or when the hint was shown recently.
   */
  hint?: RepoHint | null;
  /**
   * The scope of the prompt.
   */
  scope: RouteLabel;
  /**
   * How the conversation changed. Absent when the prompt continued the terminal's
   * current conversation, was the terminal's first prompt, or started a new
   * conversation on purpose (`,new`).
   */
  switch?: Switch | null;
}
/**
 * A git repository that is not a registered project, so writes there ask the user.
 */
export interface RepoHint {
  /**
   * The repository's work tree.
   */
  root: string;
}
/**
 * The result of `prompt.withdraw`.
 */
export interface PromptWithdrawResult {
  /**
   * The prompt that the daemon withdrew.
   */
  withdrawn: WithdrawnPrompt;
}
/**
 * A prompt that the daemon took out of the queue before it started.
 */
export interface WithdrawnPrompt {
  /**
   * The sequence number of the `prompt_withdrawn` event.
   */
  seq: Seq;
  /**
   * The prompt text, as `prompt_queued` recorded it.
   */
  text: string;
  /**
   * The turn that would have answered it.
   */
  turn_id: TurnId;
}
/**
 * The state of a terminal screen at one moment.
 */
export interface ScreenSnapshot {
  /**
   * True while a full-screen program uses the alternate screen.
   */
  alternate_screen?: boolean;
  /**
   * The cursor.
   */
  cursor: Cursor;
  /**
   * The visible grid from the top row down.
   */
  rows: RowCells[];
  /**
   * Rows above the visible grid, oldest first, as many as the client asked for.
   */
  scrollback?: RowCells[];
  /**
   * The size of the visible grid.
   */
  size: Size;
  /**
   * The window title that the program set.
   */
  title?: string | null;
}
/**
 * The cursor position in the visible grid, counted from 0 at the top left.
 */
export interface Cursor {
  /**
   * The column, from 0 at the left edge.
   */
  col: number;
  /**
   * True when the program has hidden the cursor.
   */
  hidden?: boolean;
  /**
   * The row, from 0 at the top of the visible grid.
   */
  row: number;
}
/**
 * One row of cells.
 */
export interface RowCells {
  /**
   * The cells from the left edge. Trailing blank cells may be left out.
   */
  cells: Cell[];
  /**
   * True when the row continues on the next row because the line was soft-wrapped.
   */
  wrapped?: boolean;
}
/**
 * One character cell.
 */
export interface Cell {
  /**
   * The background colour; absent means the terminal's default.
   */
  bg?: Color | null;
  /**
   * Bold text.
   */
  bold?: boolean;
  /**
   * The foreground colour; absent means the terminal's default.
   */
  fg?: Color | null;
  /**
   * Foreground and background swapped.
   */
  inverse?: boolean;
  /**
   * Italic text.
   */
  italic?: boolean;
  /**
   * The grapheme in the cell. Empty for a blank cell and for the second cell of a
   * wide character.
   */
  text: string;
  /**
   * Underlined text.
   */
  underline?: boolean;
  /**
   * The first cell of a character that takes two columns.
   */
  wide?: boolean;
}
/**
 * The result of `pty.resize`.
 */
export interface PtyResizeResult {
  /**
   * The size the PTY has now. It differs from the request when the daemon clamps it
   * or another client holds the size.
   */
  size: Size;
}
/**
 * The result of `pty.write`: the bytes were queued for the PTY.
 */
export interface PtyWriteResult {}
/**
 * The result of `sandbox.explain`.
 */
export interface SandboxExplainResult {
  /**
   * The mode the answer is for.
   */
  mode: Mode;
  /**
   * The path, absolute.
   */
  path: string;
  /**
   * The root of the project the answer is for; absent outside a project.
   */
  project?: string | null;
  /**
   * True when a contained call can read the path's content.
   */
  read: boolean;
  /**
   * Why, in one sentence, such as `a shell startup file (floor)`.
   */
  reason: string;
  /**
   * What the sandbox does with the path.
   */
  role: SandboxPathRole;
  /**
   * True when a contained call can write the path.
   */
  write: boolean;
  /**
   * The exit that a write of the path would be, such as `persistence`.
   */
  write_exit?: ExitKind | null;
}
/**
 * The result of `sandbox.surface_respond`.
 */
export interface SandboxSurfaceRespondResult {
  /**
   * The sequence number of the `surface_question_answered` event.
   */
  seq: Seq;
}
/**
 * The result of `shell.interrupt`: the daemon sent the signal, or began to close the
 * shell. The command may still run for a moment; its end comes as an event.
 */
export interface ShellInterruptResult {}
/**
 * The result of `shell.status`. The daemon reads its live state, so a
 * `shell_command_detached` event of a stopped efrd never shows as busy.
 */
export interface ShellStatusResult {
  /**
   * The command that holds the hidden shell. Absent when the shell is free, or when
   * the conversation has no hidden shell.
   */
  busy?: BusyShell | null;
}
/**
 * A command that a `shell` call left running in the hidden shell: the shell takes no
 * other call until it ends.
 */
export interface BusyShell {
  /**
   * Who approved the call. Absent when the call needed no approval.
   */
  approved_by?: ApprovedBy | null;
  /**
   * The call that left the command running.
   */
  call_id: CallId;
  /**
   * The first 200 characters of the command line. The model wrote it, so a client
   * shows it as text that a program wrote, on one line.
   */
  command: string;
  /**
   * True when the command runs in the sandbox. A client never asks the user for a
   * hidden answer, an answer to a wait that looks secret, or a manual answer to a
   * command in the sandbox.
   */
  contained: boolean;
  /**
   * What the command waits for now.
   */
  input: InputWait;
  /**
   * True for a `visible` wait whose prompt line looks like a password prompt. False
   * when absent.
   */
  looks_secret?: boolean;
  /**
   * The row of the screen that holds the cursor while the command waits, at most 200
   * characters. The program wrote it, so a client shows it as text that a program
   * wrote, on one line. Absent when the command does not wait.
   */
  prompt?: string;
  /**
   * When the command started.
   */
  since: string;
}
/**
 * The result of `turn.interrupt`.
 */
export interface TurnInterruptResult {
  /**
   * The prompt that the unread steers of `resend_steers` became. Absent when none
   * of them was unread.
   */
  resent?: ResentSteers | null;
  /**
   * The sequence number of the `turn_interrupt_requested` event.
   */
  seq: Seq;
  /**
   * The turn that is being stopped.
   */
  turn_id: TurnId;
  /**
   * The prompts of `withdraw` that the daemon withdrew, in queue order. Absent when
   * none.
   */
  withdrawn?: WithdrawnPrompt[];
  /**
   * The steers of `withdraw_steers` that the daemon took back, in sequence order.
   * Only these were not read: a client hands back these texts and no others. Absent
   * when none.
   */
  withdrawn_steers?: WithdrawnSteer[];
}
/**
 * The prompt that unread steers became when their turn was interrupted.
 */
export interface ResentSteers {
  /**
   * The sequence number of its `prompt_queued` event.
   */
  seq: Seq;
  /**
   * The `turn_steered` events whose text the prompt carries, in sequence order.
   */
  steers: Seq[];
  /**
   * The new turn that answers the prompt.
   */
  turn_id: TurnId;
}
/**
 * A steer that the daemon took back before a model call read it.
 */
export interface WithdrawnSteer {
  /**
   * The sequence number of its `turn_steered` event.
   */
  seq: Seq;
  /**
   * The text of the steer.
   */
  text: string;
}
/**
 * The result of `turn.steer`.
 */
export interface TurnSteerResult {
  /**
   * True when the steer was late and `if_late` turned it into a prompt. Absent
   * means false.
   */
  queued?: boolean;
  /**
   * The sequence number of the `turn_steered` event. When `queued` is true, of the
   * `prompt_queued` event. A client that shows the steer as unread waits for a
   * `steering_delivered` event that names this number.
   */
  seq: Seq;
  /**
   * The turn that took the guidance. When `queued` is true, the new turn of the
   * prompt that the late steer became.
   */
  turn_id: TurnId;
}
/**
 * The result of `turn.undo`.
 */
export interface TurnUndoResult {
  /**
   * True when the undo changed files or refs and ended; false in a dry run, with
   * `list`, when nothing was to do, when it wrote nothing, and when it stopped.
   */
  applied?: boolean;
  /**
   * The conversation.
   */
  conversation_id: ConversationId;
  /**
   * What the undo does or did with each path, at most
   * [`MAX_UNDO_ENTRIES`](crate::MAX_UNDO_ENTRIES). Absent when none.
   */
  entries?: UndoEntry[];
  /**
   * How many entries `entries` leaves out. Absent when none.
   */
  entries_more?: number;
  /**
   * Notes for the user, each one line: that the git index stays as it is, to run
   * `systemctl --user daemon-reload`, that empty directories and special files do
   * not come back. Absent when none.
   */
  notes?: string[];
  /**
   * The refs that the undo moves or moved. Absent when none.
   */
  refs?: RefChange[];
  /**
   * True when the undo went on with an earlier undo that stopped half way. False
   * when absent.
   */
  resumed?: boolean;
  /**
   * True when a write failed and the undo stopped half way after it changed files
   * or refs; the next `efr undo` finishes it, and `efr undo --redo` takes back what
   * it did. False when absent.
   */
  stopped?: boolean;
  /**
   * The first line of the turn's prompt. Absent when the log has none.
   */
  title?: string | null;
  /**
   * The directory of the undo trash that holds every file that the undo overwrote
   * or removed. Absent when it holds none.
   */
  trash?: string | null;
  /**
   * The turn that the undo takes back, or whose undo a redo takes back. Absent with
   * `list`.
   */
  turn_id?: TurnId | null;
  /**
   * With `list`: the turns that can be undone, newest first. Absent otherwise.
   */
  turns?: UndoableTurn[];
  /**
   * The undo: in a dry run, the id that the apply will take when it resumes an undo
   * that stopped half way. Absent with `list`.
   */
  undo_id?: UndoId | null;
}
/**
 * One path of an undo: what undo does or did with it.
 */
export interface UndoEntry {
  /**
   * What undo does with the path.
   */
  action: UndoAction;
  /**
   * The path as a client shows it: `src/a.rs` (relative to its root), `$SCRATCH/x`,
   * `~/.zshrc`, or a state such as `@crontab`.
   */
  path: string;
  /**
   * Why, for a skip or a path that no snapshot covers. Absent for the other actions.
   */
  reason?: UndoReason | null;
}
/**
 * One turn that `turn.undo` with `list` shows.
 */
export interface UndoableTurn {
  /**
   * True when the turn ran in `auto` or `full`, whose calls copy home files first.
   * False when absent.
   */
  auto?: boolean;
  /**
   * When the turn ended. Absent for a turn that efrd stopped.
   */
  ended?: string | null;
  /**
   * True when the turn ran in `full`; `auto` is true too then. False when absent, as
   * from an efrd before full mode.
   */
  full?: boolean;
  /**
   * True when efrd stopped while the turn ran, so its undo needs `force`. False
   * when absent.
   */
  stopped?: boolean;
  /**
   * The first line of the turn's prompt. Absent when the log has none.
   */
  title?: string | null;
  /**
   * The turn.
   */
  turn_id: TurnId;
  /**
   * True when an undo took the turn back and no redo returned it. False when absent.
   */
  undone?: boolean;
}
/**
 * A result, or one item of a stream: it follows the member of `MethodResults` for the method of the request.
 */
export interface ServerItemFrame {
  id: RequestId;
  item: unknown;
}
/**
 * The request finished.
 */
export interface ServerEndFrame {
  end: true;
  id: RequestId;
}
/**
 * A server frame that ends a request with an error: `{id, error}`.
 *
 * `id` is absent only when the server could not read a request id from the frame it
 * rejects; the server then closes the connection.
 */
export interface ErrorFrame {
  /**
   * What went wrong.
   */
  error: ErrorBody;
  /**
   * The request that failed.
   */
  id?: RequestId | null;
}
/**
 * The daemon accepted a streaming request.
 */
export interface ServerAckFrame {
  ack: RequestId;
}

/** The protocol version of this file, as `hello` sends it. */
export const PROTOCOL_VERSION = 1 satisfies ProtocolVersion;

/** Every event kind of this file, in schema order. */
export const EVENT_KINDS = [
  "conversation_created",
  "terminal_attached",
  "prompt_queued",
  "prompt_withdrawn",
  "prompt_held",
  "turn_started",
  "scope_changed",
  "assistant_message_updated",
  "assistant_message_completed",
  "tool_call_started",
  "tool_call_output_updated",
  "tool_call_input_changed",
  "tool_call_completed",
  "approval_requested",
  "approval_resolved",
  "approval_expired",
  "turn_steered",
  "steering_delivered",
  "steering_withdrawn",
  "turn_interrupt_requested",
  "turn_interrupted",
  "turn_completed",
  "turn_failed",
  "turn_cancelled",
  "shell_started",
  "shell_exited",
  "cwd_changed",
  "login_completed",
  "logout_completed",
  "exit_requested",
  "exit_judged",
  "always_rule_saved",
  "exit_review_started",
  "classifier_paused",
  "presence_changed",
  "cache_delete_applied",
  "home_file_changed",
  "sandbox_surface_changed",
  "surface_question_requested",
  "surface_question_answered",
  "turn_surface_report",
  "conversation_compacted",
  "sandbox_unavailable",
  "snapshot_taken",
  "undo_applied",
  "job_started",
  "job_watch_matched",
  "job_port_forwarded",
  "job_ended",
  "job_notice",
  "job_notice_held",
  "shell_command_detached",
  "shell_command_input_changed",
  "shell_command_ended",
  "conversation_deleted",
] as const satisfies readonly Event["kind"][];

/** Every draft part kind of this file. */
export const DRAFT_PART_KINDS = [
  "text",
  "reasoning",
  "tool_input",
  "context",
  "compacting",
] as const satisfies readonly DraftPart["kind"][];

/** Every item kind of `conversation.subscribe`. */
export const SUBSCRIBE_ITEM_KINDS = [
  "event",
  "snapshot",
  "draft",
] as const satisfies readonly ConversationSubscribeItem["kind"][];

/** Every method name. */
export const METHOD_NAMES = [
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
] as const satisfies readonly MethodName[];

/** Every error code. The set is closed: a new code bumps the protocol version. */
export const ERROR_CODES = [
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
] as const satisfies readonly ErrorCode[];

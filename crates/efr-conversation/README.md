# efr-conversation

## Purpose

The conversation engine: one actor per conversation that queues prompts and runs
turns.

- `ConversationActor::spawn` starts the actor of one conversation and returns its
  `ConversationHandle`, the only way in. The actor reads its settings from a
  `ConfigSource` (the daemon's settings watch; a `watch::Receiver` of
  `ConversationConfig` is one too): a turn takes the latest settings when it starts
  and keeps them until it ends, so a change never reaches a running turn's model,
  prompt or limits; each tool call reads the approval timeout and the update interval
  when it starts; a prompt reads the queue limit when it arrives. `ConversationStart::New` records
  `conversation_created` together with the first prompt, so a conversation never
  exists without one; `ConversationStart::Existing` picks up a conversation from the
  log, as after a restart.
- `send_prompt` records `prompt_queued` with the command's receipt. A prompt runs at
  once when the conversation is idle; a second prompt queues behind the running turn
  (`ConversationConfig::max_queued`, 16 by default). The last command and the recent
  commands of `prompt.send` stay in memory and reach only the turn's preamble; they
  never enter an event.
- Turn settings (`settings.rs`): a prompt may ask for a mode, a model and an effort.
  `send_prompt` resolves them over the config's defaults (`ConversationConfig::mode`,
  `model`, `effort`) and checks them against the model list
  (`ConversationConfig::models`; an empty list takes any model): a model outside the
  list, or an effort the model does not take, is `InvalidSetting` with the choices,
  and nothing is recorded. `prompt_queued` keeps what the prompt asked for, so a queued
  prompt keeps it; the result carries the effective settings. When the turn starts it
  resolves them again against the settings of that moment, fails with `invalid` and
  the choices when they no longer fit, and records them on `turn_started`. A turn that
  names no model of its own fails with `ForeignModel` (`invalid`, with the cause and
  the fix of efrd's refusal of a new prompt) while the config's default model is a
  model of another company than its provider (`ConversationConfig::foreign_model`), so
  a prompt that queued before a reload made the default foreign gets the same text
  (v0.0.4). A turn
  from a remote origin runs with at most `cautious`, and a wake turn with at most
  `auto`: a wake turn of `full` runs as `auto` with a `ModeFallback` whose `asked` is
  `full` and whose reason is `ModeFallback::WAKE_REASON` (plus the reason of `auto`
  when it falls further to `cautious`), also when full mode is off, because the cap
  comes before the gate. After those caps, `full` runs only
  when the daemon allows it (`ConversationConfig::full_mode`): else `FullModeOff`
  (`permissions.allow_full` is false) or `FullModeRefused` with the daemon's
  `FullRefusal`, both `invalid` with the other modes as the choices. A `full` that
  comes from the config default and meets a `FullRefusal` does not fail: the turn
  runs as `auto` (or `cautious` when `auto` cannot run either) with a `ModeFallback`
  whose `asked` is `full` and whose reason starts with `full mode cannot run:`. efrd
  fills `full_mode` from `permissions.allow_full` and its refusals. The turn sends its model, its
  effort as the request's `effort` (in the manual compaction's summary request too),
  and its mode in the permission
  engine's `DecisionInput` and as a line of the preamble.
- `steer` records `turn_steered`. Before its next model call the turn records
  `steering_delivered` with the seqs of the steers it takes, then sends their texts to
  the model. A turn that would end with steering waiting makes one more call. A steer
  is late when no model call of the turn would read it: the turn made its last model
  call and closed its steering, an interrupt was asked for, the steer names another
  turn, or no turn runs. A late steer is never recorded as `turn_steered`. Without
  `if_late` it gets `NoRunningTurn` (or `TurnMismatch`). With `if_late: queue` the
  actor records it as `prompt_queued` in the same step, with the steer's command id,
  the origin of the caller and the context and settings of `if_late`, and the result
  says `queued`. The turn waits for a steer that the actor is recording at that moment
  and reads it, so a steer is either read or late, never lost. A steer from the phone
  into a turn that runs in full mode (or asks for it and has not resolved its mode yet,
  `Control::mode`) is late too: without `if_late` it gets `RemoteFullSteer`
  (`forbidden`), and with `if_late: queue` it runs as a prompt of its own with at most
  `cautious` (`a_phone_steer_into_a_running_full_turn_is_not_read`).
- `interrupt` is two-phase: the actor records `turn_interrupt_requested`, the turn
  drops the provider's stream (or the parked approval, or the running tool call, which
  the toolbox is asked to `cancel`), completes the text that streamed so far, and only
  then records `turn_interrupted`. Esc in the input row of `efr` lists the prompts that
  its view queued (`withdraw`) and its unread steers (`resend_steers`); Ctrl+C lists
  the prompts and the unread steers to take back (`withdraw_steers`). In the step and
  the append that record `turn_interrupt_requested`, the actor withdraws the listed
  prompts that still wait (`prompt_withdrawn` each, in queue order) and takes the
  listed steers that no model call took yet out of the turn's steering. It records
  the steers taken back as one `steering_withdrawn`, and the steers to send again as
  one `prompt_queued` (texts joined by newlines, `steers` set) that runs next, before
  the prompts that wait. That prompt has the context, settings and last command of
  `resend_as` (the terminal that pressed Esc), else those of the interrupted turn's
  prompt. The resend never makes the interrupt fail: its prompt does not count
  against `max_queued`, because the turn took its steers already, and settings of
  `resend_as` that cannot work now give way to the interrupted turn's. Prompts and steers that are not listed stay as they are, so those of other
  terminals stay queued. A steer that the turn took for a model call counts as read,
  also when the interrupt stops that call. The history and the exit record leave out a
  steer that an interrupt sent again or took back.
- `withdraw` takes one queued prompt back, by its turn or as the newest prompt of a
  terminal, and records `prompt_withdrawn`, the last event of that turn. The actor
  starts a queued turn only between requests, so a prompt either starts or is
  withdrawn. A prompt that started, ended or was withdrawn is `PromptNotWaiting`; a turn
  that the conversation never queued is `UnknownTurn`; a terminal with no queued
  prompt is `NoQueuedPrompt`.
- The receipt of a result with sequence numbers inside it (`turn.interrupt`,
  `prompt.withdraw`) is stored without them. The events of a batch have consecutive
  numbers, so `completed_result` puts them back from the receipt's seq.
- The rebuilt history and the user messages of an exit record leave out a steer that
  an interrupt sent again (it counts once, as its prompt) and a withdrawn prompt.
- The rebuilt history puts a steer where the `steering_delivered` that names it is,
  because the model read it at that model call. A steer that no
  `steering_delivered` names is left out: no model call read it (the turn failed, an
  old client interrupted it, or the user took it back). A completed turn without any
  `steering_delivered` comes from a daemon that did not record the event yet, and
  keeps its steers where the user typed them.
- An approval's summary names the tool and what needs approval, the command line
  first. When some simple commands of a line of several ask, a second line names them,
  such as `asks for: hostnamectl, systemctl --failed`: each by its program and at most
  three words after it, cut at a long word or at a word with a quote, a space or
  another character outside letters, digits and `._/:@%+,-`, and with the value of
  `--option=value` left out, so a token on the line is not repeated there. Every other
  line break of the summary is escaped, so a path cannot pass for that line.
- The `auto` mode (`exit.rs`, `questions.rs`, `judge.rs`, `turn/review.rs`). The check
  point runs a shell call that the engine contains (`Effect::Contain`) at once with
  `Launch::Contained` in `CallContext::launch`, and `tool_call_started` names the
  launch. An exit goes to the classifier or to the user. The engine gives each exit an
  approver; `exit::route` finishes one that the classifier judges only when a user
  message names it, with `efr_permissions::exits::names_match` on the messages of the
  naming window (`exit::naming_messages`, v0.0.4): the prompt of the turn, its steers,
  and the prompt and the steers of the user's turn before it, never a turn before the
  newest `terminal_attached` (a pick-up or a resume), read from the whole log
  (`efr_store::user_messages`, never the page of newest events or the cut list of the
  record). The same check fills `named_in_user_messages` in the record, which keeps
  every message of the conversation for the reviewer (`exit::UserMessages`). A file tool's edit of a file that runs code
  later (an exit of the kind `persistence`) puts its payload in the record
  (`ActionFacts::edit`, `exit::Payload`): the new content, the old and the new text, or
  the parts of a patch for that file; a payload over 8 KiB, or one that efr cannot read
  whole, goes to the user. A plain write outside the roots has no payload, also when
  efrd copies it first. The record (`exit::record`) holds user messages, the action and
  efr's own facts, never tool output or the model's reason; a target's class says when
  a cache, the build output of a project or efr's trash can make it again, and whether
  efrd copies it into its home store first. `snapshot_covers` is false when one exit's
  copy is not covered (a masked file, a directory over the limits) and absent when no
  exit has a file to copy. The git code of a git or gh line that runs outside (v0.0.4,
  B4) reaches the record (`ExitFacts::git_code`) only when it made the exit one for
  the user alone, so the reviewer never reads a clean check as a reason to allow, and
  with only the writable parts; the engine's `ExitNeed::notes` (the writable git code, what a publish runs) join the
  question's facts.
- The classifier (`turn/review.rs`) judges first when the turn runs in `auto`,
  `ConversationDeps::judge` is set, `ConversationConfig::classifier_enabled` is on, the
  classifier has not stopped for the turn, every reason that asks is an exit, and every
  exit goes to it. `exit_requested` and `exit_review_started` come in one batch, the
  judge runs while the user can still interrupt, and `exit_judged` (judge
  `classifier`, with the model, the latency, the step of the chain, the skipped models,
  the record's hash, `cached` and `reasked`, from `JudgeOutcome::reasked`: the judge
  asked its model again for a real reason) records the answer. A verdict from the judge's cache
  of the turn comes at once and records no `exit_review_started`. efr applies the allow
  rule and the naming rule of the auto spec, section 9.4 (`exit::effective`): an allow
  stands at a low or medium risk, or at a high risk that a user message authorizes; a
  write, a floor file edit or an `above_root` exit needs a user message that names
  each target outside a cache, build output, efr's trash and the write roots; a host
  needs one that names it, except the registries of a package manager's fetch when a
  message of the naming window asks to build, install, update or fetch
  (`exit::REGISTRY_EXCEPTION`, `exit::fetch_asked`). Under
  efr's network proxy (phase D2, DP23) a host needs no name when it is `on_allow_list`:
  only `host` exits with `Grant::Host` grants name it, the list allows it on each of
  their ports, and no `upload` or `outside` exit names it. An exit that opens the whole
  network, and every exit with no proxy, never sets it. A host with `project_remote`
  (a remote of a pinned project git dir that did not change since the conversation's
  first call, never a submodule) counts as named too; an upload still needs its verb
  in a user message, which its route checks. `refused_by_proxy` (from
  `CallFacts::refused_hosts`) is a fact, never a name. The host facts come from the
  exits' grants and `ExitNeed::hosts`, so the model's `needs.hosts` words reach the
  record in their normal form. An
  allow runs the call with its launch and `ApprovedBy::Reviewer`. A deny gives
  the model the reviewer's text ("The auto reviewer refused this action ..."). Any
  other answer, and a judge that gave none (every step of its chain failed), asks the
  user with one `exit_requested` for the call, and the question shows the classifier's
  answer in `ExitInfo::judged`.
- Always-allow rules (phase E, `turn/always.rs`). They come after the floors and the
  user's rules, which the engine applied, and before the classifier. When the turn runs
  in `auto`, the launcher runs the call, and `efr_permissions::Engine::always` says that
  one rule of `sandbox.allow` covers every exit of the call (`AlwaysVerdict::Covered`),
  the call runs with the widening of a "yes", with no classifier call and no question.
  The turn records `exit_requested` and `exit_judged` (judge `always`, the category
  `sandbox.allow[N]`, the rule in words as the rationale). This needs no judge, and it
  runs while the classifier is off or paused and while the user is away. It counts as a
  judged allow, but it does not reset the classifier's denials in a row. The engine
  gives the project root of the turn's scope to the matcher, and the facts of the call
  give the real paths (`CallFacts::real_paths`, from efrd). When no rule covers the
  call but one may (`AlwaysVerdict::Offer`), and the daemon gave a writer
  (`ConversationDeps::allow_writer`, an `AllowWriter`), the question offers that rule in
  words (`ExitInfo::always`), and the parked approval keeps the rule. An answer
  `allow_always` runs the call as a "yes" (`exit_judged`, judge `user`); then the turn
  hands the rule to the writer and records `always_rule_saved` with what came of it
  (`Saved` with the index, or `NotSaved` with the reason). The call runs either way.
  The actor takes `allow_always` only from the user's own machine
  (`RemoteAlwaysAnswer` for a phone) and only for a question that offers a rule
  (`NoAlwaysOffer`); both leave the question open, never a "yes" in its place.
- The user's question: `approval_requested`, whose `exit` shows the kinds, the launch,
  what a "yes" opens, efr's facts, the model's reason and whether only the user may
  approve it. A "yes" runs the call with the narrowest launch (`exit::grant`): the
  plain sandbox for a question of a user's `ask` rule or a `destructive` exit, the
  sandbox plus the union of the exits' grants (a write bind, the open network, a
  socket, a bus, a device, an unmask), or `Launch::Unsandboxed` (the exit child) when
  an exit cannot run in the sandbox. The grants of the line's views
  (`Decision::view_grants`, such as `Grant::NetView`, which needs no question) join
  after the exits' grants by an allow list (`exit::view_fits`): `NetView` stays only
  beside `Write` and `Unmask` grants, and drops beside any other grant (a network,
  host, socket, bus, desktop or device grant). `BusFilter { bus }` (phase E, the
  filtered bus of a read form such as `systemctl --failed`) stays beside every grant
  except `NetView` and a `Bus` grant of the same bus, where it drops, so a launch
  never holds both views and never holds the filtered and the real socket of one bus.
  The exit child takes no view. The same rule gives a
  routine call with a view and no exit its grant. `CallContext::exits` carries the approved exits,
  so the toolbox can make a write target first and copy a floor file first. The answer
  is recorded as `exit_judged` (judge `user`). Before it asks about a launch in the
  exit child, the check point applies the one-command rule
  (`efr_permissions::exits::unsandboxed_line_problem`): a line that carries more than
  one command gets that text as a tool error with no question, and it is no refusal.
- Refusals and denials. A floor (`secret`, `config`) refuses the exit before any
  question and records `exit_judged` (judge `floor`); three such refusals in a row
  without a person's answer end the turn with `turn_failed` (`forbidden`, "auto stopped
  this turn: 3 actions were refused in a row. Read the answers, then send a new
  prompt."; in full mode the text starts with "full mode stopped this turn",
  `exit::refusals_stopped`). A classifier allow does not reset that count. Classifier denials count
  apart: at 3 in a row or 10 in the turn the classifier stops for the rest of the turn
  (`classifier_paused`), each later exit that it would judge asks the user, and the
  turn stops while the user is away ("auto stopped this turn: the reviewer refused 3
  actions in a row and nobody could decide. ..."). A classifier allow or any answer of
  the user resets the denials in a row; an error, a step of the chain and an expired
  question count as none. The counts live in the turn, so the model cannot reset them.
- Presence (`PresenceReader`, `ConversationDeps::presence`). A question reads it when
  it parks and again every 15 seconds while it waits (`PRESENCE_CHECK`), so a user who
  leaves or runs `efr away` after the question parked gets the away wait too, and a
  user who comes back gets the normal wait again. While the user is away it pushes the
  question once (`PresenceReader::push`) and waits `ConversationConfig::away_wait` from
  the moment it sees the user away; with no answer it
  records `approval_expired` with `away`, and `exit_judged` (judge `presence_expired`)
  for an exit, and the model reads "Nobody could approve this now; it did not run.
  Continue without it, and name it in your final answer." The turn goes on. An answer
  always wins during the wait. A phone never answers a privilege exit, a delete that
  full mode guards, nor the
  question of a line that runs a program as another user, such as `sudo`, in any mode
  (`efr_permissions::runs_as_another_user`; `RemotePrivilegeAnswer`), because the
  password needs the terminal.
- Full mode at the check point (`turn/full.rs`). A full call runs as the user with no
  sandbox: its launch is `Launch::Direct`, and `CallContext::mode` is `full`, so the
  toolbox collects the facts and copies a home file first (`predicts()`) but no
  reviewer and no always-allow rule reads it (`sandboxed()` is false). A call that the
  engine allows with exits records `exit_requested` with their record and one
  `exit_judged` (judge `full`, verdict `allow`) before it runs, and `CallContext::exits`
  carries them. Before each call of a full turn the check point reads the settings of
  that moment (`ConversationConfig::full_mode`): when the user turned full mode off,
  the call does not run, the model reads "full mode was turned off while this turn
  ran; send the prompt again", and the turn fails with `forbidden` and the same text;
  when the daemon refuses full mode now (undo or snapshots went off), the text is
  "full mode cannot run: <refusal>". A prompt or a late steer that queued in full
  mode meets the gate again when its turn starts. What efr records covers only what
  the line shows: a script or a program that the line starts is not read.
- Questions of full mode. Full mode still asks before `sudo` and the other ways to
  more rights, a process that outlives the turn (`persistence`), a delete that undo
  cannot revert, a change of efr's settings, a user's `ask` rule, and a delete that
  full mode guards (`Decision::critical`). Nobody but the user answers: no reviewer, no
  always-allow rule, no `a` offer. The summary's first line names what full mode still
  asks for (`approvals::full_line`, "full mode still asks for this: delete of ~ (your
  home directory)"); a question about more rights adds "a yes runs every command of
  this line". Only the user's own machine answers a guarded delete or a privilege
  question (`RemotePrivilegeAnswer`, "only the user's own machine answers a privilege
  exit, a delete that full mode guards, or a background job"); a phone may answer the
  others. A guarded delete waits at most 120 seconds (`exit::CRITICAL_WAIT`, or the
  approval timeout or the away wait when shorter; `ApprovalRequested::expires_in_secs`
  says so), and not at all when nobody is there when it parks. Then the delete does not
  run, `approval_expired` is recorded, and the model reads "efr did not run this delete
  of <target>: the user did not answer in 120 seconds. Do not try it another way.
  Delete narrower paths that you can name, or ask the user to run it." After three such
  expiries in a turn (`TurnExits::critical_expired`; an answer does not reset it), each
  later guarded delete is refused with no question ("efr refuses deletes of <target>
  for the rest of this turn. ..."), and each such refusal counts toward the three
  refusals in a row that stop the turn. A question of full mode with exits that
  expires records `exit_judged` with the judge `full` and the category `expired`.
- Background jobs at the check point (phase C). A `shell` call with `background: true`
  goes through the engine like any call. In `auto` a job with no exit starts at once,
  with no question and no classifier call; an exit goes to the classifier or the user
  as in the foreground, and its record has `background`, `max_minutes` (the job's
  limit, 60 when the requirements name none, so the classifier always sees the
  lifetime) and `watch`. A delete that undo covers stays an exit for a job. A part that
  no job may hold (`efr_permissions::job_refusal`) gets the model's text "... cannot
  run as a job. Run that part in the foreground, and the rest as a job.", with no
  question, in every mode. Outside `auto` (also a turn that fell back to `cautious`
  and a remote turn) each job asks the user before it starts, whatever the rules say.
  The question of a job names its lifetime (`job_lifetime_fact`, among the exit's facts
  or as the last line of the summary), and only the user's own machine answers it
  (`RemotePrivilegeAnswer` for a phone), because its "yes" lasts while nobody watches
  the job. A "no", an expired question and the away wait start no job.
- Job notices and wake turns (phase C, `actor/notices.rs`, `wake.rs`). efrd hands
  each notice of a job (it ended, or a line matched its `watch` pattern) to
  `ConversationHandle::notice`. A notice is never a prompt or a steer, never enters a
  running turn, and never answers a question: it waits until a turn starts. The next
  turn that a prompt starts takes the waiting notices after its prompt, in its first
  message. A turn takes at most 10 notices and 32 KiB of their text; one more line
  names the rest (`N more job notices; call jobs to read them`). The turn records one
  `job_notice` per notice in the batch of its `turn_started`, before it. While the
  conversation is idle (no turn, compaction, undo or queued prompt), a notice starts a
  wake turn: its first message is the preamble and the notices, with no prompt (the
  first line of its preamble says that a job notice started the turn and that the user
  sent no prompt, and it names the job turn's directory as where the turn works), it
  runs with the origin, the settings and the directory of the job's turn, and its
  `job_notice` events say `woke`. It starts only when the actor ran the job's turn and
  that turn was no wake turn (a job of a wake turn never wakes, so wake turns never
  chain), the model did not stop the job (it read the output in the tool result) and
  efrd did not stop it because full mode went off (`full_off`; such a notice and the
  one of a job that the model stopped record no `job_notice_held` and ride in the next
  turn), and both `jobs.max_wakes_per_hour` (`ConversationConfig::max_wakes_per_hour`,
  counted over the stored starts of the last hour, so a restart keeps it) and the
  daemon-wide limit
  (`Toolbox::wake_allowed`) allow one more. The wake turn runs in the mode of the
  newest turn that a prompt started, or of the job's turn when no prompt came since,
  in every mode: outside `auto` its calls ask as in any turn of that mode. A wake turn
  of `full` runs with at most `auto` (see "Turn settings"). A notice from efrd's restart (`wakes`
  false) never starts one. A notice that could not start a turn records
  `job_notice_held` with the reason, once, and rides in the next turn. A wake turn
  runs at most 20 tool calls (`WAKE_TURN_MAX_CALLS`); then it ends as completed, and
  its last message tells the next turn why. Its classifier record says `wake_turn`,
  its user messages are those before its notices (a prompt queued behind it is no
  earlier request), its naming window is the user's newest turn before it and the turn
  before that, its questions start with `(job notice turn) `
  (`efr_protocol::WAKE_QUESTION`; the CLI moves it into the card title), and a question
  counts the user away unless a client that can answer follows the conversation
  (`PresenceReader::followed`). The classifier's denial counts and its pause go on
  from one wake turn to the next until a prompt starts a turn. A user's prompt queues
  behind a wake turn, and a steer reaches it as usual. The rebuilt history of a turn
  puts its notices where the turn sent them.
- efr's own notes (v0.0.4 L2, `Toolbox::take_notes`), such as the outcome of a port
  forward of a background job. Unlike a job notice, a note does not wait for a turn:
  before each model call, also in the middle of a turn, the turn takes the notes that
  came and sends them as one user message, before the steers that it takes: the line
  `NOT_USER_NOTE` (`efr: these notes come from efr, not from the user; no human input
  has occurred.`), then each note (`notice::notes_text`). The toolbox gives each note
  once, so the model reads it in one request and then in the history. A note never
  starts a turn, records no event and is no user message of an exit record. A turn that
  ends leaves the notes that come later to the next turn's first model call. The saved
  messages of the turn keep the note; a history that is rebuilt from events does not.
- Cache deletes (`turn/cache.rs`). A contained call that deleted files in its private
  cache layer reports them in `SandboxSummary::cache_deletes`. After the call,
  `Engine::decide_cache_delete` makes one `write` exit of each path below a cache, and
  the classifier, or the user, decides them with a call id of its own, since the call
  may have had a question already. An allow hands them to
  `Toolbox::delete_cache_paths` and records `cache_delete_applied` with the call's id;
  the model reads the freed size, or that the deletes stayed private. A denied path
  stays private for the rest of the conversation and is not raised again.
- Files outside the roots that a call changed (`ToolOutcome::backups`) are recorded
  as `home_file_changed` after the call's `tool_call_completed`, for the line at the
  end of the turn. A `FileBackup` with an empty `backup` path is a copy in efrd's home
  store (phase B): `covered` is true unless its `reason` says why efr has no copy, and
  the event names no backup. A `FileBackup` with a path is a copy in the phase A backup
  store: the event names that path and says `covered: false`.
- After a call whose `ToolOutcome::sandbox` reports changes to git settings that run
  programs, the turn records them in `sandbox_surface_changed` with the call's
  `tool_call_completed`, and, for the changes that the launcher moved to quarantine,
  asks the user before any other call: `surface_question_requested` with its own
  `QuestionId`. `respond_surface` (`sandbox.surface_respond`) records
  `surface_question_answered` and hands the answer to the turn; only a "keep" from the
  user's own machine moves the changes back, through `Toolbox::restore_quarantine`. A
  phone gets `RemoteSurfaceAnswer`. The approval timeout and an interrupt record the
  answer `keep: false` with no origin and leave the changes in quarantine; the model
  reads what happened after the call's output.
- `auto` needs the sandbox. `settings::resolve` reads the probe's latest
  `SandboxStatus` (`ConversationDeps::sandbox`) and the root of the turn's registered
  project: without an available sandbox, or in a project at the home directory or above
  it, the turn runs as `cautious` and records `EffectiveSettings::fallback` with the
  reason, and the preamble says so. A prompt does not know its project yet, so only the
  turn applies the home rule.
- `ExitJudge` (`ConversationDeps::judge`) is the seam of the classifier: the daemon
  implements it with the chain of models, the cache of verdicts of a turn and the hash
  of the record. `None` asks the user about every exit.
- `AllowWriter` (`ConversationDeps::allow_writer`, phase E) is the seam that saves an
  always rule: the daemon checks the rule as a load does, adds it to `sandbox.allow`
  of `config.toml` and waits for the reload. It answers with a typed outcome, never an
  error. `None` offers no `a`.
- `respond_approval` records `approval_resolved` and hands the decision to the parked
  turn. The store refuses an answer to a call that is not pending, so of two racing
  answers only one commits. A parked call that can no longer be answered (the turn was
  interrupted, the optional `approval_timeout` passed) is recorded as
  `approval_expired`.

A turn reads one snapshot of the conversation through the store's readers, derives the
scope from the user's working directory (`ScopeResolver`, `GitScopeResolver` over
`efr_scope::derive` with the registry file read every turn), records `turn_started`
(and `scope_changed` when the user moved), makes sure `$SCRATCH` exists, and builds the
request below. When the user's directory differs from the one of the newest earlier
`turn_started`, the turn first calls `Toolbox::move_shell`, so the hidden shell goes
where the user went; without such a move the shell stays where the model left it. The
request:

1. the system prompt (the static rules, `ConversationConfig::system_prompt`);
2. the history: every turn since the newest summary, each from the actor's cache or
   the saved turn messages, else rebuilt from its events, within the byte limit of
   `HistoryLimits` (512 KiB of message JSON, scaled to the model's window, see
   "Context" and "The history only grows");
3. the newest prompt (the rule of `auto` in its preamble has one more sentence when the
   engine's egress is efr's proxy: shell commands reach package registries and GitHub
   through the proxy, and the reviewer decides another host), whose first block is the
   live-state preamble regenerated every
   turn: first the rule that only the newest live-state block is current, then the
   shell's directory and previous directory, the last command and its exit
   status, the terminal's recent commands (the newest
   `ConversationConfig::recent_commands`, oldest first, each with its status, its
   directory when it is not the shell's, and how long ago it ended when the user sent
   the prompt (`TurnSpec::sent_at`, so a prompt that waited in the queue keeps the
   time of the terminal); the newest goes
   when it is the last command; a directory is cut to 200 characters; at most 2000
   characters, the oldest go first, but the newest entry always stays; no list
   and no line when none remain, so the block is then the one of before the list), the
   git work tree and branch, home, host, OS, `$SCRATCH` (the line says that the shell
   sets it), the hidden shell's own directory when it differs, one line when the user
   moved since the
   previous prompt (where from, and whether the hidden shell moved too or where it
   stayed), one line when an earlier call left a command running in the hidden shell
   (`Toolbox::busy_shell`, which efrd answers from the live shell: the command on one
   line, how long it has run, whether it waits for input, `a password` only for a
   hidden or password-like wait of a command outside the sandbox, never the prompt row,
   and that the next shell call fails at once while it waits and the user answers or
   stops it with `,shell`), the turn's permission mode, model and
   effort, the three modes (`manual`, `cautious`, `auto`) with one line each, which
   is the one place that says what a mode allows, as `docs/permissions.md` does (four
   modes with `full` when `permissions.allow_full` is on or the turn runs in full mode;
   with full mode off the list is word for word the one of before full mode, so no
   prompt cache breaks), and
   that only the user changes a terminal's mode, model and effort (`,mode`, `,model`,
   `,effort`, or a bare `mode auto` line in sticky mode) while the settings tool
   changes only the defaults in `config.toml` after an approval, so the model never
   claims to have switched one. The `auto` line ends with what can differ in the
   sandbox, `tty` and `$$`; `df` and `findmnt` show the host's mounts there (the
   mount view of efrd). These rules ride in the preamble, not the system
   prompt, so a system prompt replaced in `config.toml` cannot drop them. The turn
   saves the prompt with its preamble, and later requests send both again unchanged
   (see "The history only grows");
4. the tool definitions, with the one edit tool that the turn's model knows.

It streams the provider and records coalesced `assistant_message_updated` events (at
most one per `update_interval` on the injected clock, the text held back sent when the
interval ends), each with only the text added since the previous one and its byte
offset, then `assistant_message_completed` with the whole text. While text, reasoning and
tool input arrive, the turn also sends drafts (`ConversationDraft`, through
`ConversationDeps::drafts`, a broadcast that the daemon holds): at most one batch per
`ConversationConfig::draft_interval` (16 ms by default, the first at once), each with
the sequence number of the last event the turn recorded before it. A text draft
carries the same `index` and byte offsets as the updates; a reasoning draft carries
the new reasoning at its offset in the reasoning of the whole turn, with the title of
the newest section (the last all-bold line); a tool-input draft carries the tool and the input bytes so far of each
call of the answer. Drafts never reach the log, and a turn that nobody follows live
does no draft work and sets no timer. Earlier turns are
rebuilt from the newest events without `tool_call_output_updated`, so a long command's
progress cannot push them out of the page. Every tool call is recorded
with `tool_call_started`, judged at the check point, run when allowed or approved
(with coalesced `tool_call_output_updated` events, and a `tool_call_input_changed`
event for every change of whether the call's command waits for input, which
`OutputSink::input_changed` reports: never coalesced, and recorded after the output
that came before it, in the same batch when that output was still held back), and
answered with `tool_call_completed`, until the model answers without a tool call. A
change that the tool reports just before it returns is still recorded before the
completion. The turn ends with
`turn_completed` (with the summed usage, and `stop` when the last model call gave no
text and no tool call or stopped for another reason than `end_turn`, such as
`max_tokens` or `refusal`; a `pause_turn` is not continued), `turn_failed` (a provider error mapped to an
`ErrorBody`: 401, missing credentials and a token source that cannot produce a token
to `unauthorized`, rate limits to `busy` with
`retry_after_ms`, an overloaded provider to `busy` without it, an unknown model to `invalid` with the `model`, the rest to
`internal`) or
`turn_interrupted`. A failed turn writes one `info` line, `the turn failed`, with its code
and message, and an interrupted one `the user interrupted the turn`. The turn builds that batch and hands it to the actor. The actor
clears the running turn first and records the batch after. It answers one request at
a time, so a client that sees the end and at once steers or interrupts gets
`NoRunningTurn`, and a prompt that it sends then starts at once.

`$SCRATCH` is `<scratch root>/<YYYY-MM-DD>-<slug>-<idtail>`: the day the conversation
began, up to five words of its title (lowercase ASCII, at most 48 bytes) and the last
8 hex digits of its id. It is claimed with `efr_stdx::fs::claim_dir` and marked with a
`.efr-conversation` file that holds the conversation id. The name is a function of the
conversation, so after a restart the same names are tried in order and the marked one
is found again; a name that something else holds falls back to 12 digits, the whole
id, then the whole id with `-2` to `-9`. A symbolic link is never taken as scratch.
The directory is made again, under the same name, when it goes missing.

### The tools reach this crate through `Toolbox`

The structure document gives this crate the edge `efr-conversation -> efr-tools`, but
the allowlist in `xtask/src/deps.rs` does not name it: `efr-tools` depends on
`efr-shell`, and the same file forbids `efr-conversation -> efr-shell` through any
chain, so `cargo xtask deps` would refuse the edge
(`efr-conversation -> efr-tools -> efr-shell`). The crate therefore defines `Toolbox`,
the registry's shape in its own terms, and the daemon implements it over its
`efr_tools::ToolRegistry` in `efr-daemon/src/tools.rs`. The adapter is a copy, field
by field:

- `definitions(edit)`: each `ToolSpec` (`name`, `description`, `input_schema`) as an
  `efr_provider::ToolDefinition`, with one of the two tools that change files: the one
  that `edit` names (`efr_provider::EditTool`), `apply_patch` or `edit`. The turn, and
  the manual compaction for its summary request, pass the edit tool of the request's
  model: the one that `Provider::models` names for it, else the one that every listed
  model shares, else the provider's own default (`turn::edit_tool`,
  `Provider::default_edit_tool`): `edit` for the Anthropic provider, also before it
  has its list, and `apply_patch` for the others. The turn reads it once when it
  starts, so the tool list stays the same for each call of the turn. Each call carries
  it in `CallContext::edit_tool`, so the toolbox refuses a call of the other edit tool,
  which the request did not offer;
- `requirements`: build an `efr_tools::ToolContext` from the `CallContext` (plus the
  home, the clock and the write journal the daemon holds), call
  `ToolRegistry::requirements`, add what each path reaches through a symbolic link
  (`ToolRequirements::with_real_paths`, on the blocking pool), and copy
  `ToolRequirements` into
  `efr_permissions::Requirements` (`paths` with `AccessMode::Read`, `ReadTree` or
  `Write` to `with_read`, `with_read_tree` or `with_write`, then `command`, `network`,
  `interactive`, `may_prompt`); a `ToolError` becomes its `Display` text for the model. The turn
  asks the toolbox's `shell_cwd` before each call and puts it in
  `CallContext::shell_cwd`, so a command's relative paths are declared from where the
  hidden shell is, which an earlier call may have moved. Once a call may run, the
  check point sets three fields. `CallContext::may_prompt` is true when the
  requirements are `interactive` (the hard flag) or `may_prompt` (the soft flag:
  `rsync` to another host, `scp`, `git` over ssh). `CallContext::may_wait` is true
  when the call also runs outside the sandbox (any launch but `Launch::Contained`:
  `manual`, `cautious`, full mode or the exit child), whoever approved it: the daemon
  then lets the call run past the model's timeout while someone who can answer
  follows it, up to `shell.interactive_timeout_minutes`. A contained call never may
  wait. `CallContext::approved_by` says who approved the call: `ApprovedBy::You` for
  an answer of the user, `YourRule` for an always rule or for an allow rule of the
  user's config (every reason about the line has a rule after the mode's built-in
  policy as its cause), `Reviewer` for the reviewer, `FullMode` for a call that full
  mode let run, and `None` for a contained call, a built-in rule or a rule of the
  conversation. The hidden shell keeps an unattended wait for a password only for a
  flagged line that `You` or `YourRule` approved, so `None` is the safe side. The
  question's `ApprovalRequested::interactive` is the hard flag, or the soft flag of a
  call that runs outside the sandbox, so the question says that the user may need to
  type a password only for a command that can get one. `CallContext::mode` is the
  turn's effective mode: `sandboxed()` (`auto`: the sandbox, the reviewer and
  always-allow), `predicts()` (`auto` or `full`: the engine predicts exits, so the
  toolbox collects facts and copies home files first) and `is_full()`;
- `invoke`: `ToolRegistry::invoke` with the `OutputSink` passed through as the
  `ToolOutputSink` (output and input waits; the daemon itself answers whether a person
  can answer hidden input), and `ToolResult` copied into `ToolOutcome`; a `ToolError`
  becomes an error outcome. A call whose `CallContext::launch` uses the launcher runs
  only through it, never typed into the hidden shell; its `result.json` summary goes to
  `ToolOutcome::sandbox`;
- `restore_quarantine`: moves the quarantined changes of a call back when the user
  keeps them; the default keeps no quarantine and moves nothing;
- `turn_report`: the files that the turn changed through the launcher and that run
  code later outside the sandbox; the turn records them as `turn_surface_report` right
  before its terminal event; the default reports none;
- `turn_changes`: the files that the turn changed, from the toolbox's snapshots, what
  `efr undo` can take back (`TurnUndoInfo`) and the snapshots that the turn took; the
  turn asks once before its terminal event, whatever the ending, records one
  `snapshot_taken` per snapshot right before it, and puts the files and the undo info
  in `turn_completed`, `turn_failed` or `turn_interrupted`; the default takes no
  snapshot.
- `undo`: takes back the file and ref changes of one turn (`turn.undo`), or with
  `dry_run` only says what it would do; the default has no undo and refuses with
  `off`. `call_runs`: true while a call of the conversation still runs after its tool
  answered, so an undo must wait; the default runs no such call. A call's own changes and
  a file tool's diff come back in `ToolOutcome::changes` and `ToolOutcome::diff`, which
  the turn records in `tool_call_completed`;
- `cancel`: `ShellSessions::interrupt(conversation_id)` for the shell tool, so an
  interrupted command does not keep running in the hidden shell;
- `preview`: the diff of a `write_file` call, once the tools offer one;
- `takes_manual_input`: `ToolRegistry::takes_manual_input`, which the turn records as
  `manual_input` in the call's `tool_call_started`, so a client offers a manual answer
  (`Ctrl+\` in `efr`) only for a call that takes one.

If the forbidden-edge check learns to accept the path through `efr-tools`, this crate
can depend on `efr-tools` and implement `Toolbox` for `ToolRegistry` itself; nothing
else changes.

### What the daemon does around it

- It completes stored results: a receipt's result is stored without `seq`, and the
  store records the number with the receipt, so a retry is answered with the stored
  result plus `receipt.seq`. A `ConversationError::DuplicateCommand` carries the
  receipt. Rejected commands (an error before anything was recorded) get their
  rejected receipt from the daemon, which owns the mapping to wire errors.
- It reconciles after a restart (cancels in-flight turns, expires pending approvals,
  records queued prompts as not run); the actor starts with an empty queue.
- It routes `prompt.send` to the conversation of the terminal's slot for the prompt's
  scope and spawns or reuses its actor. `ConversationStart::New` carries the slot's
  `route` for `conversation_created`, and `ConversationHandle::send_routed_prompt`
  takes a `PromptRouting`: an event that the prompt's append records right before its
  `prompt_queued` (`terminal_attached`, so the log never holds a pick-up without its
  prompt) and the `route` that the result and its receipt carry.

### Provider items survive a restart

`provider_raw` must go back unchanged to the model that made it, but no event in
`efr-protocol` carries it. The actor keeps the exact messages of the turns it ran that
the newest request carried, each with the provider and the model that answered it,
and uses them while both are the same; with another provider or another model (a
prompt that names one, or a new default in the config) they lose `provider_raw`: the
other model's encrypted reasoning and item ids go, the text, the tool calls and their
results stay, as opencode does. A turn also saves its exact messages in
`efr_store::turn_messages`, in the batch that records its end, and the store keeps
every turn until a summary covers it. After a restart the snapshot reads them back in
place of the empty cache, so the next request is the same as without the restart.
Only a turn with no saved messages (one from before the table, or one whose saved
messages cannot be read back) is rebuilt from the events, with the daemon's call ids
and without provider items or its preamble.

A call of a freeform tool (its input is text, such as a patch) keeps that kind on the
way: `ContentBlock::ToolCall` has `freeform` set in the saved messages, and the turn
records it as the `freeform` flag of `tool_call_started`, which a rebuild from the
events reads back. So the next request sends the call and its result in the form the
model wrote them (for the Responses API, `custom_tool_call` and
`custom_tool_call_output`).

### Calls of tools that the request does not offer

The history can hold a call of a tool that the next request does not offer: an `edit`
call of a Claude model when an OpenAI model goes on with `apply_patch`, an
`apply_patch` call the other way round, a call of a tool that efr no longer has, or a
call that the model made of a tool that the request never had. A provider can refuse
such a call, and a model can take it as a tool that it may call. So every request
shows each such call and its result as text, in their places (`unoffered.rs`, applied
when the turn builds a request and in each summary request), for every provider:

- the call: ``Earlier tool call `edit` (id call_1), shown as text: this request does
  not offer the tool. Its input:`` and, on the next line, the input: a freeform call's
  text as it is, any other input as compact JSON;
- its result: ``Result of the earlier tool call `edit` (id call_1):`` (with `, which
  failed` before the colon for an error) and, on the next line, the output.

The text is a pure function of the call and the request's tools, so each request with
the same tools sends the same bytes for an old call, and the prompt cache reads them.
A message with such a call keeps its `provider_raw`, which holds the call in the
provider's own form: the provider sends that raw call as the same text
(`efr_provider::unoffered_call_text`), or sends the canonical content in place of
the raw items. So a Claude message keeps its thinking blocks, which the Messages API
needs at the start of the last assistant message of a tool loop. The saved messages
keep the call as it was, so after a switch back to a model that has the tool, the
call goes in its own form again.

### The history only grows

A provider's prompt cache, and Claude's thinking blocks, need one rule: each request
starts with the messages of the request before it, plus its answer. Only these events
may edit earlier messages: a compaction (a pruning or a summary), another model or
provider, and a change of the system prompt, the tools or the effort.

The efr-daemon test `cache_prefix` measures this on the OpenAI path: the real
`openai-api` provider against a local Responses server, 55 turns and a restart. Before
this design, every request edited the prompt of the turn before it, because the
preamble left that prompt. From the 52nd request on, each request also edited the
oldest turn: the actor and the store kept the exact messages of 50 turns only, so the
oldest turn came back from its events, without its provider items.

The design:

1. The saved prompt is the prompt as the model read it: the `<live_state>` preamble,
   then the user's text, in one user message. Every later request sends it again
   word for word, so an old preamble stays as a record of that moment. The first line
   of each block says that only the newest live-state block is current, so a system
   prompt replaced in `config.toml` cannot lose the rule (the default system prompt
   says it too). A block keeps the bytes it was sent with, so a newer efr whose
   preamble says more changes only the blocks of new prompts. The preamble shows the
   user's last command with its secrets redacted when it is rendered, so the bytes
   that the store keeps are the bytes that went to the model (`preamble/secrets.rs`).
   It redacts the value of an assignment to a name that `efr_sandbox::secret_like`
   matches, also inside the value of another assignment
   (`--env=API_TOKEN=[redacted]`), but not a count (`MAX_TOKENS`, `TOKEN_COUNT`); key
   forms such as `sk-ant-`, `sk-proj-`, `sk-` and `ghp_`; the password of a URL
   (`https://user:[redacted]@host`); the value of a secret header (`Authorization`,
   `Proxy-Authorization`, `Cookie` and the names that look like secrets) after its
   scheme (`Authorization: Bearer [redacted]`), where a header stands: after `-H` or
   `--header`, after a `=`, at the start of a command, among the arguments of `http`
   or `xh`, or after `echo` for `Authorization` and `Cookie`; and the value of an
   option that takes a password, only for the programs in its list
   (`mysql -p[redacted]`, `sshpass -p`, `docker login -p`, `curl -u user:[redacted]`).
   The list is per program because the same option means something else elsewhere
   (`docker run -p 8080:80`, `psql --password`), and a subcommand such as `login`
   counts only as one of the first two words after the program. It also redacts the
   keys of other services by their prefix (`AKIA`, `xoxb-`, `xoxp-`, `glpat-`, `AIza`,
   `npm_`, `hf_`, `pypi-`), a JSON web token, and a long random word: 32 or more
   characters of base64 or url-safe base64 with upper and lower case letters and
   digits, but not hex of the length of a git id or a sha256 sum and not a path (a
   part between two `/` of three or more lower case letters, such as `src`; the last
   part, and the first one before the only `/`, never count, so a base64 key with a
   `/` stays hidden). A key of only lower case hex or base32 stays visible. A value that names another value
   (`$TOKEN`) stays. The events keep only the prompt's text; a turn
   rebuilt from its events has no preamble.
2. The list of earlier turns comes from the conversation's turns
   (`efr_store::conversations::turns`) and the saved messages, never from the
   4096-event page. A turn counts when it started and finished and the newest summary
   does not cover it. Its messages come from the actor's cache, else from
   `turn_messages`, else from its events. Only a turn with no saved messages (one from
   an efr before the table) needs its events; when it started before the page, the
   store reads its events by their sequence numbers (`events::read_turn_range`).
3. There is no turn limit. `turn_messages` keeps every turn until a compaction with a
   summary covers it, and the actor keeps in memory the turns that the newest request
   carried.
4. The byte limit (`HistoryLimits::max_bytes`, at least twice the window) stays as
   the safety net. When the window would leave out a turn, the guard compacts before
   the model call, and that compaction always writes a summary. This happens after a
   switch to a model with a smaller window; otherwise the trigger compacts long
   before. Only a turn that cannot compact (auto off, the breaker open, or nothing
   before the verbatim tail) sends the history without that turn, with the note
   `N earlier turns are omitted.`
5. The fresh context block of a compaction is part of its record
   (`Compaction::fresh`). The turn and the manual compaction send that stored text, so
   a restart sends the same bytes. Only a compaction from an efr before this field
   reads the block from disk again, once, and the actor keeps it in memory.
6. A turn that did not complete ends with a user message of its own, the marker
   (`history::end_marker`), so the next turn does not answer its prompt or go on with
   its work: `[The user interrupted this turn. ...]`, `[This turn stopped because of
   an error ...]` or, for a turn that ran when efr stopped, `[This turn stopped
   because efr stopped. ...]`. `finish` adds it for an interrupted or failed turn,
   after the results of the calls that it closed, and counts it in the context; a
   rebuild adds it from the turn's status. The text depends only on the status, so
   each later request sends the same bytes. A turn saved before the marker keeps its
   bytes. A turn that only a rebuild serves, such as one that ran when efr stopped,
   gets the marker also when it ended before the marker existed. So the first request
   of such a conversation after the update misses the prompt cache once; we accept
   this, because without the marker the model can answer that turn's prompt. A steer
   that an interrupt sends again as a prompt follows the marker.

## The index redaction

efrd's search index keeps the user's prompts, the model's answers and the commands
that the agent ran. Before a text goes into it, efrd passes it through
`redact_for_index` (`index_text.rs`). That function replaces each secret that it
recognises with `[***]`. The marker has no letter or digit, so the tokenizer of the
index makes no word of it, and a search for `redacted` cannot list the places where a
secret was.

It runs two passes:

1. A pass over the whole text, for the index only: a PEM block of a private key
   (`-----BEGIN ... PRIVATE KEY-----` to its `-----END ...-----`, or to the end of the
   text when the cut of the index removed that line), and the value of a secret in
   config text: a line that starts with a name and `: ` (YAML) or `=` with a blank on
   either side (TOML, INI), and a JSON pair `"name": value` anywhere in a line. The
   name must look like a secret, by the rule of the preamble (`secret_name`).
2. The line redaction of the preamble (`preamble/secrets.rs`) on each line, with the
   marker `[***]` in place of `[redacted]` (`redact_with`). The preamble keeps
   `redact`, byte for byte as before.

It cannot recognise every secret. A password in prose ("my password is hunter2"), a
key of only lower case hex, a YAML value on the line after its name and a webhook URL
with a lower case path stay. The tests list these misses (`index_text/tests.rs`).

A change of the patterns of `preamble/secrets.rs` or of `index_text.rs` must increase
`REDACTION_VERSION`. efrd keeps the version in the index and, when it differs, clears
the index and builds it again from the event log: each increase costs a full backfill.
The file does not get smaller. The snapshot test `redaction_corpus_is_stable` shows
the version on its first line, above the redaction of a fixed corpus of 100 lines, so
a reviewer sees a change of the output next to the version.

The loops of the line redaction are linear: a key is read only at the start of a word,
and the value of an assignment only for a secret name. A text of 33 KiB of one
repeated pattern costs a few milliseconds at most (efrd's bench
`bench_search_redaction_worst`).

## Context

This section is the contract of context management: the accounting, the guards, the
compaction and its display. The code that builds each part follows it. A change of the
contract changes this text first. The numbers live in `src/context.rs`
(`CompactionConfig`, `ContextLimits` and the constants); the wire types live in
`efr-protocol` (`Usage`, `ContextUse`, `Compaction`, `CompactionTrigger`, the draft
parts `context` and `compacting`, `conversation.compact`).

### Words

- The window: the context window of the turn's model, in tokens. It is the
  `context_window` of the model in `ConversationConfig::models` (the provider's model
  catalog, from the backend, its cache or, for OpenAI only, efr's built-in list; or the
  model's entry in `[openai] models` or `[anthropic] models`, which the daemon cuts
  down to the model's largest window). When it is not known, efr counts with
  `DEFAULT_CONTEXT_WINDOW` (128000).
- The context: the tokens of one request plus its answer.
- The trigger: `auto_at` percent of the window, rounded down (`[compaction] auto_at`,
  default 76; 206720 tokens on a window of 272000). An auto compaction runs at the
  trigger.
- The hard cap: 95% of the window, rounded down (`HARD_CAP_PERCENT`). efr never sends a
  request that it estimates above the hard cap.
- The limit: the trigger when `[compaction] auto` is true, else the hard cap. A client
  shows the context as a percent of the limit, so 100% means "a compaction runs now"
  (or, with auto off, "the next request does not go out").
- The cut: a place in the model's history. The messages before it are compacted. The
  messages after it (the verbatim tail) stay word for word. A cut never falls between a
  tool call and its result: the tail never starts with a message that holds tool
  results.

### Accounting

1. After each model call, the turn keeps the usage of that call next to the sum of the
   turn. `context_tokens` is the input plus the output of the last call. The input
   counts cached tokens too.
2. `turn_completed`, `turn_failed` and `turn_interrupted` carry `usage` (the sums, plus
   `context_tokens` of the last call) and `context` (`ContextUse`: `tokens`, `limit`,
   `window`). `context.tokens` is the last real count plus the estimate of the
   messages after it, such as the results of a call that an interrupt stopped. After
   a completed turn nothing comes after it, so it is the real count. When a
   compaction came after the last call, it is the estimate of the compacted request.
   A turn that ended before it had its settings has no `context`.
3. The turn sends a `context` draft (`DraftPart::Context`) before each model call, with
   the estimate, and after each call, with the real count.
4. Each request carries the conversation id as `prompt_cache_key` unless
   `ConversationConfig::provider_options` names one. OpenAI caches on its own; the key
   sends the requests of one conversation to the same cache. On the subscription
   backend, `efr_providers::openai` also sends the key as the `session-id` header, as
   Codex does.
5. Each model call of the conversation runs in a `provider_request` span with the
   field `gap_ms`: the time from the start of the conversation's call before to the
   start of this one (`gap.rs`). The turn's calls and the summary calls of both
   compactions count, because they read and write the same prompt cache. The first
   call after a start of efrd has no `gap_ms`, because the time of the call before is
   not stored. A provider's cache entry lives for a time that counts from the start of
   the request that wrote or last read it, so the field, the provider's debug line
   about its cache markers and the call's usage show which time to live pays.
6. Each model call of a turn that reported its usage writes one `debug` line with the
   target `efr_conversation::usage` (`turn/stream.rs`, DV12): `usage gap_ms=95000
   input=18200 output=1100 read=16600 write=1200 write_1h=1200` (`gap_ms=none` for the
   first call since efrd started). It holds numbers only, never text, and the gap comes
   from the shared clock. `log = "info,efr_conversation::usage=debug"` shows it without
   the rest of the debug lines; an estimator reads it from the journal. The summary
   calls of a compaction write no such line.
7. The `phase=model_answer` line of a call names `stop_reason` only for an answer that
   ended (`end_turn`, `tool_use`, `max_tokens` and the others); a call that the user
   interrupted, or that failed, says `outcome=interrupted` or `outcome=error`, because
   neither is a stop reason of the API (C2.8e). The start of the line stays the same.

### The estimate

Before each model call, the turn estimates the context of the request (`Meter` in
`src/context.rs`):

- with a base: the base plus `estimate_tokens` (4 bytes a token, rounded up) of the
  JSON of each canonical message added after it. Inside a turn the base is the input
  plus the output of the last call. At the start of a turn it is the `context.tokens`
  of the newest ended turn in the log, and the new prompt is added to it. The log
  holds it, so it survives a restart;
- without one (the first turn, the first call after a compaction, a newest turn that
  was cancelled or has no end, as after a daemon restart, or a newest ended turn
  without `context`, without a real count in its `usage`, with a
  `conversation_compacted` after it, or that ran with another model than this turn,
  as before a switch of `[model] provider`, because that model's count says nothing
  about the tokens of this one):
  `estimate_tokens` of the JSON of the whole request (system prompt, tool definitions
  and messages).

Each real count replaces the estimate as the new base. A base that counts turns that
the history no longer sends is too high, never too low, so the turn compacts early
rather than late.

### The guards, before each model call

1. When `auto` is on and the estimate is at or above the trigger (or above the hard
   cap, for an `auto_at` above 95), the turn compacts (trigger `auto`) and then makes
   the call. The turn goes on after the compaction.
2. When the estimate is above the hard cap and the turn cannot compact (auto is off, or
   the breaker stopped it), the turn does not send the request. It fails, see "Failures".
3. When the provider answers `ProviderError::ContextOverflow` (OpenAI
   `context_length_exceeded` or an HTTP 413, Anthropic's 400 `prompt is too long`;
   an Anthropic 413 is a body above 32 MB and not an overflow), the turn never retries
   it as a transient error. With auto on, it compacts once (trigger `overflow`,
   `tokens_before` is the estimate of the refused request) and sends the call again
   once. A second overflow of the same call fails the turn. With auto off, the first
   overflow fails the turn.
4. The breaker: a compaction whose `tokens_after` is at or above the trigger counts as
   a miss, and so does a compaction that frees nothing and records nothing (nothing
   lies before the tail, or the summary request fails). After `BREAKER_TRIES` (2)
   misses in a row, the turn makes no more auto or overflow compactions. It goes on
   while its requests fit under the hard cap. The count of misses carries from turn
   to turn (the actor keeps it in memory), so a context that stays above the trigger
   does not pay for two summaries at the start of each turn. It goes back to 0 when a
   compaction brings the context below the trigger, when a guard finds the estimate
   below the trigger, and after a manual compaction. The client shows the breaker
   line (see "Display").
5. The safety net of `HistoryLimits` stays, scaled to the model's window
   (`HistoryLimits::for_window`): the byte limit is at least twice the window at 4
   bytes a token. So the net never leaves out turns that the window can hold, and the
   compaction, not the net, makes room. When the history would still leave out
   earlier turns that ran and that no summary covers (past the most bytes), the turn
   compacts first, with trigger `auto`, also below the trigger, and the compaction
   always writes a summary. A turn that cannot compact sends a history with a user
   message `N earlier turns are omitted.` (`1 earlier turn is omitted.` for one) after
   the summary, or first when there is no summary, and the daemon logs one `warn`
   line with the count. It never leaves out turns without that note. A turn that
   never started, such as one whose settings failed, is no omitted turn. A summary
   written while turns were omitted records their count (`omitted_turns`), see
   "Storage". While the history leaves out turns, the breaker does not close.

The turn sends the `compacting` draft before each compaction, records
`conversation_compacted` itself, drops the base of its estimate, counts the misses of
the breaker, and sends the next `context` draft with the estimate of the compacted
request. An interrupt during a compaction ends the turn at once.

### Compaction

One compaction has these steps. A turn runs them between two model calls, and a
manual compaction runs them between turns.

1. Prune. Take the model's history as the next request would send it. Walk its tool
   results from the oldest. Keep every result in the newest `PRUNE_KEEP_TOKENS` (40000)
   of history as it is, and every result of the shortest tail (step 3) however large,
   so the model always reads the newest result. Give every other result whose output
   is longer than `PRUNED_OUTPUT_STUB` that stub as its output. When this frees fewer than
   `PRUNE_MIN_TOKENS` (20000), do not prune: a new cache prefix costs more than it
   saves. When it frees enough and the estimate is then below the trigger, the
   compaction ends here, with no summary (an auto compaction only). After an overflow
   the estimate has just counted too low, so it cannot show that pruning is enough: a
   summary always follows. The cut is the place after the newest pruned result.
2. Summarize. Send one summary request:
   - the same system prompt and the same tool definitions as the turn;
   - the history as the last request sent it, so the request hits the prompt cache.
     Only when that request would pass the hard cap, or after an overflow, does it
     send the pruned history;
   - as the last message, a user message with the summary prompt (below) and, for a
     manual compaction with `focus`, the line `Keep in the summary: <focus>`;
   - `max_output_tokens` of `SUMMARY_MAX_OUTPUT_TOKENS` (8000) for the summary plus
     `SUMMARY_REASONING_TOKENS` (24000) for the model's reasoning, which the Responses
     API counts in the same limit. The subscription backend refuses that member, so
     there the prompt alone asks for the length. On the API backend, a `max_output_tokens`
     in the model's entry of `[openai] models` wins over it. So the 8000 is a budget
     that the prompt asks for, not a cap that efr can make the provider keep.

   Some messages do not fit in the summary request. Then the newest messages go first,
   from the end down to the start of the tail: the new history keeps them word for
   word after the summary, so the summary loses nothing, and the request keeps the
   prefix of the last request. The request never ends with a tool call. When the
   request without the tail still does not fit, the oldest messages after the fresh
   block and the earlier summary go (never so that a tool result comes first, so a
   tool call never loses its result), and one user message in their place says `N
   earlier messages are omitted: they did not fit in this request.` The summary prompt
   also says it: `This request leaves out N earlier messages of the conversation: they
   did not fit in the model's context. Say so under the first heading.` (with the
   earlier turns that the history left out too, when there are any). The target is the
   trigger, which leaves room for the answer. The provider counts more tokens than the
   estimate, so after a refusal the target is the trigger scaled by what the refusal
   shows: below the estimate of the refused request times the trigger over the window.
   This happens:

   - before the first try, when the summary request is above the hard cap, because efr
     never sends such a request. A history that grew on a model with a larger window
     (a conversation that moves from a Claude model with a window of 1M tokens to an
     OpenAI model with 272k) compacts this way at the first turn on the new model: the
     guard compacts before the first call, and the summary request fits the new
     window, so the provider refuses nothing;
   - after an overflow, before the first try, when the summary request is not smaller
     than the request that the provider refused (pruning freed nothing), because the
     provider would refuse it too;
   - when the provider refuses the summary request itself as too large; then the
     request goes once more.

   When nothing fits short of all the messages after the head, they all go, and the
   summary carries the earlier summary forward. The messages before the cut that the
   summary never saw are counted in `omitted_messages`, and the daemon logs one `warn`
   line with the count.

   The answer's text is the summary. Tool calls in the answer are ignored. An answer
   without text fails the compaction, and so does an answer that the provider cut
   off (it reached the output limit, or the provider stopped it for its content): a
   summary that stops in the middle of its sections never replaces the history. The
   summary covers the whole history it saw,
   the tail too: the tail repeats the newest part word for word, and the summary must
   stand alone. An earlier summary is part of that history, so each summary carries
   the one before it forward.
3. Cut. Walk back from the newest message and keep messages in the tail while their
   estimate fits in `TAIL_TOKENS` (20000). The tail always holds at least the newest
   tool call with its result, or the newest prompt or steer, whichever is newer: the
   shortest tail. Move the cut back until it does not start with tool results. The cut
   may fall inside a turn (`through_message`). When the cut would be before the first
   message, nothing lies before it, and the compaction frees no room.
4. Record `conversation_compacted` (`Compaction`), see "Storage". `tokens_after` is the
   estimate of the rebuilt request.

A compaction that fails (the summary request fails, or its answer has no text or was
cut off) records nothing. An auto compaction that fails lets the turn go on when the request
fits under the hard cap; else the turn fails.

### The summary prompt and its record

The summary prompt is one constant, the text of `src/compaction/prompt.md`. It asks for
these sections, as Markdown headings in this order, with `None.` under a heading that
has nothing:

1. `## Task and state`: what the user asked for, and how far the work is.
2. `## Decisions`: the choices made and why; what the user approved or refused.
3. `## Important details`: the facts the rest of the work needs: names, values,
   errors, commands that worked or failed, and the instructions and preferences that
   the user gave in chat, quoted.
4. `## Files and places`: the files read or changed, directories, URLs and services
   touched.
5. `## Open work and next step`: what is left, and the next action.
6. `## Shell and directories`: the user's directory and the hidden shell's directory
   now, the earlier directories that matter, the shell state (variables, virtual
   environments) and the running jobs.
7. `## System tasks`: open system-manager tasks that need a follow-up or a cleanup,
   such as a service that was started, a package that is half installed, a timer or a
   mount.

The prompt also says: write facts, not a story; keep paths, commands and error texts
exact; at most about 2000 words; do not call tools. The same prompt and the same
sections make the handoff record of milestone 4: a record is the summary text plus
the event's `compaction_id`, `model`, time and conversation id.

### Storage

- `conversation_compacted` holds the whole compaction (`efr_protocol::Compaction`): its
  id, the turn that ran it (absent for a manual one), `trigger`, `focus`, `model`,
  `window`, `limit`, `tokens_before`, `tokens_after`, the cut (`through_turn`,
  `through_message`), `kept_turns`, `pruned_outputs`, `pruned_tokens`, what the
  summary never saw (`omitted_turns`, `omitted_messages`), the fresh context block
  (`fresh`, with a summary only), `summary` and the `usage` of the summary request.
- The store keeps every compaction in the `compactions` projection
  (`efr_store::compactions`). A turn reads the newest compaction with a summary and the
  newest compaction of any kind with their own query (`compactions::latest`), never
  from the 4096-event page, so the page never decides what the model sees.
- Every event stays in the log. After the event is recorded, the `turn_messages` rows
  of the turns before `through_turn` may be deleted, and the row of `through_turn` too
  when `through_message` is absent.
- `efr history` shows every event and marks the place of each compaction with its
  scrollback line (see "Display").

### The history after a compaction

A request after a compaction with a summary has these parts, in this order:

1. the system prompt;
2. a fresh context block, one user message read from disk when the compaction ends
   (`fresh.rs`), between `<fresh-context>` and `</fresh-context>`: the user's directory
   and the hidden shell's directory, the children of the hidden shell
   (`Toolbox::shell_children`; left out when the toolbox cannot tell), efrd's
   background jobs of the conversation under `## Background jobs`
   (`Toolbox::background_jobs`: id, state, command and the time left; left out when
   there is none), the git status
   of the project root (`ScopeResolver::status`, at most 40 lines, git runs none of the work
   tree's programs), and the `AGENTS.md` files from the project root down to the user's
   directory, each cut at 32 KiB (memory joins at milestone 4). The project root is the
   registered project's, else the git work tree's; without one, only the user's
   directory is read. The compaction stores the block (`Compaction::fresh`), so every
   request until the next compaction sends the same bytes, also after a daemon
   restart. Only for a compaction from an efrd before the stored block does the next
   turn read it from disk again, and the actor keeps that copy in memory with the
   compaction id;
3. the summary, one user message: `<conversation-summary>`, the summary text,
   `</conversation-summary>`; when the summary never saw some turns or messages
   (`omitted_turns`, `omitted_messages`), one more user message after it says so:
   `The summary leaves out 2 earlier turns and 40 earlier messages: they did not fit
   in the model's context.`
4. the verbatim tail: the messages after the cut, from the turn's messages (the
   actor's cache or `turn_messages`), with the stub in each tool result before the cut
   of a newer prune-only compaction. A tool result whose call is not in the history
   is left out (a `warn` line gives the count): a turn rebuilt from its events can
   have other places than its transcript had, and a provider refuses a result
   without its call;
5. the new turn: its preamble and prompt, or, inside a turn, the rest of that turn.

Parts 1 to 3 stay the same until the next compaction, so the request prefix stays
stable and the prompt cache hits. After a prune-only compaction, the request is the
history as before, with the stub in each pruned result.

### Manual compaction

`conversation.compact { command_id, conversation_id, focus? }` (`efr compact [focus]`,
`,compact [focus]`), scope `operate`:

- While a turn of the conversation runs, or another compaction does, it is refused
  with `conflict` (`ConversationError::CompactionBusy`). efrd does not wait for the
  turn: the turn compacts on its own when it needs to, and a wait would hold the client
  for an unknown time. A retry with the command id of the compaction that runs, as
  after a dropped connection, is no other compaction: it waits for the same answer.
- When nothing lies before the cut, it is refused with `conflict`
  (`ConversationError::NothingToCompact`). A summary request that fails is
  `ConversationError::Summary`, an answer without text `EmptySummary`, an answer that
  was cut off `IncompleteSummary`; none records anything.
- Otherwise the actor runs the steps above as its one job, in a task of its own
  (`actor/compact.rs`), so it keeps answering, and `ConversationState::compacting` is
  true. A prompt that arrives meanwhile waits in the queue (its result says `queued`)
  and starts when the compaction ends. The summary request uses the model and the
  effort of the newest turn, so it shares the prefix of the last request; when the
  model list no longer has that model, as after a switch of `[model] provider`, it
  uses the default model and effort of the config. A manual
  compaction always writes a summary: it does not stop after the pruning. The event
  has trigger `manual`, no `turn_id`, and the `focus`; it is recorded with the
  command's receipt. The result has the event's `seq` and the compaction. A retry with
  the same command id returns the first result.
- It never starts a turn.

### What fills the context

`conversation.context { conversation_id, model? }` (`efr context`, `,context`), scope
`read`, records nothing:

- The actor answers at any time, also while a turn, a compaction or an undo runs, in a
  task of its own (`actor/context.rs`), so it keeps answering and nothing waits for it.
- It builds the request that the next model call would send, as a manual compaction
  builds it: the snapshot of the log, the window with the actor's cached turns and the
  stored fresh block, the system prompt and the tool definitions, for the model of
  `model`, else of the newest turn (the default model when the model list no longer
  has it). While a turn runs, the turn counts as ended, so its messages so far are
  rebuilt from the log; the log holds no preamble of the running turn. The next prompt
  and its preamble are not known, so they are left out.
- `context/breakdown.rs` measures the request at 4 bytes a token, as the turn's own
  estimate does: each block counts for its kind (`system`, `tools`, `summary`,
  `prompts`, `preambles`, `notices`, `answers`, `tool_calls`, `tool_outputs`,
  `pruned`), the frame of a message for the kind of its first block, and the rest of
  the request's JSON for `system`. The tokens of the parts add up to exactly the
  `estimate`. A text block is a preamble when it starts with `<live_state>`, a notice
  when it starts with `<job id="`, else a prompt.
- `largest` holds the 5 largest items, each with its turn's number, its tool and the
  label that `efr history` shows (the first line of a command, a path, the files of a
  patch, a job id), at most 120 characters. No item carries the text of a message.
- `counted` is the provider's count at the end of the newest ended turn, the base of
  the next turn's estimate; none after a compaction or a cancelled turn. `window_known`
  is false when the model list gives no window and `window` is the default 128000.
- `compactions` counts the compactions and gives the facts of the newest without its
  summary. `cache` sums the usage of every ended turn and every compaction since the
  start (a turn that ended before the page of the log is read by its own end event),
  and gives the usage of the last turn that reported one.

### Undo

`turn.undo` reaches the actor as `ConversationHandle::undo` with the turns that the
daemon found in the log (`UndoCandidate`, newest first). The actor runs it like a
manual compaction (`actor/undo.rs`):

- While a turn, a compaction or another undo of the conversation runs,
  `Toolbox::call_runs` says that a call still runs after its tool answered, or
  `Toolbox::job_blocks_undo` names a background job that may write in a root of the
  turn (phase C: a job of the conversation, or one of another conversation in the same
  roots), it is refused with `ConversationError::UndoBusy` and its reason
  (`UndoBusyReason`, `Job` with the job's number). A
  retry with the command id of the undo that runs waits for the same answer. A phone
  gets `UndoRefused(PhoneOrigin)`; the daemon refuses it before that too.
- Otherwise the actor calls `Toolbox::undo` in a task of its own, with an `UndoId` from
  its clock and generator, and `ConversationState::undoing` is true. A prompt that
  arrives meanwhile waits in the queue and starts when the undo ends.
- A refusal of the toolbox is `UndoRefused` with its reason. A dry run and a list
  record nothing; a list runs at any time and takes no place. An applied undo records
  `undo_applied` (a conversation event, no turn) with the command's receipt in one
  batch; its counts come from the entries of the result. The stored result has no
  sequence number, so `completed_result` returns it as it is.
- It never starts a turn.

### Failures

A turn that fails for its context records `turn_failed` with code `internal`, data
`{"cause": "context_overflow", "tokens": <estimate or refused size>, "window":
<window>}`, and a message that names the cause and the way out (`context_full` in
`src/context.rs`):

- above the hard cap: `the context is full: about 260k of 272k tokens, above the cap
  of 258k; run ,compact or start a new conversation`;
- a refusal with auto off: `the context is full: the model refused about 281k of 272k
  tokens; run ,compact or start a new conversation`;
- a refusal whose compaction failed: `the context is full and the compaction failed
  (<why>): the model refused about 281k of 272k tokens; ...`, where `<why>` is the
  sentence of the provider's error (such as `the provider is rate limiting requests`)
  or of the summary's failure;
- a refusal or a request above the hard cap when nothing lies before the tail, so no
  compaction frees room: `the context is full and the newest messages alone do not
  fit: about 260k of 272k tokens, above the cap of 258k; start a new conversation`
  (without `,compact`, because a manual compaction keeps the same tail);
- a request above the hard cap whose auto compaction failed: `the context is full and
  the compaction failed (<why>): about 260k of 272k tokens, above the cap of 258k;
  ...`;
- a refusal or a request above the hard cap after the breaker: `the context is full:
  compaction did not free enough room (still about 240k of 272k tokens); ...`;
- a second refusal after the compaction: `the context is still full after a
  compaction: the model refused about 150k of 272k tokens; ...`.

A `ContextOverflow` that reaches the generic mapping of provider errors gets the data
`{"cause": "context_overflow"}`.

### Display

`efr` shows the context inline, never in an alternate screen:

- The status row of a running turn always has `ctx N%`, from the newest `context`
  draft or compaction (`tokens_after`), and so does the line of a running call, which
  takes the row's place. N is `tokens` times 100 divided by `limit`, rounded down. The
  colour roles are `Success` below 50, `Warning` (not bold) from 50 and `Error` from
  90. On a screen that is too narrow, the gauge goes first.
- The end-of-turn line has `ctx N% (<tokens>/<limit>)` from the end event's `context`,
  such as `done in 42s, ctx 43% (89k/207k), 1.1k out, cache 91%`. A turn without
  `context` keeps today's line. `cache N%` comes from the event's `usage` (the README
  of `efr-cli`).
- After a `compacting` draft the status row says `compacting context`, until the
  `conversation_compacted` event, the next `context` draft or the end of the turn.
- A client that subscribes while a turn runs gets the turn's newest `context` draft
  (and its `compacting` draft while it compacts) first, so the gauge shows at once,
  not only at the next model call (`ConversationHandle::live_drafts`).
- Each `conversation_compacted` prints one muted line, wrapped at the width so that
  its way out is never cut off:
  - `context compacted (auto): 231k -> 24k tokens, kept 3 turns, summary 3.2k`
    (`(efr compact)` for a manual one; `pruned 12 outputs` in place of the summary
    for a prune-only one; `(left out 2 turns and 40 messages)` at the end when the
    summary never saw them, also on the overflow line);
  - for trigger `overflow`: `context full: the request was 281k of 272k tokens;
    compacted and retried`;
  - when `tokens_after` is at or above `limit` (a breaker miss): `context full:
    compaction did not free enough room (still 240k); run ,compact or efr new`, and
    `...; run efr new` after a manual one, which another `,compact` cannot help.

## Tier

Tier 3, the engine.

## Allowed dependencies

`efr-provider` (the `Provider` trait and canonical messages), `efr-permissions` (the
engine at the check point), `efr-scope` (scope derivation, `Home`), `efr-store` (the
writer and the readers), `efr-sandbox` (`secret_like`, the names that look like
secrets, which the sandbox's environment filter uses too; the preamble redacts the
values of such names in the last command), `efr-protocol` (events, ids, method params
and results) and `efr-stdx` (the clock, the generator, ids, `claim_dir`). The allowlist
does not name `efr-tools`; see above for why. `xtask/src/deps.rs` forbids
`efr-conversation -> efr-shell` and `efr-conversation -> efr-transport`.

Third-party crates: `tokio` (the actor, its turn tasks, channels, `spawn_blocking`),
`futures` (the provider's stream, the first poll of the judge), `async-trait`
(`Toolbox`, `ScopeResolver`, `ExitJudge`),
`serde` and `serde_json` (receipts, tool inputs), `jiff` (the date in a scratch name),
`thiserror` and `tracing`.

## Invariant

- `turn.rs::authorize_tool_call` is the only place where a tool call meets the
  permission engine, and nothing reaches `Toolbox::invoke` without passing it. A
  toolbox declares, `efr_permissions::Engine::decide` decides with the turn's scope,
  origin, permission mode (`ConversationConfig::mode`, read when the turn starts) and
  the conversation's policy, and the turn enforces: `Allow` runs, `Contain` runs in
  the sandbox, `Deny` gives the model an error that names each refused path with its
  class, `Ask` parks the turn on a `oneshot` until the user answers.
- In `auto` nothing runs outside the sandbox without a person's or the classifier's
  allow: a contained call has `Launch::Contained`, and only an allow gives
  `Launch::Unsandboxed`, to a line of one command. An always rule covers only a call
  that the launcher runs, and only the user's own terminal saves one. The classifier never judges a floor,
  a kind that only the user approves, a settings change or an exit that no user message
  names where a name is needed, and its errors ask the user, never allow. A delete in a
  real cache passes the engine (`Engine::decide_cache_delete`) and the same question
  before the toolbox deletes anything. A turn without a working sandbox runs as
  `cautious`, never as `auto` with less.
- The scope is derived again on every turn; it is never cached.
- The last command and the recent commands of a prompt never enter an event or a log
  field; `Debug` of the types that hold them leaves them out. They reach the saved
  messages only inside the turn's preamble, with their secrets redacted
  (`preamble/recent.rs` for the recent commands).
- Every change a request makes is one batch with its receipt, so a retried command
  runs once.
- Time and randomness come from the injected clock and generator.

## Tests

Run the tests of this crate alone, without the rest of the workspace:

```sh
cargo nextest run -p efr-conversation
```

The turn and actor tests run a real actor against the real store in memory, a
`TestClock`, a seeded generator, a `ReplayProvider` that checks every request against
the one the test expects, a fake toolbox and a fake scope resolver: a text turn, a tool
call allowed, denied, asked then approved or denied, a phone turn that asks, the scope
of each turn deciding a write, an interrupt mid-stream, during an approval and during
a tool call, an approval that times out, a provider 401, a cwd move between turns,
provider items passed back to the same provider, also after a restart, and dropped
for another provider, steering, coalesced updates, drafts (`turn/tests/drafts.rs`: their parts
and `after_seq`, coalescing on the clock, the same log with and without a listener, no
timer without one), a queued second prompt, receipts and
the refusals, a steer after the last model call, and the requests of a client that has
just seen the end of a turn (`actor/tests.rs` holds the turn's task after its work
with a tracing layer, so an end recorded too early shows every time). The input row
of a turn (`actor/tests/turn_input.rs`) holds turns with gates, not time: a steer
after an interrupt, a steer as steering closes, a late steer with no running turn,
withdraws by turn and by terminal with their refusals, a withdraw just before and just
after the queued prompt would start, an interrupt that withdraws and resends in one
append (with the prompt of another terminal left queued and a retry that gets the same
sequence numbers), a steer that a model call read, and a refused interrupt that
changes nothing. The context tests (`turn/tests/context.rs`) cover the sums and the
last call's count on the end event, the `context` drafts, the estimate of the next turn
from the end of the last, the cache key, the hard cap with auto off, a small event
page that leaves out no turn, the safety net that scales with the window, a newest
turn that has no end, and the breaker that stays open from turn to turn. The compaction tests (`turn/tests/compaction.rs`,
`actor/tests/compact.rs`, `compaction/tests.rs`) use a model with a window of 100000
tokens, big prompts and big reads: an auto compaction at the trigger that goes on with
the turn, the request of the next turn (an insta snapshot of its shape) and the same
request after a restart, an overflow that compacts once and sends the call again, a
second overflow and an overflow with auto off that fail the turn, a summary after an
overflow even when pruning looks enough, a failed summary under and above the hard
cap (with the provider's sentence), a summary request that leaves out its oldest
messages and the note that the next turn rebuilds, a summary cut off at the output
limit, a retry of the running manual compaction, the breaker, a pruning without a summary that the next
turn rebuilds, a history that would leave out a turn after a switch to a model with a
smaller window and compacts instead, a manual compaction with a focus, a prompt that
waits for it, and its
refusals. The tests of what fills the context (`context/breakdown/tests.rs`,
`actor/tests/context.rs`) check that the parts add up to the request's estimate, that a
pruned output counts as `pruned`, the labels of the 5 largest items, the sums of the
cache, an answer while a turn runs (the provider holds the turn's answer until the
test releases it), that the answer holds no text of a prompt, an output or an answer,
the summary and the same estimate as the compaction's own after a manual compaction,
and an unknown model with the default window. `turn/tests/switch.rs` grows a history of about 500k tokens on a model with
a window of 1M, then goes on with another provider's model of 272k against a fake that
refuses every request above its model's window: the turn compacts before its first
call, the summary request leaves out the oldest prompts and says so in its prompt, and
no request is refused; a history of tool loops shows that no request parts a tool call
from its result; the pure steps (pruning frees at least 20000 tokens or does nothing, the tail
rule, the cut) and the summary prompt's sections have unit tests. The `auto` tests
(`turn/tests/sandbox.rs`) cover a contained call, a
network, write and privilege exit with their launches, a denied exit, the one-command
rule, a user's `ask` rule, the floor refusals that stop a turn at three, the fallback to
`cautious` (no sandbox, a project at home) and the quarantine question (answered,
expired, refused for a phone, interrupted). The classifier tests run a model that
follows a script of calls (`testing::Script`), a judge that answers from a queue and
keeps each record (`testing::FakeJudge`) and a presence that a test sets
(`testing::FakePresence`): which exits the classifier judges and which ask the user
(`turn/tests/review.rs`), the pause after denials and the floor count
(`turn/tests/denials.rs`), the away wait and the phone (`turn/tests/away.rs`), the
always rules (a covered call before the classifier, the offer and the `a` answer, the
phone, a user's `deny` or `ask` rule, the kinds that no rule covers, the pause and the
away wait, a job; `turn/tests/always.rs`), cache
deletes (`turn/tests/cache.rs`), the snapshots and the undo info at the end of a turn
(`turn/tests/sandbox.rs`), and the routine lines of the command corpus of
`efr-permissions`, which make no judge call (`turn/tests/corpus.rs`). `exit/tests.rs`
checks `grant`, the question facts and the record against the real engine, and
`exit/tests/review.rs` the route, the payload, the allow rule and the naming rule,
`exit/tests/naming.rs` the naming rule under the proxy and its window (the turn
before, a `terminal_attached`, a wake turn, the record that keeps every message);
`turn/tests/review.rs` runs the window through turns. The undo of the actor (busy, a call that
runs on, the queue, one record per apply, a phone) is in `actor/tests/undo.rs`. Job
notices and wake turns (the wake limits, the mode, the chain rule, the call cap, the
held notices, the presence of a wake turn) are in `actor/tests/jobs.rs` and the table
tests of `wake/tests.rs`; a job at the check point (what starts at once, what is
refused as a job, the question outside `auto`) is in `turn/tests/jobs.rs`. The calls of tools that a request does
not offer are covered by table tests (`unoffered/tests.rs`) and by turns that move an
`edit` call from a Claude model to an OpenAI model and an `apply_patch` call the other
way and back (`turn/tests/tools.rs`), each request a byte prefix of the next. The
preamble is covered by insta snapshots, and the redaction of the last command by table
tests with the lines that must change and the lines that must stay
(`preamble/secrets/tests.rs`, and the shell words in `preamble/secrets/words/tests.rs`).
The index redaction has table tests of each form it recognises, of the text that
stays and of its known misses (`index_text/tests.rs`).
`append_only.rs` checks that a redacted prompt goes again, byte for byte from the
store, after a restart. The append-only test (`turn/tests/append_only.rs`) runs
64 turns against a model that keeps every request (`testing::Recorder`), with tool
calls, a manual compaction, a restart and more than 4096 events, and checks that each
request starts with the request before it, except the first one after the
compaction. A second one runs under a custom system prompt that says nothing of the
live state: each newest block starts with the rule, and the block of the first prompt
keeps its bytes in every later request, also after a restart. The scratch and resolver
tests use temporary directories; the resolver tests run git, isolated from the user's
configuration. No test uses the network, a real model, real time or the user's home.

## Benches

The `bench_perf_*` benches are ignored. `just bench-perf` runs them in release, one at
a time. They report and do not fail on a time.

- `bench_perf_snapshot_read` (`src/history/tests/perf.rs`): the time of
  `Snapshot::read` and of `window()` on the perf corpus S, M, L and XL of
  `efr-test-support`, the count of `read_turn_range` calls of one read, and the time of
  those calls and of the user messages read alone. The corpus files are built once
  below `target/tmp/perf-corpus/`.

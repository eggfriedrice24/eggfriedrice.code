//! Following a turn: subscribe to the conversation after the prompt's event, show the
//! turn's events as they arrive, answer approvals with one key, and return when the
//! turn ends.
//!
//! Ctrl+C asks the daemon to interrupt the turn (`turn.interrupt`, naming the
//! followed turn) and then ends the command; a closed connection alone would leave the
//! turn running in its conversation. A subscription that falls behind is resumed from
//! the last event shown, so a slow terminal loses nothing.
//!
//! When the running call's command waits for input, the same key thread reads an
//! answer line, which goes to the daemon with `input.respond`. A hidden answer stays in
//! an [`AnswerLine`] and the [`SecretText`] it becomes: it is never handed to the view,
//! never logged, and both are zeroed once it is sent or dropped (the README lists the
//! copies that are not).
//!
//! A call that takes a manual input, reports no wait and prints nothing for [`SILENCE`]
//! gets a line that offers `Ctrl+\`, and the loop waits for the key (`crate::quit`)
//! while that line is shown. No key is read before it: what the user types meanwhile
//! stays typeahead for their shell, unless they press `Ctrl+\`, which makes the terminal
//! throw it away. `Ctrl+\` opens an answer line, which goes as a
//! manual answer. The loop also waits for the key while it reads keys, because the key
//! reader holds the terminal in modes of its own that the key's default action, the
//! end of the process, would leave behind: a press then closes an open manual line
//! unsent, and does nothing while an approval or another answer is asked.
//!
//! The view collects what each event changes, and the loop writes it in frames
//! (`TurnView::frame`): a change shows at once when the last frame is [`FRAME`] old,
//! else when that time is up, so the screen gets at most 60 frames a second however
//! fast drafts and events come. A question, an answer, a key and the end of the turn
//! show at once. While the turn runs, a tick every [`TICK`] moves the status row on,
//! and a resize of the window (SIGWINCH) draws the live zone again at the new width.
//! After a stop (Ctrl+Z, then `fg`, SIGCONT) the live zone starts again below the
//! shell's lines, and the cursor hides again.
//! Every way out writes a last frame, which shows the cursor again and clears the
//! progress bar. SIGTERM and SIGHUP are ways out too: the command ends with that last
//! frame, and `main` then lets the signal take its default action. A panic and the
//! default action of SIGQUIT write what
//! [`TurnView::restore`] last said instead (`crate::output::set_restore`).
//!
//! The subscription asks for drafts: the text, the reasoning and the tool input of the
//! running turn before the daemon records them. They are best effort; the view merges
//! them with the persisted events, which stay the truth.
//!
//! A call that the user allowed here with `y` and whose approval says it may wait for
//! input at the terminal keeps its keys: the reader that read the `y` goes on, and what
//! is typed while the call asks nothing goes into a pending [`AnswerLine`] that is never
//! shown or sent, with Enter dropped. A visible wait of the call takes that line as its
//! answer line, together with the keys still queued, so a `y` typed ahead stands in the
//! line when the question appears and waits for Enter. A hidden wait, a manual line and
//! the call's end drop it, zeroed; the reader then throws away what is still unread.
//!
//! With the input row ([`Row`]), the key thread runs for the whole turn and the keys go
//! to the row whenever nothing else takes them: a question, an answer line, or the keys
//! that a call allowed here keeps. Enter steers the followed turn (`turn.steer`, which
//! the daemon queues as a prompt when it comes too late), Tab queues a prompt
//! (`prompt.send`), Esc interrupts the turn and takes back what this view sent and the
//! turn did not read (`turn.interrupt`), and Alt+Up takes back the newest prompt that
//! this view queued (`prompt.withdraw`). Ctrl+C clears the row; with an empty row it
//! interrupts as before. The view follows each prompt that it queued after the turn
//! before it, and the command ends when the last one ends. Text that is still in the
//! row then goes back to the user's shell (`crate::draft`).

mod view;

use std::time::Duration;

use efr_client::{Client, ClientError, ItemStream};
use efr_protocol::{
    ApprovalDecision, ApprovalRespond, ApprovalRespondResult, CallId, ConversationHistory,
    ConversationHistoryResult, ConversationId, ConversationSubscribe, ConversationSubscribeItem,
    ErrorCode, Event, InputRespond, InputRespondResult, LateSteer, Method, PromptSend,
    PromptSendResult, PromptWithdraw, PromptWithdrawResult, QuestionId, SandboxSurfaceRespond,
    SandboxSurfaceRespondResult, SecretText, Seq, ShellContext, TurnId, TurnInterrupt,
    TurnInterruptResult, TurnSettings, TurnSteer, TurnSteerResult, WithdrawTarget, WithdrawnPrompt,
};
use efr_stdx::time::{Clock as _, Sleep};
use futures::StreamExt as _;
use jiff::Timestamp;
use serde_json::Value;

use crate::answer::{AnswerLine, Edit};
use crate::cli::LastCommand;
use crate::context::{Context, Stop};
use crate::error::CliError;
use crate::keys::{self, Key, KeyReader};
use crate::output::Output;
use crate::row::Action;
use view::AnswerKind;

pub(crate) use view::{Ask, Look, Step, TICK, TurnEnd, TurnView};

/// What the input row sends with: the shell context, the last command line and the
/// turn settings that the plugin handed to this command, so a prompt or a late steer
/// from the row goes as a prompt from this terminal would.
#[derive(Debug, Clone)]
pub(crate) struct Compose {
    pub(crate) context: ShellContext,
    pub(crate) last_command: Option<LastCommand>,
    pub(crate) settings: TurnSettings,
}

/// The input row of a followed turn: what it sends with, and its key reader, which
/// started before the prompt went out, so keys typed meanwhile land in the row.
#[derive(Debug)]
pub(crate) struct Row {
    pub(crate) compose: Compose,
    pub(crate) reader: KeyReader,
}

/// How often in a row a subscription may fall behind before the command gives up.
const MAX_RESUBSCRIBES: u32 = 8;

/// The events read to find the approvals that a queued prompt waits behind.
const BLOCKING_PAGE: u32 = 500;

/// How long Ctrl+C waits for the daemon to take the interrupt before the command ends
/// anyway.
const INTERRUPT_TIMEOUT: Duration = Duration::from_secs(3);

/// How long a shell call that reports no wait prints nothing before the view offers
/// `Ctrl+\` to type an input for it.
pub(crate) const SILENCE: Duration = Duration::from_secs(10);

/// The shortest time between two frames: at most 60 a second.
pub(crate) const FRAME: Duration = Duration::from_millis(16);

/// The time from `then` to `now`; zero when `now` is earlier. The view and its parts
/// count their times with it too.
pub(crate) fn since_then(then: Timestamp, now: Timestamp) -> Duration {
    Duration::try_from(now.duration_since(then)).unwrap_or_default()
}

/// Where the turn to follow is.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Target {
    pub(crate) conversation: ConversationId,
    /// The turn, which Ctrl+C interrupts.
    pub(crate) turn: TurnId,
    /// The sequence number of the prompt's event; the turn's events come after it.
    pub(crate) after: Seq,
}

/// Follows `target` on `client` until the turn ends, writing it to `out`. With `row`,
/// the view's input row reads keys for the whole turn, and the view follows the
/// prompts that it queues until the last one ends.
pub(crate) async fn follow(
    ctx: &Context,
    client: &Client,
    out: &mut Output,
    view: &mut TurnView,
    target: Target,
    row: Option<Row>,
) -> Result<(), CliError> {
    let (compose, keys) = match row {
        Some(Row { compose, reader }) if view.has_row() => {
            (Some(compose), Some((reader, Asking::Row)))
        }
        Some(Row { reader, .. }) => {
            reader.stop().await;
            (None, None)
        }
        None => (None, None),
    };
    let mut follower = Follower {
        ctx,
        client,
        target,
        last_seen: target.after,
        keys,
        compose,
        escaped: None,
        silence: None,
        quit: None,
        last_frame: None,
        frame: None,
        tick: None,
    };
    // The first frame shows the status row and the notes before the first event.
    let result = match follower.paint(out, view, false) {
        Ok(()) => match follower.blocking(out, view).await {
            Ok(()) => follower.run(out, view).await,
            Err(error) => Err(error),
        },
        Err(error) => Err(error),
    };
    // NOTE: the terminal must be back in its normal mode before anything else is
    // written or the process exits, whichever way the loop ended.
    if let Some((mut keys, asking)) = follower.keys.take() {
        if matches!(asking, Asking::Row) {
            // Keys that the row did not take yet are part of its text.
            while let Some(key) = keys.queued() {
                view.row_key(key);
            }
        }
        stop(keys, &asking).await;
    }
    if let Err(error) = &result {
        // The last frame shows the cursor again and clears the progress bar before the
        // interrupt waits for the daemon.
        let step = view.close();
        follower.show(out, view, &step)?;
        if matches!(error, CliError::Interrupted) {
            // The time of the Ctrl+C, not of the daemon's answer.
            let stopped = ctx.clock.now();
            // Prompts that this view queued would run with nobody to follow them: they
            // come back with the steers that no model call read, to the user's shell.
            let withdraw = view.queued_turns();
            let note =
                match interrupt(ctx, client, target.conversation, view.turn(), withdraw).await {
                    Ok(withdrawn) => {
                        for text in view.take_unread() {
                            view.row_append(&text);
                        }
                        for prompt in withdrawn {
                            view.withdrawn(prompt.turn_id, &prompt.text);
                        }
                        view.interrupted(stopped)
                    }
                    Err(note) => note,
                };
            let step = view.note(&note, ctx.screen.size());
            follower.show(out, view, &step)?;
        }
    }
    let text = view.row_text().to_owned();
    if let Some(note) = crate::draft::hand_back(ctx, &text).await {
        let step = view.note(&note, ctx.screen.size());
        follower.show(out, view, &step)?;
    }
    crate::output::set_restore(&view.restore());
    result
}

/// Asks the daemon to stop `turn` and to take back the prompts of `withdraw`, which this
/// view queued; the prompts it took back, or how that failed.
async fn interrupt(
    ctx: &Context,
    client: &Client,
    conversation: ConversationId,
    turn: TurnId,
    withdraw: Vec<TurnId>,
) -> Result<Vec<WithdrawnPrompt>, String> {
    let method = Method::TurnInterrupt(TurnInterrupt {
        command_id: ctx.command_id(),
        conversation_id: conversation,
        turn_id: Some(turn),
        resend_steers: Vec::new(),
        withdraw,
    });
    let call = client.call::<TurnInterruptResult>(method);
    match ctx.clock.timeout(INTERRUPT_TIMEOUT, call).await {
        Ok(Ok(result)) => Ok(result.withdrawn),
        // The turn already ended, or it still waits behind another turn, which the
        // daemon cannot take back yet.
        Ok(Err(ClientError::Server { body })) if body.code == ErrorCode::Conflict => {
            Err("not interrupted: the turn is not running; a queued prompt still runs in its turn"
                .to_owned())
        }
        Ok(Err(error)) => {
            tracing::debug!(error = %error, "turn.interrupt failed");
            Err(format!("the interrupt failed: {}", crate::format::one_line(&error.to_string())))
        }
        Err(_) => {
            Err("the daemon did not confirm the interrupt; the turn may still run".to_owned())
        }
    }
}

struct Follower<'a> {
    ctx: &'a Context,
    client: &'a Client,
    target: Target,
    last_seen: Seq,
    /// Reads keys while an approval question or an input is pending, and for the whole
    /// turn with the input row.
    keys: Option<(KeyReader, Asking)>,
    /// What the input row sends with; `None` without the row.
    compose: Option<Compose>,
    /// The turn that Esc interrupted or took back: when it is the last to end, the
    /// command ends as after Ctrl+C.
    escaped: Option<TurnId>,
    /// The silent shell call and its activity count, with the time until it offers
    /// `Ctrl+\`.
    silence: Option<((CallId, u64), Sleep)>,
    /// Waits for `Ctrl+\` while the view offers it, and only then.
    quit: Option<Stop>,
    /// The time of the last frame.
    last_frame: Option<Timestamp>,
    /// The time until the next frame, while a change waits for it.
    frame: Option<Sleep>,
    /// The time until the next tick of the status row, while it runs.
    tick: Option<Sleep>,
}

/// What the keys being read answer.
#[derive(Debug)]
enum Asking {
    /// An approval, with one key.
    Approval(CallId),
    /// The input that a running call waits for, with a line.
    Input { call_id: CallId, kind: AnswerKind, line: AnswerLine },
    /// Nothing: the keys are thrown away until the call that asked for a hidden answer
    /// completes.
    Discard,
    /// Nothing yet: the keys typed for call `call_id`, which the user allowed here as one
    /// that may wait for input, wait in `line` for a visible wait of the call.
    Pending { call_id: CallId, line: AnswerLine },
    /// The quarantine question, with one key.
    Surface(QuestionId),
    /// The input row: nothing else takes the keys.
    Row,
}

impl Asking {
    /// True when the keys read for it may hold a password or the rest of one: before
    /// they go to the input row, what is still unread is thrown away.
    fn guards(&self) -> bool {
        matches!(self, Asking::Input { .. } | Asking::Discard | Asking::Pending { .. })
    }
}

/// What an answer line starts with when it takes over the pending line of its call.
#[derive(Debug, PartialEq, Eq)]
enum Seed {
    /// The text of a shown answer line, which the view echoes.
    Shown(String),
    /// The number of characters in a line that is not shown, which a note gives: a
    /// stray key typed ahead in front of a password would fail it unseen.
    Unshown(usize),
}

/// Stops `keys`. A reader that read an answer line throws away what is still unread,
/// so the rest of a password neither shows nor reaches the user's shell.
async fn stop(keys: KeyReader, asking: &Asking) {
    match asking {
        Asking::Approval(_) | Asking::Surface(_) | Asking::Row => keys.stop().await,
        Asking::Input { .. } | Asking::Discard | Asking::Pending { .. } => {
            keys.stop_discarding().await;
        }
    }
}

/// Feeds `key` to the pending `line` of a call that asks nothing yet. Enter is dropped:
/// a line typed ahead is sent only by an Enter typed after its question appeared.
fn pend(line: &mut AnswerLine, key: u8) {
    if !matches!(key, b'\r' | b'\n') {
        line.key(key);
    }
}

impl Follower<'_> {
    /// Shows the approvals that the turn ahead of a queued prompt waits for, and the
    /// input its running call waits for, so the user can answer them here. They were
    /// asked before the prompt's event, where the subscription starts, so they come
    /// from the newest page of the log; one older than that page is not found.
    async fn blocking(&mut self, out: &mut Output, view: &mut TurnView) -> Result<(), CliError> {
        if !view.is_queued() {
            return Ok(());
        }
        let method = Method::ConversationHistory(ConversationHistory {
            conversation_id: self.target.conversation,
            cursor: None,
            limit: Some(BLOCKING_PAGE),
        });
        let page = match self.client.call::<ConversationHistoryResult>(method).await {
            Ok(page) => page,
            Err(error) => {
                tracing::debug!(error = %error, "the approvals ahead of the prompt could not be read");
                return Ok(());
            }
        };
        let mut pending: Vec<Event> = Vec::new();
        // The running call's last output and input wait; tool calls run one at a time.
        let mut call: Option<CallId> = None;
        let mut output: Option<Event> = None;
        let mut input: Option<Event> = None;
        for envelope in page.events.into_iter().filter(|envelope| envelope.seq <= self.target.after)
        {
            match &envelope.event {
                // The record of an exit comes before its approval and shows in it.
                Event::ApprovalRequested { turn_id, .. }
                | Event::ExitRequested { turn_id, .. }
                | Event::SurfaceQuestionRequested { turn_id, .. }
                    if *turn_id != self.target.turn =>
                {
                    pending.push(envelope.event);
                }
                Event::SurfaceQuestionAnswered { question_id, .. } => {
                    pending.retain(|event| {
                        !matches!(event, Event::SurfaceQuestionRequested { question_id: asked, .. } if asked == question_id)
                    });
                }
                Event::ToolCallOutputUpdated { turn_id, call_id, .. }
                | Event::ToolCallInputChanged { turn_id, call_id, .. }
                    if *turn_id != self.target.turn =>
                {
                    if call != Some(*call_id) {
                        call = Some(*call_id);
                        output = None;
                        input = None;
                    }
                    if matches!(envelope.event, Event::ToolCallInputChanged { .. }) {
                        input = Some(envelope.event);
                    } else {
                        output = Some(envelope.event);
                    }
                }
                Event::ToolCallCompleted { call_id, .. } if call == Some(*call_id) => {
                    call = None;
                    output = None;
                    input = None;
                }
                Event::ApprovalResolved { call_id, .. }
                | Event::ApprovalExpired { call_id, .. } => {
                    pending.retain(|event| {
                        !matches!(event, Event::ApprovalRequested { call_id: asked, .. } if asked == call_id)
                    });
                }
                _ => {}
            }
        }
        for event in pending.into_iter().chain(output).chain(input) {
            let step = view.event(&event, self.ctx.screen.size(), self.ctx.keys.available());
            self.apply(step, out, view, true).await?;
        }
        Ok(())
    }

    /// Writes a frame of `view` now; a tick's frame with `tick`.
    fn paint(&mut self, out: &mut Output, view: &mut TurnView, tick: bool) -> Result<(), CliError> {
        // The input row shows while its keys come to it, and hides while a question or
        // an answer line takes them.
        view.show_row(matches!(self.keys, Some((_, Asking::Row))));
        let now = self.ctx.clock.now();
        let size = self.ctx.screen.size();
        let frame = if tick { view.tick(size, now) } else { view.frame(size, now) };
        self.last_frame = Some(now);
        // Text that still waits to show asks for the next frame.
        self.frame = view.wants_frame().then(|| self.ctx.clock.sleep(FRAME));
        // NOTE: set before the write, so a panic during it still shows the cursor.
        crate::output::set_restore(&view.restore());
        out.out(&frame)
    }

    /// Writes a frame now when the last one is [`FRAME`] old, else when that time is
    /// up, if the view has something to show.
    fn schedule(&mut self, out: &mut Output, view: &mut TurnView) -> Result<(), CliError> {
        if !view.wants_frame() || self.frame.is_some() {
            return Ok(());
        }
        let now = self.ctx.clock.now();
        let since = self.last_frame.map(|last| since_then(last, now));
        match since {
            Some(since) if since < FRAME => {
                self.frame = Some(self.ctx.clock.sleep(FRAME - since));
                Ok(())
            }
            _ => self.paint(out, view, false),
        }
    }

    /// Writes `step` and a frame of `view` at once.
    fn show(&mut self, out: &mut Output, view: &mut TurnView, step: &Step) -> Result<(), CliError> {
        write(out, step)?;
        self.paint(out, view, false)
    }

    /// Keeps a tick waiting while the status row runs, and none otherwise.
    fn watch_tick(&mut self, view: &TurnView) {
        if !view.ticks() {
            self.tick = None;
        } else if self.tick.is_none() {
            self.tick = Some(self.ctx.clock.sleep(TICK));
        }
    }

    async fn run(&mut self, out: &mut Output, view: &mut TurnView) -> Result<(), CliError> {
        let mut interrupt = self.ctx.interrupt.wait();
        let mut resizes = self.ctx.resize.resizes();
        let mut resumes = self.ctx.resume.resumes();
        let mut ending = self.ctx.terminate.wait();
        let mut resubscribes = 0;
        loop {
            let mut stream = self.subscribe().await?;
            let resubscribe = loop {
                self.watch_silence(view);
                self.watch_tick(view);
                tokio::select! {
                    () = &mut interrupt => {
                        // Ctrl+C clears the text of the input row first.
                        if matches!(self.keys, Some((_, Asking::Row))) && view.row_clear() {
                            interrupt = self.ctx.interrupt.wait();
                            self.paint(out, view, false)?;
                        } else {
                            return Err(CliError::Interrupted);
                        }
                    }
                    signal = &mut ending => {
                        return Err(CliError::Ended { signal });
                    }
                    key = next_key(&mut self.keys) => {
                        self.key(key, out, view).await?;
                    }
                    call_id = silent(&mut self.silence) => {
                        self.silence = None;
                        let step = view.silent(call_id, self.ctx.screen.size());
                        self.apply(step, out, view, false).await?;
                    }
                    () = due(&mut self.frame) => {
                        self.paint(out, view, false)?;
                    }
                    () = due(&mut self.tick) => {
                        self.tick = None;
                        self.paint(out, view, true)?;
                    }
                    Some(()) = resizes.next() => {
                        self.paint(out, view, false)?;
                    }
                    Some(()) = resumes.next() => {
                        view.resumed();
                        if let Some((reader, _)) = &self.keys {
                            reader.resumed();
                        }
                        self.paint(out, view, false)?;
                    }
                    () = pressed(&mut self.quit) => {
                        self.quit = None;
                        self.pressed(out, view).await?;
                    }
                    item = stream.next() => match item {
                        Some(Ok(value)) => {
                            resubscribes = 0;
                            if let Some(end) = self.item(value, out, view).await? {
                                return self.finished(end, view);
                            }
                        }
                        Some(Err(ClientError::Server { body })) if body.code == ErrorCode::Overflow => {
                            break true;
                        }
                        Some(Err(ClientError::StreamOverflow { .. })) => break true,
                        Some(Err(error)) => return Err(error.into()),
                        None => return Err(CliError::SubscriptionEnded),
                    },
                }
            };
            if resubscribe {
                resubscribes += 1;
                if resubscribes > MAX_RESUBSCRIBES {
                    return Err(CliError::FellBehind { times: resubscribes });
                }
                tracing::debug!(after = %self.last_seen, "the subscription fell behind; resuming");
            }
        }
    }

    /// Times the silence of the call that may offer `Ctrl+\`, from its last sign of
    /// life, and waits for the key while the view offers it or keys are read. Without a
    /// terminal to read keys from, nothing is offered.
    fn watch_silence(&mut self, view: &TurnView) {
        let candidate = if self.ctx.keys.available() { view.silence() } else { None };
        if self.silence.as_ref().map(|(key, _)| *key) != candidate {
            self.silence = candidate.map(|key| (key, self.ctx.clock.sleep(SILENCE)));
        }
        let wanted = view.manual_offer().is_some() || self.keys.is_some();
        if wanted != self.quit.is_some() {
            self.quit = wanted.then(|| self.ctx.quit.wait());
        }
    }

    /// `Ctrl+\` was pressed: it opens a manual answer line while the view offers one,
    /// closes an open one unsent, and does nothing while anything else reads keys.
    async fn pressed(&mut self, out: &mut Output, view: &mut TurnView) -> Result<(), CliError> {
        let size = self.ctx.screen.size();
        let step = if let Some(call_id) = view.manual_offer() {
            view.manual(call_id, size)
        } else if view.manual_open() {
            view.manual_cancelled(size)
        } else {
            return Ok(());
        };
        self.apply(step, out, view, false).await.map(drop)
    }

    async fn subscribe(&self) -> Result<ItemStream<Value>, CliError> {
        let method = Method::ConversationSubscribe(ConversationSubscribe {
            conversation_id: self.target.conversation,
            after_seq: Some(self.last_seen),
            // A person at this terminal can type an answer exactly when approvals can be
            // answered here; without one, the daemon stops a command that waits for a
            // password nobody can type.
            answers_input: self.ctx.keys.available(),
            // The text, the reasoning and the tool input of the running turn before the
            // daemon records them; an older daemon sends none.
            drafts: true,
        });
        Ok(self.client.stream(method).await?)
    }

    /// Shows one subscription item; the turn's end when it ended.
    async fn item(
        &mut self,
        value: Value,
        out: &mut Output,
        view: &mut TurnView,
    ) -> Result<Option<TurnEnd>, CliError> {
        let (events, hwm) = match serde_json::from_value::<ConversationSubscribeItem>(value) {
            Ok(ConversationSubscribeItem::Event(envelope)) => (vec![envelope], None),
            Ok(ConversationSubscribeItem::Snapshot(snapshot)) => {
                (snapshot.events, Some(snapshot.hwm))
            }
            Ok(ConversationSubscribeItem::Draft(draft)) => {
                let step = view.draft(&draft, self.ctx.screen.size());
                self.apply(step, out, view, true).await?;
                return Ok(None);
            }
            Ok(_) | Err(_) => {
                tracing::debug!("skipped a subscription item of a kind this build does not know");
                return Ok(None);
            }
        };
        for envelope in events {
            if envelope.seq <= self.last_seen {
                continue;
            }
            self.last_seen = envelope.seq;
            let step = view.envelope(&envelope, self.ctx.screen.size(), self.ctx.keys.available());
            if let Some(end) = self.apply(step, out, view, true).await? {
                return Ok(Some(end));
            }
        }
        // A snapshot is complete up to its high-water mark, so a resumed subscription
        // starts there.
        if let Some(hwm) = hwm {
            self.last_seen = self.last_seen.max(hwm);
        }
        Ok(None)
    }

    /// Writes a step and starts or stops reading keys as it says. The frame goes out at
    /// once unless the step is `paced` and asks, settles and ends nothing: a question
    /// never waits, and the prompt comes back as soon as the turn ends.
    async fn apply(
        &mut self,
        step: Step,
        out: &mut Output,
        view: &mut TurnView,
        paced: bool,
    ) -> Result<Option<TurnEnd>, CliError> {
        write(out, &step)?;
        let urgent = !paced || step.ask.is_some() || step.settled || step.end.is_some();
        match step.ask {
            Some(ask) => {
                let (reader, before) = match self.keys.take() {
                    Some((mut reader, Asking::Row)) => {
                        // NOTE: the keys typed before the question are the row's; a key
                        // that would send stays in the row instead.
                        while let Some(key) = reader.queued() {
                            view.row_key(key);
                        }
                        (reader, Some(Asking::Row))
                    }
                    Some((reader, before)) => (reader, Some(before)),
                    // Starting the reader discards typeahead, so nothing typed before
                    // the question answers it or stays queued for the shell.
                    None => (self.ctx.keys.start()?, None),
                };
                // NOTE: the reader is held here before anything can fail, so the exit
                // path still stops it and restores the terminal.
                let (reader, asking, seeded) = take_over(reader, before, ask);
                self.keys = Some((reader, asking));
                let size = self.ctx.screen.size();
                match seeded {
                    Some(Seed::Shown(text)) => write(out, &view.typed(&text))?,
                    Some(Seed::Unshown(count)) => write(out, &view.typed_ahead(count, size))?,
                    None => {}
                }
            }
            None if step.end.is_some() => {
                if let Some((mut keys, asking)) = self.keys.take() {
                    if matches!(asking, Asking::Row) {
                        // Keys that the row did not take yet are part of its text.
                        while let Some(key) = keys.queued() {
                            view.row_key(key);
                        }
                    }
                    stop(keys, &asking).await;
                }
            }
            None if step.settled => match self.keys.take() {
                // The row asked for nothing, so nothing of it is settled.
                Some((reader, Asking::Row)) => self.keys = Some((reader, Asking::Row)),
                Some((reader, asking)) => self.release(reader, &asking, view).await,
                None => {}
            },
            None => {}
        }
        if urgent {
            self.paint(out, view, false)?;
        } else {
            self.schedule(out, view)?;
        }
        Ok(step.end)
    }

    /// Handles one key; `None` means the key source ended.
    async fn key(
        &mut self,
        key: Option<Key>,
        out: &mut Output,
        view: &mut TurnView,
    ) -> Result<(), CliError> {
        let Some((reader, asking)) = self.keys.take() else {
            return Ok(());
        };
        let Some(key) = key else {
            stop(reader, &asking).await;
            return Ok(());
        };
        if matches!(asking, Asking::Row) {
            self.keys = Some((reader, Asking::Row));
            return self.row_key(key, out, view).await;
        }
        // A line or a question reads bytes; Esc alone is the escape byte there.
        let key = key.byte();
        match asking {
            Asking::Row => Ok(()),
            Asking::Discard => {
                self.keys = Some((reader, Asking::Discard));
                Ok(())
            }
            Asking::Pending { call_id, mut line } => {
                pend(&mut line, key);
                self.keys = Some((reader, Asking::Pending { call_id, line }));
                Ok(())
            }
            Asking::Approval(call_id) => {
                let Some(decision) = keys::decision(key) else {
                    self.keys = Some((reader, Asking::Approval(call_id)));
                    return Ok(());
                };
                let step = view.answered(call_id, decision, self.ctx.screen.size());
                match step.ask {
                    // The call may wait for input: the reader goes on, and the keys
                    // typed after the `y` are kept for it.
                    Some(Ask::Retain(kept)) => {
                        let line = AnswerLine::new();
                        self.keys = Some((reader, Asking::Pending { call_id: kept, line }));
                    }
                    _ => self.release(reader, &Asking::Approval(call_id), view).await,
                }
                self.show(out, view, &step)?;
                self.respond(call_id, decision, out, view).await
            }
            Asking::Surface(question_id) => {
                let Some(decision) = keys::decision(key) else {
                    self.keys = Some((reader, Asking::Surface(question_id)));
                    return Ok(());
                };
                self.release(reader, &Asking::Surface(question_id), view).await;
                let keep = decision == ApprovalDecision::Allow;
                let step = view.surface_answered(question_id, keep, self.ctx.screen.size());
                self.show(out, view, &step)?;
                self.surface_respond(question_id, keep, out, view).await
            }
            Asking::Input { call_id, kind, mut line } => {
                let edit = line.key(key);
                let shown = (kind.shown() && edit == Edit::Changed).then(|| line.text().to_owned());
                let text = (edit == Edit::Submit).then(|| line.take());
                // NOTE: the reader goes back before anything can fail, so the exit path
                // still stops it and restores the terminal.
                self.keys = Some((reader, Asking::Input { call_id, kind, line }));
                if let Some(shown) = shown {
                    let step = view.typed(&shown);
                    self.show(out, view, &step)?;
                }
                match text {
                    Some(text) => self.answer(call_id, kind, text, out, view).await,
                    None => Ok(()),
                }
            }
        }
    }

    /// How the command ends after the last followed turn ended as `end`. A turn that Esc
    /// stopped ends it as Ctrl+C does.
    fn finished(&self, end: TurnEnd, view: &TurnView) -> Result<(), CliError> {
        if self.escaped == Some(view.turn())
            && matches!(end, TurnEnd::Interrupted | TurnEnd::Withdrawn)
        {
            return Err(CliError::Escaped);
        }
        match end {
            TurnEnd::Completed | TurnEnd::Withdrawn => Ok(()),
            TurnEnd::Failed(body) => Err(CliError::TurnFailed { body }),
            TurnEnd::Interrupted => Err(CliError::TurnInterrupted),
            TurnEnd::Cancelled => Err(CliError::TurnCancelled),
        }
    }

    /// Nothing asks for keys any more: they go back to the input row, after the keys
    /// that may be the rest of a password are thrown away. Without the row, the reader
    /// stops.
    async fn release(&mut self, mut reader: KeyReader, before: &Asking, view: &TurnView) {
        if self.compose.is_some() && view.has_row() {
            if before.guards() {
                reader.flush();
            }
            self.keys = Some((reader, Asking::Row));
        } else {
            stop(reader, before).await;
        }
    }

    /// One key of the input row.
    async fn row_key(
        &mut self,
        key: Key,
        out: &mut Output,
        view: &mut TurnView,
    ) -> Result<(), CliError> {
        match view.row_key(key) {
            Action::None => Ok(()),
            Action::Edited => self.paint(out, view, false),
            Action::Steer => self.steer(out, view).await,
            Action::Queue => self.queue(out, view).await,
            Action::Interrupt => self.escape(out, view).await,
            Action::Withdraw => self.withdraw(out, view).await,
        }
    }

    /// Enter: the text of the row steers the followed turn. A steer that comes too
    /// late becomes a prompt queued behind it, as Tab would send it.
    async fn steer(&mut self, out: &mut Output, view: &mut TurnView) -> Result<(), CliError> {
        let Some(compose) = self.compose.clone() else {
            return Ok(());
        };
        if !view.row_ready() {
            return Ok(());
        }
        let text = view.row_take();
        let method = Method::TurnSteer(TurnSteer {
            command_id: self.ctx.command_id(),
            conversation_id: self.target.conversation,
            turn_id: Some(view.turn()),
            text: text.clone(),
            if_late: Some(LateSteer::Queue {
                context: Some(compose.context),
                last_command: compose.last_command.map(LastCommand::into_string),
                settings: compose.settings,
            }),
        });
        let note = match self.client.call::<TurnSteerResult>(method).await {
            Ok(result) if result.queued => {
                view.queued_prompt(result.turn_id, text, true);
                None
            }
            Ok(result) => {
                view.steered(result.seq, text);
                None
            }
            Err(error) => {
                view.row_append(&text);
                Some(match server_error(error)? {
                    body if body.code == ErrorCode::Conflict => {
                        "not sent: the turn takes no more steers; press Tab to queue the text"
                            .to_owned()
                    }
                    body => format!("not sent: {}", crate::format::one_line(&body.message)),
                })
            }
        };
        self.row_done(note, out, view).await
    }

    /// Tab: the text of the row goes as a prompt queued behind the running turn.
    async fn queue(&mut self, out: &mut Output, view: &mut TurnView) -> Result<(), CliError> {
        let Some(compose) = self.compose.clone() else {
            return Ok(());
        };
        if !view.row_ready() {
            return Ok(());
        }
        let text = view.row_take();
        let method = Method::PromptSend(PromptSend {
            command_id: self.ctx.command_id(),
            conversation_id: Some(self.target.conversation),
            new_conversation: false,
            text: text.clone(),
            context: Some(compose.context),
            last_command: compose.last_command.map(LastCommand::into_string),
            settings: compose.settings,
        });
        let note = match self.client.call::<PromptSendResult>(method).await {
            Ok(result) => {
                view.queued_prompt(result.turn_id, text, false);
                None
            }
            Err(error) => {
                view.row_append(&text);
                let body = server_error(error)?;
                Some(format!("not queued: {}", crate::format::one_line(&body.message)))
            }
        };
        self.row_done(note, out, view).await
    }

    /// Esc: interrupts the followed turn. The steers of this view that no model call
    /// read go again as one prompt, which runs next, and the prompts that this view
    /// queued come back into the row, all in one step of the daemon. A followed prompt
    /// that did not start yet is taken back instead, with the prompts behind it.
    async fn escape(&mut self, out: &mut Output, view: &mut TurnView) -> Result<(), CliError> {
        if view.is_queued() {
            return self.take_back_all(out, view).await;
        }
        let turn = view.turn();
        let method = Method::TurnInterrupt(TurnInterrupt {
            command_id: self.ctx.command_id(),
            conversation_id: self.target.conversation,
            turn_id: Some(turn),
            resend_steers: view.unread_steers(),
            withdraw: view.queued_turns(),
        });
        let size = self.ctx.screen.size();
        let step = match self.client.call::<TurnInterruptResult>(method).await {
            Ok(result) => {
                self.escaped = Some(turn);
                view.interrupt_result(&result, size)
            }
            Err(error) => match server_error(error)? {
                body if body.code == ErrorCode::Conflict => {
                    view.note("not interrupted: the turn is not running", size)
                }
                body => view.note(
                    &format!("the interrupt failed: {}", crate::format::one_line(&body.message)),
                    size,
                ),
            },
        };
        self.apply(step, out, view, false).await.map(drop)
    }

    /// Esc while the followed prompt waits behind another turn: takes it back, and the
    /// prompts that this view queued after it, into the row. Its `prompt_withdrawn`
    /// then ends it, and with it the command.
    async fn take_back_all(
        &mut self,
        out: &mut Output,
        view: &mut TurnView,
    ) -> Result<(), CliError> {
        let followed = view.turn();
        let mut notes = Vec::new();
        for turn in std::iter::once(followed).chain(view.queued_turns()) {
            match self.take_back(turn).await? {
                TakeBack::Taken(prompt) if turn == followed => {
                    self.escaped = Some(turn);
                    view.row_append(&prompt.text);
                }
                TakeBack::Taken(prompt) => view.withdrawn(turn, &prompt.text),
                TakeBack::Gone(note) | TakeBack::Refused(note) => notes.push(note),
            }
        }
        let size = self.ctx.screen.size();
        let mut step = Step::default();
        // One note says it for all.
        if let Some(note) = notes.first() {
            step = view.note(note, size);
        }
        self.apply(step, out, view, false).await.map(drop)
    }

    /// Alt+Up: takes back the newest prompt that this view queued, into the row.
    async fn withdraw(&mut self, out: &mut Output, view: &mut TurnView) -> Result<(), CliError> {
        let Some(turn) = view.newest_queued() else {
            return Ok(());
        };
        let note = match self.take_back(turn).await? {
            TakeBack::Taken(prompt) => {
                view.withdrawn(turn, &prompt.text);
                None
            }
            TakeBack::Gone(note) => {
                view.drop_queued(turn);
                Some(note)
            }
            TakeBack::Refused(note) => Some(note),
        };
        self.row_done(note, out, view).await
    }

    /// Asks the daemon to take back the queued prompt of `turn`.
    async fn take_back(&self, turn: TurnId) -> Result<TakeBack, CliError> {
        let method = Method::PromptWithdraw(PromptWithdraw {
            command_id: self.ctx.command_id(),
            conversation_id: self.target.conversation,
            target: WithdrawTarget::Turn { turn_id: turn },
        });
        match self.client.call::<PromptWithdrawResult>(method).await {
            Ok(result) => Ok(TakeBack::Taken(result.withdrawn)),
            Err(error) => Ok(match server_error(error)? {
                body if body.code == ErrorCode::Conflict => {
                    TakeBack::Gone("not taken back: the prompt already started".to_owned())
                }
                body if body.code == ErrorCode::NotFound => {
                    TakeBack::Gone("not taken back: the daemon has no such prompt".to_owned())
                }
                body => TakeBack::Refused(format!(
                    "not taken back: {}",
                    crate::format::one_line(&body.message)
                )),
            }),
        }
    }

    /// Shows what a key of the row did: `note`, when there is one, and a frame.
    async fn row_done(
        &mut self,
        note: Option<String>,
        out: &mut Output,
        view: &mut TurnView,
    ) -> Result<(), CliError> {
        match note {
            Some(note) => {
                let step = view.note(&note, self.ctx.screen.size());
                self.apply(step, out, view, false).await.map(drop)
            }
            None => self.paint(out, view, false),
        }
    }

    /// Sends the answer line `text` to the running call `call_id`, and says how that
    /// went. The `SecretText` is dropped, and its buffer zeroed, once the call returns;
    /// `efr-client` zeroes the encoded request frame once it is written. A manual answer
    /// asks once, so its keys stop after it.
    async fn answer(
        &mut self,
        call_id: CallId,
        kind: AnswerKind,
        text: SecretText,
        out: &mut Output,
        view: &mut TurnView,
    ) -> Result<(), CliError> {
        // NOTE: an answer that is not shown because its prompt looks secret still goes as
        // a visible one, the kind of the wait that the daemon reported.
        let method = Method::InputRespond(InputRespond {
            conversation_id: self.target.conversation,
            call_id,
            text,
            hidden: kind.hidden(),
            manual: kind.manual(),
        });
        let size = self.ctx.screen.size();
        let step = match self.client.call::<InputRespondResult>(method).await {
            Ok(_) => view.answer_sent(kind, size),
            // The command ended or stopped reading, or the call is gone: the daemon
            // wrote nothing.
            Err(ClientError::Server { body })
                if matches!(body.code, ErrorCode::NotFound | ErrorCode::Conflict) =>
            {
                view.answer_refused(size)
            }
            Err(ClientError::Server { body }) => view.answer_failed(&body.message, size),
            Err(error) => return Err(error.into()),
        };
        self.apply(step, out, view, false).await.map(drop)
    }

    async fn respond(
        &mut self,
        call_id: CallId,
        decision: ApprovalDecision,
        out: &mut Output,
        view: &mut TurnView,
    ) -> Result<(), CliError> {
        let method = Method::ApprovalRespond(ApprovalRespond {
            command_id: self.ctx.command_id(),
            conversation_id: self.target.conversation,
            call_id,
            decision,
        });
        match self.client.call::<ApprovalRespondResult>(method).await {
            Ok(_) => Ok(()),
            // Answered elsewhere first, or expired: the events say which.
            Err(ClientError::Server { body })
                if matches!(body.code, ErrorCode::NotFound | ErrorCode::Conflict) =>
            {
                let line =
                    format!("the answer was not taken: {}", crate::format::one_line(&body.message));
                let step = view.note(&line, self.ctx.screen.size());
                self.show(out, view, &step)
            }
            Err(error) => Err(error.into()),
        }
    }

    /// Sends the answer to the quarantine question `question_id`. A "yes" moves the git
    /// change back; anything else leaves it in quarantine.
    async fn surface_respond(
        &mut self,
        question_id: QuestionId,
        keep: bool,
        out: &mut Output,
        view: &mut TurnView,
    ) -> Result<(), CliError> {
        let method = Method::SandboxSurfaceRespond(SandboxSurfaceRespond {
            command_id: self.ctx.command_id(),
            conversation_id: self.target.conversation,
            question_id,
            keep,
        });
        match self.client.call::<SandboxSurfaceRespondResult>(method).await {
            Ok(_) => Ok(()),
            // Answered elsewhere first, expired, or refused for this client: the events
            // say which, and the change stays in quarantine unless one says otherwise.
            Err(ClientError::Server { body })
                if matches!(
                    body.code,
                    ErrorCode::NotFound | ErrorCode::Conflict | ErrorCode::Forbidden
                ) =>
            {
                let line =
                    format!("the answer was not taken: {}", crate::format::one_line(&body.message));
                let step = view.note(&line, self.ctx.screen.size());
                self.show(out, view, &step)
            }
            Err(error) => Err(error.into()),
        }
    }
}

/// What the running `reader`, which read for `before` (`None` for a new reader), reads
/// for `ask`, with the text that a shown answer line starts with.
///
/// The pending line of a call that kept its keys becomes the answer line of a visible
/// wait of the same call, with the keys still queued fed to it first (Enter dropped),
/// because they were typed before the question appeared. A visible wait shows that text;
/// one that looks secret does not, and says how many characters it starts with. Keeping
/// keys again keeps the queue too. Anything
/// else drops what came before, zeroed, and the queue: those keys were typed before
/// this question appeared, and a new reader's flush would have dropped them.
fn take_over(
    mut reader: KeyReader,
    before: Option<Asking>,
    ask: Ask,
) -> (KeyReader, Asking, Option<Seed>) {
    match (before, ask) {
        (Some(Asking::Pending { call_id, mut line }), Ask::Input { call_id: asked, kind })
            if asked == call_id && matches!(kind, AnswerKind::Visible | AnswerKind::Masked) =>
        {
            while let Some(key) = reader.queued() {
                pend(&mut line, key.byte());
            }
            let seeded = match line.text() {
                "" => None,
                text if kind.shown() => Some(Seed::Shown(text.to_owned())),
                text => Some(Seed::Unshown(text.chars().count())),
            };
            (reader, Asking::Input { call_id, kind, line }, seeded)
        }
        (Some(Asking::Pending { call_id, line }), Ask::Retain(kept)) if kept == call_id => {
            (reader, Asking::Pending { call_id, line }, None)
        }
        (before, ask) => {
            // A running reader is kept, so echo never comes back between two questions.
            if before.is_some() && !matches!(ask, Ask::Retain(_)) {
                reader.discard_queued();
            }
            let asking = match ask {
                Ask::Approval(call_id) => Asking::Approval(call_id),
                Ask::Input { call_id, kind } => {
                    Asking::Input { call_id, kind, line: AnswerLine::new() }
                }
                Ask::Discard(_) => Asking::Discard,
                Ask::Retain(call_id) => Asking::Pending { call_id, line: AnswerLine::new() },
                Ask::Surface(question_id) => Asking::Surface(question_id),
            };
            (reader, asking, None)
        }
    }
}

/// The call that has been silent for [`SILENCE`] when its time is up; never resolves
/// while no call is silent.
async fn silent(silence: &mut Option<((CallId, u64), Sleep)>) -> CallId {
    match silence {
        Some(((call_id, _), sleep)) => {
            sleep.as_mut().await;
            *call_id
        }
        None => std::future::pending().await,
    }
}

/// The end of `sleep`; never resolves without one.
async fn due(sleep: &mut Option<Sleep>) {
    match sleep {
        Some(sleep) => sleep.as_mut().await,
        None => std::future::pending().await,
    }
}

/// `Ctrl+\` while it is offered; never resolves otherwise.
async fn pressed(quit: &mut Option<Stop>) {
    match quit {
        Some(wait) => wait.as_mut().await,
        None => std::future::pending().await,
    }
}

/// The next key while a question, an input or the input row reads keys; never resolves
/// otherwise.
async fn next_key(keys: &mut Option<(KeyReader, Asking)>) -> Option<Key> {
    match keys {
        Some((reader, _)) => reader.next().await,
        None => std::future::pending().await,
    }
}

/// The daemon's error body of `error`; any other failure, such as a broken connection,
/// is the command's error.
fn server_error(error: ClientError) -> Result<efr_protocol::ErrorBody, CliError> {
    match error {
        ClientError::Server { body } => Ok(body),
        error => Err(error.into()),
    }
}

fn write(out: &mut Output, step: &Step) -> Result<(), CliError> {
    out.err(&step.err);
    out.out(&step.out)
}

/// How a prompt that `prompt.withdraw` was asked to take back fared.
#[derive(Debug)]
enum TakeBack {
    /// The daemon took it back.
    Taken(WithdrawnPrompt),
    /// It is no longer in the queue: it started, ended or was taken back already. The
    /// note says so.
    Gone(String),
    /// The daemon refused for another reason, which the note gives.
    Refused(String),
}

#[cfg(test)]
mod tests;

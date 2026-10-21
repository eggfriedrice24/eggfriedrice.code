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
//! that a call allowed here keeps. The keys typed before a question appeared stay the
//! row's ([`KeyReader::mark`]), and so does the rest of a paste that the question cut. Enter steers the followed turn (`turn.steer`, which
//! the daemon queues as a prompt when it comes too late), Tab queues a prompt
//! (`prompt.send`), Esc interrupts the turn and takes back what this view sent and the
//! turn did not read (`turn.interrupt`), and Alt+Up takes back the newest prompt that
//! this view queued (`prompt.withdraw`). Ctrl+C clears the row; with an empty row it
//! interrupts as before and takes back what this view sent and the turn did not read,
//! but only what the daemon names. The view follows each prompt that it queued after
//! the turn before it, and the command ends when the last one ends. Text that is still
//! in the row then goes back to the user's shell (`crate::draft`). When the connection
//! ends, the view with the row connects again and subscribes after the last event that
//! it showed, so the end events of a restarted daemon still reach it, and a request of
//! the row whose answer was lost goes again with the same command id.
//!
//! A command that a shell call left running in the hidden shell asks with its own
//! answer line, which takes the keys whenever nothing else asks
//! (`Follower::offer_left`). Ctrl+C at that line sends `shell.interrupt` for the command
//! and leaves the turn running. When the last turn ends while the command waits for an
//! answer here, the loop goes on until the wait ends, the command ends, or Esc.
//!
//! Ctrl+X in the row asks `jobs.list` for the newest running job of the conversation,
//! and after a `y` to the view's question it sends `jobs.stop`. efrd answers when the
//! job ended, so the loop waits for the answer beside the events.
//!
//! Ctrl+B in the row, whatever the row holds, sends `call.background` for the running
//! shell call of the followed turn. efrd moves a call in the sandbox at once: it keeps
//! its sandbox, so the key press is the user's answer. For a call outside the sandbox
//! efrd answers a preview, and only a `y` to the view's one-key question sends
//! `call.background` again with the preview's token. efrd answers once the job is
//! recorded, within a few seconds, so the loop waits for that answer at most
//! [`MOVE_WAIT`]. With no such call, a note says so. The key also acts while efr holds
//! the keys for the call, and after the question they go back to the call.

mod view;

use std::pin::Pin;
use std::time::Duration;

use efr_protocol::{
    ApprovalDecision, ApprovalRespond, ApprovalRespondResult, CallBackground, CallBackgroundResult,
    CallId, ConversationHistory, ConversationHistoryResult, ConversationId, ConversationSubscribe,
    ConversationSubscribeItem, ErrorCode, Event, InputRespond, InputRespondResult, JobId, JobState,
    JobsList, JobsListResult, JobsStop, JobsStopResult, LateSteer, Method, Origin, PromptSend,
    PromptSendResult, PromptWithdraw, PromptWithdrawResult, QuestionId, RecentCommand,
    SandboxSurfaceRespond, SandboxSurfaceRespondResult, SecretText, Seq, ShellContext,
    ShellInterrupt, ShellInterruptResult, TurnId, TurnInterrupt, TurnInterruptResult, TurnSettings,
    TurnSteer, TurnSteerResult, WithdrawTarget, WithdrawnPrompt,
};
use efr_stdx::time::{Clock as _, Sleep};
use efr_transport::client::{Client, ClientError, ItemStream};
use futures::StreamExt as _;
use jiff::Timestamp;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::answer::{AnswerLine, Edit};
use crate::cli::LastCommand;
use crate::context::{Context, Stop};
use crate::error::CliError;
use crate::keys::{self, Key, KeyReader};
use crate::output::Output;
use crate::row::Action;
use view::{AnswerKind, Preview};

pub(crate) use view::{Ask, Look, Step, TICK, TurnEnd, TurnView, spinner};

/// What the input row sends with: the shell context, the last command line, the recent
/// command lines and the turn settings that the plugin handed to this command, so a
/// prompt or a late steer from the row goes as a prompt from this terminal would.
#[derive(Debug, Clone)]
pub(crate) struct Compose {
    pub(crate) context: ShellContext,
    pub(crate) last_command: Option<LastCommand>,
    pub(crate) recent_commands: Vec<RecentCommand>,
    pub(crate) settings: TurnSettings,
}

/// The input row of a followed turn: what it sends with, its key reader, which
/// started before the prompt went out, so keys typed meanwhile land in the row, and the
/// origin that the command connected as, to connect again when the connection ends.
#[derive(Debug)]
pub(crate) struct Row {
    pub(crate) compose: Compose,
    pub(crate) reader: KeyReader,
    pub(crate) origin: Origin,
}

/// How often in a row a subscription may fall behind before the command gives up.
const MAX_RESUBSCRIBES: u32 = 8;

/// The events read to find the approvals that a queued prompt waits behind.
const BLOCKING_PAGE: u32 = 500;

/// How long a paste into the input row that a question cut may still go on into the
/// row: a terminal sends a whole paste at once, so its end comes well within this.
const PASTE_GRACE: Duration = Duration::from_secs(1);

/// How often the view with the input row tries to connect again after its connection
/// ended, such as while efrd restarts: with [`RECONNECT_PAUSE`], about 15 s.
const RECONNECT_TRIES: u32 = 60;

/// The pause between two tries to connect again.
const RECONNECT_PAUSE: Duration = Duration::from_millis(250);

/// How often in a row the view may subscribe again after the daemon ended its
/// subscription or the connection, before the command gives up.
const MAX_RECONNECTS: u32 = 3;

/// How long Ctrl+C waits for the daemon to take the interrupt before the command ends
/// anyway.
const INTERRUPT_TIMEOUT: Duration = Duration::from_secs(3);

/// How long the command waits at its end for the stop of a job that the user asked
/// for: efrd answers when the job ended, at most about 6 s after the stop.
const STOP_WAIT: Duration = Duration::from_secs(10);

/// Ctrl+B, the background key, which also reaches the follow loop while efr holds the
/// keys for the running call.
const CTRL_B: u8 = 0x02;

/// How long the background key waits for efrd's answer: efrd answers a move once the
/// job is recorded, and gives up itself after 10 s.
const MOVE_WAIT: Duration = Duration::from_secs(15);

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
    let (compose, keys, origin) = match row {
        Some(Row { compose, reader, origin }) if view.has_row() => {
            (Some(compose), Some((reader, Asking::Row)), Some(origin))
        }
        Some(Row { reader, .. }) => {
            reader.stop().await;
            (None, None, None)
        }
        None => (None, None, None),
    };
    let mut follower = Follower {
        ctx,
        first: client,
        again: None,
        origin,
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
        paste_until: None,
        stopping: None,
        lingering: None,
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
    if let Some((keys, asking)) = follower.keys.take() {
        stop_into(keys, &asking, view).await;
    }
    // NOTE: a dropped request is cancelled on the daemon, so a stop that the user asked
    // for is waited for before the command ends, also after Ctrl+C or the turn's end.
    if let Some((job, answer)) = follower.stopping.take() {
        let note = match ctx.clock.timeout(STOP_WAIT, answer).await {
            Ok(Ok(_)) => None,
            Ok(Err(ClientError::Server { body })) => Some(format!(
                "job {job} was not stopped: {}",
                crate::format::one_line(&body.message)
            )),
            Ok(Err(error)) => Some(format!(
                "job {job} was not stopped: {}",
                crate::format::one_line(&error.to_string())
            )),
            Err(_) => {
                Some(format!("efrd did not confirm the stop of job {job}; efr jobs shows it"))
            }
        };
        if let Some(note) = note {
            let step = view.note(&note, ctx.screen.size());
            follower.show(out, view, &step)?;
        }
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
            let (note, unsure) =
                match ctx.clock.timeout(INTERRUPT_TIMEOUT, follower.ctrl_c(view, stopped)).await {
                    Ok(Ok(note)) => (note, false),
                    Ok(Err(error)) => (
                        format!(
                            "the interrupt failed: {}",
                            crate::format::one_line(&error.to_string())
                        ),
                        true,
                    ),
                    Err(_) => (
                        "the daemon did not confirm the interrupt; the turn may still run"
                            .to_owned(),
                        true,
                    ),
                };
            let step = view.note(&note, ctx.screen.size());
            follower.show(out, view, &step)?;
            // NOTE: without an answer, nothing says what the daemon took back; it may
            // have taken back steers that no model call reads now. So what this view
            // sent and did not see run comes back, and no text is lost.
            if unsure && view.give_back_pending() {
                let step = view.note(UNCONFIRMED, ctx.screen.size());
                follower.show(out, view, &step)?;
            }
        }
        // NOTE: a daemon that ended the subscription for good also leaves unknown what
        // runs.
        let gone = matches!(error, CliError::Client(error) if lost(error))
            || matches!(error, CliError::SubscriptionEnded);
        if gone && view.give_back_pending() {
            let step = view.note(GIVEN_BACK, ctx.screen.size());
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

/// What the unread steers of this view become when it interrupts its turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Unread {
    /// Esc: they go again as one prompt that runs next.
    Resend,
    /// Ctrl+C: they come back to the user, and no model call reads them.
    TakeBack,
}

/// What a take-back of the followed prompt, which had not started, and of the prompts
/// that this view queued after it did.
#[derive(Debug, Default)]
struct TakenBack {
    /// The text of the followed prompt, when the daemon took it back.
    followed: Option<String>,
    /// The prompts of this view that the daemon took back, in queue order.
    withdrawn: Vec<WithdrawnPrompt>,
    /// The followed prompt started before its take-back: the interrupt that stopped it.
    interrupted: Option<TurnInterruptResult>,
    /// What did not work, one note each.
    notes: Vec<String>,
}

struct Follower<'a> {
    ctx: &'a Context,
    /// The connection that the command made.
    first: &'a Client,
    /// The connection that the view made after the first one ended.
    again: Option<Client>,
    /// The origin to connect again as; `None` without the input row, which does not
    /// connect again.
    origin: Option<Origin>,
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
    /// A question took the keys while a paste into the row went on: until this time,
    /// the rest of the paste still goes into the row.
    paste_until: Option<Timestamp>,
    /// The stop of a job that the stop key asked for, while efrd ends the job: the loop
    /// goes on meanwhile, and the job's end shows when its `job_ended` comes.
    stopping: Option<(JobId, Stopping<'a>)>,
    /// The last followed turn ended so, but `efr` stays open while a command that a
    /// call left running in the hidden shell waits for an answer here
    /// ([`TurnView::lingers`]).
    lingering: Option<TurnEnd>,
}

/// The answer to a `jobs.stop`, which comes when the job ended.
type Stopping<'a> = Pin<Box<dyn Future<Output = Result<JobsStopResult, ClientError>> + 'a>>;

/// What the keys being read answer.
#[derive(Debug)]
enum Asking {
    /// An approval, with one key.
    Approval(CallId),
    /// The confirm of an `a` answer to the approval of a call (phase E): `y` saves the
    /// rule that its question offers and runs the call, any other key asks the question
    /// again. The keys typed before the confirm showed never answer it.
    Always(CallId),
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
    /// The question of the stop key, with one key.
    StopJob,
    /// The question of the background key about call `call_id`, with one key: `y` moves
    /// it with `token`.
    Move { call_id: CallId, token: u64 },
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

/// Stops `keys` like [`stop`]. The keys that the input row did not take yet, also those
/// that the reader reads until it stops, are part of the row's text.
async fn stop_into(keys: KeyReader, asking: &Asking, view: &mut TurnView) {
    if matches!(asking, Asking::Row) {
        for key in keys.stop_keeping().await {
            view.row_key(key);
        }
    } else {
        stop(keys, asking).await;
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
        Asking::Approval(_)
        | Asking::Always(_)
        | Asking::Surface(_)
        | Asking::StopJob
        | Asking::Move { .. }
        | Asking::Row => {
            keys.stop().await;
        }
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
        let page = match self.client().call::<ConversationHistoryResult>(method).await {
            Ok(page) => page,
            Err(error) => {
                tracing::debug!(error = %error, "the approvals ahead of the prompt could not be read");
                return Ok(());
            }
        };
        let mut pending: Vec<Event> = Vec::new();
        // The notice that started the turn ahead, when a job notice started it.
        let mut wake: Option<Event> = None;
        // The running call's last output and input wait; tool calls run one at a time.
        let mut call: Option<CallId> = None;
        let mut output: Option<Event> = None;
        let mut input: Option<Event> = None;
        for envelope in page.events.into_iter().filter(|envelope| envelope.seq <= self.target.after)
        {
            match &envelope.event {
                Event::JobNotice { turn_id, woke: true, .. } if *turn_id != self.target.turn => {
                    wake = Some(envelope.event);
                }
                Event::TurnCompleted { turn_id, .. }
                | Event::TurnFailed { turn_id, .. }
                | Event::TurnInterrupted { turn_id, .. }
                | Event::TurnCancelled { turn_id }
                    if wake.as_ref().and_then(Event::turn_id) == Some(*turn_id) =>
                {
                    wake = None;
                }
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
        for event in wake.into_iter().chain(pending).chain(output).chain(input) {
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
        let mut reconnects = 0;
        let mut ends = 0;
        loop {
            let again = match self.subscribe().await {
                Ok(mut stream) => loop {
                    // NOTE: the turn ended, and the command in the hidden shell no longer
                    // waits here, or the user left it with Esc.
                    if let Some(end) = self.lingering.take_if(|_| !view.lingers()) {
                        return self.finished(end, view);
                    }
                    self.watch_silence(view);
                    self.watch_tick(view);
                    tokio::select! {
                        () = &mut interrupt => {
                            // Ctrl+C at the line of a command left running in the hidden
                            // shell stops that command, not the turn.
                            if let Some(call_id) = view.left_keys().filter(|_| {
                                !matches!(self.keys, Some((_, Asking::Row)) | None)
                            }) {
                                interrupt = self.ctx.interrupt.wait();
                                self.interrupt_left(call_id, out, view).await?;
                            // Ctrl+C clears the text of the input row first.
                            } else if matches!(self.keys, Some((_, Asking::Row))) && view.row_clear() {
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
                        answer = stop_answer(&mut self.stopping) => {
                            if let Some((job, _)) = self.stopping.take() {
                                self.stopped(job, answer, out, view)?;
                            }
                        }
                        item = stream.next() => match item {
                            Some(Ok(value)) => {
                                resubscribes = 0;
                                reconnects = 0;
                                ends = 0;
                                if let Some(end) = self.item(value, out, view).await? {
                                    if !view.lingers() {
                                        return self.finished(end, view);
                                    }
                                    self.lingering = Some(end);
                                }
                            }
                            Some(Err(ClientError::Server { body })) if body.code == ErrorCode::Overflow => {
                                break Again::Behind;
                            }
                            Some(Err(ClientError::StreamOverflow { .. })) => break Again::Behind,
                            Some(Err(error)) if lost(&error) => break Again::Lost(error),
                            Some(Err(error)) => return Err(error.into()),
                            None => break Again::Ended,
                        },
                    }
                },
                Err(CliError::Client(error)) if lost(&error) => Again::Lost(error),
                Err(error) => return Err(error),
            };
            let error = match again {
                Again::Behind => {
                    resubscribes += 1;
                    if resubscribes > MAX_RESUBSCRIBES {
                        return Err(CliError::FellBehind { times: resubscribes });
                    }
                    tracing::debug!(after = %self.last_seen, "the subscription fell behind; resuming");
                    continue;
                }
                // NOTE: without the input row, an end stays the command's error, as it
                // was before the row. A daemon that stops ends its streams and then
                // closes the connection: the view subscribes again, and that request
                // fails once the connection is closed. The end and that loss are one
                // restart, so each counts against a limit of its own.
                Again::Ended => {
                    ends += 1;
                    if self.origin.is_none() || ends > MAX_RECONNECTS {
                        return Err(CliError::SubscriptionEnded);
                    }
                    continue;
                }
                Again::Lost(error) => error,
            };
            reconnects += 1;
            if self.origin.is_none() || reconnects > MAX_RECONNECTS {
                return Err(error.into());
            }
            tracing::debug!(error = %error, "the connection to the daemon ended; connecting again");
            let step = view.note(RECONNECTING, self.ctx.screen.size());
            self.show(out, view, &step)?;
            let connected = tokio::select! {
                () = &mut interrupt => return Err(CliError::Interrupted),
                signal = &mut ending => return Err(CliError::Ended { signal }),
                connected = self.reconnect() => connected,
            };
            if !connected {
                return Err(error.into());
            }
        }
    }

    /// The connection that requests go on: the one that the view made again, once it
    /// had to, else the command's.
    fn client(&self) -> &Client {
        self.again.as_ref().unwrap_or(self.first)
    }

    /// Connects to the daemon again, as the command connected, up to
    /// [`RECONNECT_TRIES`] times: efrd may be restarting. True when the view has a
    /// connection that is open; nothing is done when it has one already.
    async fn reconnect(&mut self) -> bool {
        let Some(origin) = self.origin else {
            return false;
        };
        if !self.client().is_closed() {
            return true;
        }
        let tty = self.compose.as_ref().and_then(|compose| compose.context.tty.clone());
        for _ in 0..RECONNECT_TRIES {
            match self.ctx.connect(origin, tty.as_deref()).await {
                Ok(client) => {
                    self.again = Some(client);
                    return true;
                }
                // NOTE: a daemon of another protocol answers the same way to every try.
                Err(error @ CliError::Client(ClientError::ProtocolMismatch { .. })) => {
                    tracing::debug!(error = %error, "connecting again cannot work");
                    return false;
                }
                Err(error) => {
                    tracing::debug!(error = %error, "connecting again failed");
                    self.ctx.clock.sleep(RECONNECT_PAUSE).await;
                }
            }
        }
        false
    }

    /// Sends a request of the input row. When the connection ended, the view connects
    /// again and sends the same request, with the same command id, so the daemon
    /// answers from its receipt when it took the first one: the text never goes twice.
    /// The daemon's refusal is the inner error; a connection that does not come back,
    /// or Ctrl+C, SIGTERM or SIGHUP meanwhile, is the outer one.
    async fn send<R: DeserializeOwned>(
        &mut self,
        method: Method,
    ) -> Result<Result<R, efr_protocol::ErrorBody>, CliError> {
        let first = self.client().call::<R>(method.clone()).await;
        let answer = match first {
            Err(error) if lost(&error) && self.origin.is_some() => {
                tracing::debug!(error = %error, "a request of the input row lost its connection");
                let mut interrupted = self.ctx.interrupt.wait();
                let mut ending = self.ctx.terminate.wait();
                let connected = tokio::select! {
                    () = &mut interrupted => return Err(CliError::Interrupted),
                    signal = &mut ending => return Err(CliError::Ended { signal }),
                    connected = self.reconnect() => connected,
                };
                if !connected {
                    return Err(error.into());
                }
                self.client().call::<R>(method).await
            }
            answer => answer,
        };
        match answer {
            Ok(result) => Ok(Ok(result)),
            Err(error) => server_error(error).map(Err),
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
        Ok(self.client().stream(method).await?)
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
            Some(ask) => self.take_keys(ask, out, view)?,
            None if step.end.is_some() => {
                if let Some((keys, asking)) = self.keys.take() {
                    stop_into(keys, &asking, view).await;
                }
            }
            None if step.settled => match self.keys.take() {
                // The row asked for nothing, so nothing of it is settled.
                Some((reader, Asking::Row)) => self.keys = Some((reader, Asking::Row)),
                // NOTE: what settled is not the line of a command left running in the
                // hidden shell, which still asks: its keys stay.
                Some((reader, asking)) if left_owns(view, &asking) => {
                    self.keys = Some((reader, asking));
                }
                Some((reader, asking)) => self.release(reader, &asking, view).await,
                None => {}
            },
            None => {}
        }
        if step.end.is_none() {
            self.offer_left(out, view)?;
        }
        if urgent {
            self.paint(out, view, false)?;
        } else {
            self.schedule(out, view)?;
        }
        Ok(step.end)
    }

    /// The keys go to `ask`: from the input row, from what read them before, or from a
    /// reader that starts now. Another question closes the line of a command left
    /// running in the hidden shell, which comes back after it.
    fn take_keys(
        &mut self,
        ask: Ask,
        out: &mut Output,
        view: &mut TurnView,
    ) -> Result<(), CliError> {
        if !view.left_asks(&ask) {
            view.left_yield();
        }
        let (reader, before) = match self.keys.take() {
            Some((mut reader, Asking::Row)) => {
                // NOTE: the keys typed before the question are the row's, also
                // those still in the terminal (the mark) and the rest of a
                // paste; a key that would send stays in the row instead. So are
                // the keys typed before a password prompt showed: the user had
                // not seen it yet.
                while let Some(key) = reader.queued() {
                    view.row_key(key);
                }
                reader.mark();
                self.paste_until = view.row_pasting().then(|| self.ctx.clock.now() + PASTE_GRACE);
                (reader, Some(Asking::Row))
            }
            Some((reader, before)) => (reader, Some(before)),
            // Starting the reader discards typeahead, so nothing typed before
            // the question answers it or stays queued for the shell.
            None => (self.ctx.keys.start()?, None),
        };
        // NOTE: the reader is held here before anything can fail, so the exit
        // path still stops it and restores the terminal.
        let (reader, asking, seeded, row) = take_over(reader, before, ask);
        self.keys = Some((reader, asking));
        for key in row {
            view.row_key(key);
        }
        let size = self.ctx.screen.size();
        match seeded {
            Some(Seed::Shown(text)) => write(out, &view.typed(&text))?,
            Some(Seed::Unshown(count)) => write(out, &view.typed_ahead(count, size))?,
            None => {}
        }
        Ok(())
    }

    /// The line of a command left running in the hidden shell takes the keys when it
    /// needs them and nothing else reads them but the input row.
    fn offer_left(&mut self, out: &mut Output, view: &mut TurnView) -> Result<(), CliError> {
        if !matches!(self.keys, Some((_, Asking::Row)) | None) || !self.ctx.keys.available() {
            return Ok(());
        }
        let Some(step) = view.left_offer() else {
            return Ok(());
        };
        write(out, &step)?;
        match step.ask {
            Some(ask) => self.take_keys(ask, out, view),
            None => Ok(()),
        }
    }

    /// Ctrl+C at the line of the command that call `call_id` left running in the
    /// hidden shell: efrd sends it SIGINT. Its end comes as an event; a refusal is a
    /// note.
    async fn interrupt_left(
        &mut self,
        call_id: CallId,
        out: &mut Output,
        view: &mut TurnView,
    ) -> Result<(), CliError> {
        let method = Method::ShellInterrupt(ShellInterrupt {
            conversation_id: self.target.conversation,
            call_id,
            command_id: self.ctx.command_id(),
            close: false,
        });
        match self.send::<ShellInterruptResult>(method).await? {
            Ok(_) => Ok(()),
            Err(body) => {
                let step = view.left_not_stopped(&body.message, self.ctx.screen.size());
                self.apply(step, out, view, false).await.map(drop)
            }
        }
    }

    /// Handles one key; `None` means the key source ended. Afterwards the line of a
    /// command left running in the hidden shell takes the keys when nothing else does.
    async fn key(
        &mut self,
        key: Option<Key>,
        out: &mut Output,
        view: &mut TurnView,
    ) -> Result<(), CliError> {
        self.one_key(key, out, view).await?;
        self.offer_left(out, view)?;
        self.schedule(out, view)
    }

    /// Handles one key.
    async fn one_key(
        &mut self,
        key: Option<Key>,
        out: &mut Output,
        view: &mut TurnView,
    ) -> Result<(), CliError> {
        let Some((mut reader, asking)) = self.keys.take() else {
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
        // NOTE: a key typed after the question but before its confirm showed was typed
        // for the question, so it answers nothing and goes nowhere.
        if matches!(asking, Asking::Always(_)) && reader.before_mark() {
            self.keys = Some((reader, asking));
            return Ok(());
        }
        // NOTE: a key typed before the question appeared, or the rest of a paste that
        // it cut, is the row's: it never answers the question, and it never sends.
        if reader.before_mark() || self.pasting(view) {
            view.row_key(key);
            self.keys = Some((reader, asking));
            return self.schedule(out, view);
        }
        // NOTE: Esc at the line of a command that still waits in the hidden shell after
        // the turn leaves it there, and `efr` ends.
        if let (Key::Esc, Asking::Input { call_id, .. }) = (key, &asking)
            && let Some(step) = view.left_leave(*call_id, self.ctx.screen.size())
        {
            stop(reader, &asking).await;
            return self.show(out, view, &step);
        }
        // A line or a question reads bytes; Esc alone is the escape byte there.
        let key = key.byte();
        match asking {
            Asking::Row => Ok(()),
            // NOTE: Ctrl+B moves the call also while efr holds the keys for it: it is no
            // character of a password, and the keys come back to the call after the
            // move's question ([`TurnView::move_answered`]).
            Asking::Discard => {
                self.keys = Some((reader, Asking::Discard));
                if key == CTRL_B { Box::pin(self.background_key(out, view)).await } else { Ok(()) }
            }
            Asking::Pending { call_id, line } if key == CTRL_B => {
                self.keys = Some((reader, Asking::Pending { call_id, line }));
                Box::pin(self.background_key(out, view)).await
            }
            Asking::Pending { call_id, mut line } => {
                pend(&mut line, key);
                self.keys = Some((reader, Asking::Pending { call_id, line }));
                Ok(())
            }
            Asking::Approval(call_id) => {
                let offered = view.offers_always(call_id);
                let Some(decision) = keys::decision_with_always(key, offered) else {
                    self.keys = Some((reader, Asking::Approval(call_id)));
                    return Ok(());
                };
                if decision == ApprovalDecision::AllowAlways {
                    // NOTE: `a` saves a standing rule, so it asks first. The keys typed
                    // before the confirm showed come before the mark, and go.
                    reader.discard_queued();
                    reader.mark();
                    self.keys = Some((reader, Asking::Always(call_id)));
                    let step = view.confirm_always(call_id, self.ctx.screen.size());
                    return self.show(out, view, &step);
                }
                self.settle_answer(reader, call_id, decision, out, view).await?;
                let answer = self.send_answer(call_id, decision).await;
                self.answer_sent(answer, out, view)
            }
            Asking::Always(call_id) => {
                if !matches!(key, b'y' | b'Y') {
                    self.keys = Some((reader, Asking::Approval(call_id)));
                    let step = view.always_cancelled(call_id, self.ctx.screen.size());
                    return self.show(out, view, &step);
                }
                // NOTE: the answer goes first: the daemon refuses an `a` from a client
                // that may not save a rule, or for a question that offers none, and the
                // question then stays open, so this client asks it again.
                let decision = ApprovalDecision::AllowAlways;
                match self.send_answer(call_id, decision).await {
                    Err(ClientError::Server { body })
                        if matches!(body.code, ErrorCode::Forbidden | ErrorCode::Invalid) =>
                    {
                        self.keys = Some((reader, Asking::Approval(call_id)));
                        let size = self.ctx.screen.size();
                        let step = view.always_refused(call_id, &body.message, size);
                        self.show(out, view, &step)
                    }
                    answer => {
                        self.settle_answer(reader, call_id, decision, out, view).await?;
                        self.answer_sent(answer, out, view)
                    }
                }
            }
            Asking::StopJob => {
                let stop = match key {
                    b'y' | b'Y' => true,
                    b'n' | b'N' | keys::ESC => false,
                    _ => {
                        self.keys = Some((reader, Asking::StopJob));
                        return Ok(());
                    }
                };
                self.release(reader, &Asking::StopJob, view).await;
                let (step, job) = view.stop_answered(stop, self.ctx.screen.size());
                self.show(out, view, &step)?;
                match job {
                    Some(job) => Box::pin(self.stop_job(job, out, view)).await,
                    None => Ok(()),
                }
            }
            Asking::Move { call_id, token } => {
                let yes = match key {
                    b'y' | b'Y' => true,
                    b'n' | b'N' | keys::ESC => false,
                    _ => {
                        self.keys = Some((reader, Asking::Move { call_id, token }));
                        return Ok(());
                    }
                };
                let (step, call) = view.move_answered(yes, self.ctx.screen.size());
                if step.ask.is_some() {
                    // NOTE: the call held the keys before the question: they go back to it.
                    self.keys = Some((reader, Asking::Move { call_id, token }));
                    self.apply(step, out, view, false).await?;
                } else {
                    self.release(reader, &Asking::Move { call_id, token }, view).await;
                    self.show(out, view, &step)?;
                }
                match call {
                    Some(call_id) => {
                        Box::pin(self.move_call(call_id, Some(token), out, view)).await
                    }
                    None => Ok(()),
                }
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
    async fn release(&mut self, mut reader: KeyReader, before: &Asking, view: &mut TurnView) {
        if self.compose.is_some() && view.has_row() {
            if before.guards() {
                reader.flush();
            }
            view.row_resumed();
            self.paste_until = None;
            self.keys = Some((reader, Asking::Row));
        } else {
            stop(reader, before).await;
        }
    }

    /// True while the rest of a paste into the row that a question cut still goes to
    /// the row. A paste whose end did not come in time ends there.
    fn pasting(&mut self, view: &mut TurnView) -> bool {
        if !view.row_pasting() {
            self.paste_until = None;
            return false;
        }
        match self.paste_until {
            Some(until) if self.ctx.clock.now() < until => true,
            _ => {
                view.row_end_paste();
                self.paste_until = None;
                false
            }
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
            Action::StopJob => Box::pin(self.stop_key(out, view)).await,
            Action::Background => Box::pin(self.background_key(out, view)).await,
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
                recent_commands: compose.recent_commands,
                settings: compose.settings,
            }),
        });
        let answer = self.send::<TurnSteerResult>(method).await.inspect_err(|_| {
            // NOTE: efrd did not come back, so nobody knows whether it took the steer;
            // the text goes back to the user with the row rather than get lost.
            view.row_append(&text);
        })?;
        let note = match answer {
            Ok(result) if result.queued => {
                view.queued_prompt(result.turn_id, text, true);
                None
            }
            Ok(result) => {
                view.steered(result.seq, text);
                None
            }
            Err(body) => {
                view.row_append(&text);
                Some(if body.code == ErrorCode::Conflict {
                    "not sent: the turn takes no more steers; press Tab to queue the text"
                        .to_owned()
                } else {
                    format!("not sent: {}", crate::format::one_line(&body.message))
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
            recent_commands: compose.recent_commands,
            settings: compose.settings,
        });
        let answer = self.send::<PromptSendResult>(method).await.inspect_err(|_| {
            view.row_append(&text);
        })?;
        let note = match answer {
            Ok(result) => {
                view.queued_prompt(result.turn_id, text, false);
                None
            }
            Err(body) => {
                view.row_append(&text);
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
        let method = self.interrupt_method(view, Unread::Resend, view.queued_turns());
        let size = self.ctx.screen.size();
        let step = match self.client().call::<TurnInterruptResult>(method).await {
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

    /// `turn.interrupt` for the followed turn, with what happens to the unread steers
    /// of this view, and the queued prompts of `withdraw` to take back. A resent prompt
    /// goes with this terminal's values, as a late steer does.
    fn interrupt_method(&self, view: &TurnView, unread: Unread, withdraw: Vec<TurnId>) -> Method {
        let steers = view.unread_steers();
        let (resend_steers, withdraw_steers) = match unread {
            Unread::Resend => (steers, Vec::new()),
            Unread::TakeBack => (Vec::new(), steers),
        };
        let resend_as = self.compose.clone().filter(|_| !resend_steers.is_empty()).map(|compose| {
            Box::new(LateSteer::Queue {
                context: Some(compose.context),
                last_command: compose.last_command.map(LastCommand::into_string),
                recent_commands: compose.recent_commands,
                settings: compose.settings,
            })
        });
        Method::TurnInterrupt(TurnInterrupt {
            command_id: self.ctx.command_id(),
            conversation_id: self.target.conversation,
            turn_id: Some(view.turn()),
            resend_steers,
            resend_as,
            withdraw_steers,
            withdraw,
        })
    }

    /// Esc while the followed prompt waits behind another turn: takes it back, and the
    /// prompts that this view queued after it, into the row. Its `prompt_withdrawn`
    /// then ends it, and with it the command. When it started meanwhile, Esc
    /// interrupts it instead.
    async fn take_back_all(
        &mut self,
        out: &mut Output,
        view: &mut TurnView,
    ) -> Result<(), CliError> {
        let followed = view.turn();
        let taken = self.take_back_queued(view, Unread::Resend).await?;
        let size = self.ctx.screen.size();
        if let Some(text) = &taken.followed {
            self.escaped = Some(followed);
            view.row_append(text);
        }
        for prompt in &taken.withdrawn {
            view.withdrawn(prompt.turn_id, &prompt.text);
        }
        let mut step = Step::default();
        if let Some(result) = &taken.interrupted {
            self.escaped = Some(followed);
            step = view.interrupt_result(result, size);
        }
        // One note says it for all.
        if let Some(note) = taken.notes.first() {
            let noted = view.note(note, size);
            step.out.push_str(&noted.out);
            step.err.push_str(&noted.err);
        }
        self.apply(step, out, view, false).await.map(drop)
    }

    /// Takes back the followed prompt, which had not started, and the prompts that this
    /// view queued after it. The newest goes first and the followed prompt last: a
    /// prompt cannot start while one before it waits, so none of them starts in
    /// between. When the followed prompt started meanwhile, the interrupt stops it and
    /// does with the unread steers of this view as `unread` says.
    async fn take_back_queued(
        &self,
        view: &TurnView,
        unread: Unread,
    ) -> Result<TakenBack, CliError> {
        let mut taken = TakenBack::default();
        let mut later = Vec::new();
        let mut left = Vec::new();
        for turn in view.queued_turns().into_iter().rev() {
            match self.take_back(turn).await? {
                TakeBack::Taken(prompt) => later.push(prompt),
                TakeBack::Started(note) | TakeBack::Unknown(note) | TakeBack::Refused(note) => {
                    left.push(turn);
                    taken.notes.push(note);
                }
            }
        }
        later.reverse();
        left.reverse();
        taken.withdrawn = later;
        match self.take_back(view.turn()).await? {
            TakeBack::Taken(prompt) => taken.followed = Some(prompt.text),
            TakeBack::Started(note) => {
                let method = self.interrupt_method(view, unread, left);
                match self.client().call::<TurnInterruptResult>(method).await {
                    Ok(result) => taken.interrupted = Some(result),
                    Err(error) => {
                        let body = server_error(error)?;
                        tracing::debug!(message = %body.message, "the started prompt was not interrupted");
                        taken.notes.push(note);
                    }
                }
            }
            TakeBack::Unknown(note) | TakeBack::Refused(note) => taken.notes.push(note),
        }
        Ok(taken)
    }

    /// Ctrl+C with an empty row, after the view closed: interrupts the followed turn
    /// and takes back what this view sent and the turn did not read, so it goes back to
    /// the user's shell. Only what the daemon names comes back: a steer that it does
    /// not name was read, or stays part of the turn. A followed prompt that did not
    /// start yet is taken back, with the prompts behind it. The note says what
    /// happened; `stopped` is the time of the Ctrl+C. An error, such as a lost
    /// connection, leaves unknown what the daemon did, and the view as it was: a
    /// result changes the view only when every answer came.
    async fn ctrl_c(&self, view: &mut TurnView, stopped: Timestamp) -> Result<String, CliError> {
        if view.is_queued() && self.compose.is_some() {
            return match self.take_back_queued(view, Unread::TakeBack).await {
                Ok(taken) => {
                    if let Some(text) = &taken.followed {
                        view.row_append(text);
                    }
                    for prompt in &taken.withdrawn {
                        view.withdrawn(prompt.turn_id, &prompt.text);
                    }
                    if let Some(result) = &taken.interrupted {
                        view.taken_back(result);
                    }
                    Ok(match (taken.notes.first(), &taken.interrupted) {
                        (Some(note), _) => note.clone(),
                        (None, Some(_)) => view.interrupted(stopped),
                        (None, None) => TAKEN_BACK.to_owned(),
                    })
                }
                Err(error) => Err(error),
            };
        }
        let method = self.interrupt_method(view, Unread::TakeBack, view.queued_turns());
        match self.client().call::<TurnInterruptResult>(method).await {
            Ok(result) => {
                view.taken_back(&result);
                Ok(view.interrupted(stopped))
            }
            Err(error) => match server_error(error)? {
                // The turn already ended, or it still waits behind another turn.
                // Without the row, Ctrl+C leaves a queued prompt in the queue, as before
                // the row.
                body if body.code == ErrorCode::Conflict => Ok(
                    "not interrupted: the turn is not running; a queued prompt still runs in its turn"
                        .to_owned(),
                ),
                body => {
                    tracing::debug!(message = %body.message, "turn.interrupt failed");
                    Ok(format!(
                        "the interrupt failed: {}",
                        crate::format::one_line(&body.message)
                    ))
                }
            },
        }
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
            // NOTE: it runs now, or it is about to: the view keeps it in its list and
            // follows it, and its end event takes it out.
            TakeBack::Started(note) | TakeBack::Refused(note) => Some(note),
            TakeBack::Unknown(note) => {
                view.drop_queued(turn);
                Some(note)
            }
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
        match self.client().call::<PromptWithdrawResult>(method).await {
            Ok(result) => Ok(TakeBack::Taken(result.withdrawn)),
            Err(error) => Ok(match server_error(error)? {
                body if body.code == ErrorCode::Conflict => {
                    TakeBack::Started("not taken back: the prompt already started".to_owned())
                }
                body if body.code == ErrorCode::NotFound => {
                    TakeBack::Unknown("not taken back: the daemon has no such prompt".to_owned())
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
    /// `efr_transport::client` zeroes the encoded request frame once it is written. A
    /// manual answer asks once, so its keys stop after it.
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
        let step = match self.client().call::<InputRespondResult>(method).await {
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

    /// Ctrl+X: asks whether to stop the newest running job of the conversation. The
    /// view follows the conversation only from its prompt on, so it may not have seen
    /// the job start: `jobs.list` names the jobs.
    async fn stop_key(&mut self, out: &mut Output, view: &mut TurnView) -> Result<(), CliError> {
        let method = Method::JobsList(JobsList { conversation_id: Some(self.target.conversation) });
        let answer = self.send::<JobsListResult>(method).await?;
        let size = self.ctx.screen.size();
        let step = match answer {
            // The list is newest first.
            Ok(list) => match list.jobs.iter().find(|job| job.state == JobState::Running) {
                Some(job) => view.stop_question(job, size),
                None => view.note(crate::format::jobs::NO_JOB, size),
            },
            Err(body) => view.note(
                &format!(
                    "the jobs could not be listed: {}",
                    crate::format::one_line(&body.message)
                ),
                size,
            ),
        };
        self.apply(step, out, view, false).await.map(drop)
    }

    /// Ctrl+B: moves the running shell call of the followed turn to the background, or
    /// says that no such call runs.
    async fn background_key(
        &mut self,
        out: &mut Output,
        view: &mut TurnView,
    ) -> Result<(), CliError> {
        match view.background_call() {
            Some(call_id) => self.move_call(call_id, None, out, view).await,
            None => {
                let step = view.note(crate::format::jobs::NO_CALL, self.ctx.screen.size());
                self.apply(step, out, view, false).await.map(drop)
            }
        }
    }

    /// Sends `call.background` for `call_id`: with no `confirm` after Ctrl+B, with the
    /// preview's token after a `y` to the question. A preview asks that question; a
    /// refusal is a note. efrd answers once the job is recorded, at most [`MOVE_WAIT`]
    /// later.
    async fn move_call(
        &mut self,
        call_id: CallId,
        confirm: Option<u64>,
        out: &mut Output,
        view: &mut TurnView,
    ) -> Result<(), CliError> {
        let method = Method::CallBackground(CallBackground {
            conversation_id: self.target.conversation,
            call_id,
            command_id: self.ctx.command_id(),
            confirm,
        });
        let ctx = self.ctx;
        let answer = ctx.clock.timeout(MOVE_WAIT, self.send::<CallBackgroundResult>(method)).await;
        let size = self.ctx.screen.size();
        let step = match answer {
            Ok(answer) => match answer? {
                Ok(CallBackgroundResult::Moved { job_id }) => {
                    view.moved(call_id, job_id);
                    Step::default()
                }
                // NOTE: a preview after a `y` means that efrd wants a new answer; the
                // user presses the key again rather than answering twice in a row.
                Ok(CallBackgroundResult::Preview { token, command, max_minutes, exits })
                    if confirm.is_none() =>
                {
                    let preview = Preview { token, command: &command, max_minutes, exits: &exits };
                    view.move_question(call_id, &preview, size)
                }
                Ok(CallBackgroundResult::Preview { .. }) => {
                    view.not_moved(call_id, MOVE_AGAIN, size)
                }
                Ok(_) => view.not_moved(call_id, MOVE_UNKNOWN, size),
                Err(body) => view.not_moved(call_id, &body.message, size),
            },
            Err(_) => view.note(MOVE_UNCONFIRMED, size),
        };
        self.apply(step, out, view, false).await.map(drop)
    }

    /// Asks efrd to stop `job`. The answer comes when the job ended, so the loop waits
    /// for it beside the events; on a connection made again, it waits here.
    async fn stop_job(
        &mut self,
        job: JobId,
        out: &mut Output,
        view: &mut TurnView,
    ) -> Result<(), CliError> {
        let method = Method::JobsStop(JobsStop {
            command_id: self.ctx.command_id(),
            conversation_id: Some(self.target.conversation),
            job_id: job.clone(),
        });
        if self.again.is_none() {
            let first: &Client = self.first;
            self.stopping = Some((job, Box::pin(first.call::<JobsStopResult>(method))));
            return Ok(());
        }
        let answer = self.client().call::<JobsStopResult>(method).await;
        self.stopped(job, answer, out, view)
    }

    /// The answer to the stop of `job`: nothing to show when efrd stopped it, because
    /// its `job_ended` shows; else a note says why not.
    fn stopped(
        &mut self,
        job: JobId,
        answer: Result<JobsStopResult, ClientError>,
        out: &mut Output,
        view: &mut TurnView,
    ) -> Result<(), CliError> {
        let message = match answer {
            Ok(_) => return Ok(()),
            Err(ClientError::Server { body }) => body.message,
            Err(error) => error.to_string(),
        };
        let step = view.stop_failed(&job, &message, self.ctx.screen.size());
        self.show(out, view, &step)
    }

    /// The approval `call_id` was answered here with `decision`: its card gives its place
    /// to the answer line, and the keys go back, or stay for a call that may wait for
    /// input.
    async fn settle_answer(
        &mut self,
        reader: KeyReader,
        call_id: CallId,
        decision: ApprovalDecision,
        out: &mut Output,
        view: &mut TurnView,
    ) -> Result<(), CliError> {
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
        self.show(out, view, &step)
    }

    /// Sends the answer `decision` to the approval `call_id`.
    async fn send_answer(
        &mut self,
        call_id: CallId,
        decision: ApprovalDecision,
    ) -> Result<ApprovalRespondResult, ClientError> {
        let method = Method::ApprovalRespond(ApprovalRespond {
            command_id: self.ctx.command_id(),
            conversation_id: self.target.conversation,
            call_id,
            decision,
        });
        self.client().call::<ApprovalRespondResult>(method).await
    }

    /// What the daemon said to an answer: nothing to show when it took it, a note when
    /// it was answered elsewhere first or expired.
    fn answer_sent(
        &mut self,
        answer: Result<ApprovalRespondResult, ClientError>,
        out: &mut Output,
        view: &mut TurnView,
    ) -> Result<(), CliError> {
        match answer {
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
        match self.client().call::<SandboxSurfaceRespondResult>(method).await {
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
/// this question appeared, and a new reader's flush would have dropped them. In every
/// case, a queued key from before the last mark (`KeyReader::before_mark`) was typed
/// for the input row: it comes back in the last part, for the row, and never starts an
/// answer.
fn take_over(
    mut reader: KeyReader,
    before: Option<Asking>,
    ask: Ask,
) -> (KeyReader, Asking, Option<Seed>, Vec<Key>) {
    let mut row = Vec::new();
    match (before, ask) {
        (Some(Asking::Pending { call_id, mut line }), Ask::Input { call_id: asked, kind })
            if asked == call_id && matches!(kind, AnswerKind::Visible | AnswerKind::Masked) =>
        {
            while let Some(key) = reader.queued() {
                if reader.before_mark() {
                    row.push(key);
                } else {
                    pend(&mut line, key.byte());
                }
            }
            let seeded = match line.text() {
                "" => None,
                text if kind.shown() => Some(Seed::Shown(text.to_owned())),
                text => Some(Seed::Unshown(text.chars().count())),
            };
            (reader, Asking::Input { call_id, kind, line }, seeded, row)
        }
        (Some(Asking::Pending { call_id, line }), Ask::Retain(kept)) if kept == call_id => {
            (reader, Asking::Pending { call_id, line }, None, row)
        }
        // The same wait asks again, such as the line of a command left running in the
        // hidden shell at the end of the turn: what was typed stays.
        (
            Some(Asking::Input { call_id, kind, line }),
            Ask::Input { call_id: asked, kind: again },
        ) if asked == call_id && again == kind => {
            (reader, Asking::Input { call_id, kind, line }, None, row)
        }
        (before, ask) => {
            // A running reader is kept, so echo never comes back between two questions.
            // NOTE: the row's keys before the question are told apart by the mark.
            if before.is_some()
                && !matches!(before, Some(Asking::Row))
                && !matches!(ask, Ask::Retain(_))
            {
                while let Some(key) = reader.queued() {
                    if reader.before_mark() {
                        row.push(key);
                    }
                }
            }
            let asking = match ask {
                Ask::Approval(call_id) => Asking::Approval(call_id),
                Ask::Input { call_id, kind } => {
                    Asking::Input { call_id, kind, line: AnswerLine::new() }
                }
                Ask::Discard(_) => Asking::Discard,
                Ask::Retain(call_id) => Asking::Pending { call_id, line: AnswerLine::new() },
                Ask::Surface(question_id) => Asking::Surface(question_id),
                Ask::StopJob => Asking::StopJob,
                Ask::Move { call_id, token } => Asking::Move { call_id, token },
            };
            (reader, asking, None, row)
        }
    }
}

/// True when `asking` reads the keys for the line of a command left running in the
/// hidden shell, which still asks.
fn left_owns(view: &TurnView, asking: &Asking) -> bool {
    match asking {
        Asking::Input { call_id, .. } => {
            view.left_asks(&Ask::Input { call_id: *call_id, kind: AnswerKind::Visible })
        }
        Asking::Discard => view.left_keys().is_some_and(|call| view.left_asks(&Ask::Discard(call))),
        _ => false,
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

/// The answer to the stop of a job; never resolves while no stop waits.
async fn stop_answer(
    stopping: &mut Option<(JobId, Stopping<'_>)>,
) -> Result<JobsStopResult, ClientError> {
    match stopping {
        Some((_, answer)) => answer.as_mut().await,
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

/// Why the follow loop subscribes again.
#[derive(Debug)]
enum Again {
    /// The subscription fell behind.
    Behind,
    /// The daemon ended the subscription.
    Ended,
    /// The connection ended.
    Lost(ClientError),
}

/// True when `error` says that the connection ended, not that the daemon refused.
fn lost(error: &ClientError) -> bool {
    matches!(error, ClientError::Closed | ClientError::Io { .. })
}

/// The note while the view connects again.
const RECONNECTING: &str = "the connection to efrd ended; connecting again";

/// The note when Ctrl+C got no answer and what this view sent comes back.
const UNCONFIRMED: &str =
    "what you queued or steered here goes back to your shell, but efrd may still run it";

/// The note when the connection did not come back and what this view sent comes back.
const GIVEN_BACK: &str = "efrd did not come back: what you queued or steered here goes back to your shell, because it may not run";

/// How a prompt that `prompt.withdraw` was asked to take back fared.
#[derive(Debug)]
enum TakeBack {
    /// The daemon took it back.
    Taken(WithdrawnPrompt),
    /// It no longer waits (`conflict`): it started, ended or was taken back already.
    /// The note says so.
    Started(String),
    /// The daemon does not know it (`not_found`). The note says so.
    Unknown(String),
    /// The daemon refused for another reason, which the note gives.
    Refused(String),
}

/// Why the call did not move when efrd asked again after the user's `y`.
const MOVE_AGAIN: &str = "efrd asks again; press ctrl+b again";

/// Why the call did not move when efrd answered in a way that this efr does not know.
const MOVE_UNKNOWN: &str = "efrd answered in a way that this efr does not know";

/// The note when efrd did not answer the move in time.
const MOVE_UNCONFIRMED: &str = "efrd did not confirm the move; efr jobs shows whether it moved";

/// The note when Ctrl+C took back the followed prompt before it ran.
const TAKEN_BACK: &str = "the prompt was taken back before it ran";

#[cfg(test)]
mod tests;

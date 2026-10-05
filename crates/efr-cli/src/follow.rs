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

mod view;

use std::time::Duration;

use efr_client::{Client, ClientError, ItemStream};
use efr_protocol::{
    ApprovalDecision, ApprovalRespond, ApprovalRespondResult, CallId, ConversationHistory,
    ConversationHistoryResult, ConversationId, ConversationSubscribe, ConversationSubscribeItem,
    ErrorCode, Event, InputRespond, InputRespondResult, Method, SecretText, Seq, TurnId,
    TurnInterrupt, TurnInterruptResult,
};
use efr_stdx::time::{Clock as _, Sleep};
use futures::StreamExt as _;
use serde_json::Value;

use crate::answer::{AnswerLine, Edit};
use crate::context::{Context, Stop};
use crate::error::CliError;
use crate::keys::{self, KeyReader};
use crate::output::Output;
use view::AnswerKind;

pub(crate) use view::{Ask, Step, TurnEnd, TurnView};

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

/// Where the turn to follow is.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Target {
    pub(crate) conversation: ConversationId,
    /// The turn, which Ctrl+C interrupts.
    pub(crate) turn: TurnId,
    /// The sequence number of the prompt's event; the turn's events come after it.
    pub(crate) after: Seq,
}

/// Follows `target` on `client` until the turn ends, writing it to `out`.
pub(crate) async fn follow(
    ctx: &Context,
    client: &Client,
    out: &mut Output,
    view: &mut TurnView,
    target: Target,
) -> Result<(), CliError> {
    let mut follower = Follower {
        ctx,
        client,
        target,
        last_seen: target.after,
        keys: None,
        silence: None,
        quit: None,
    };
    let result = match follower.blocking(out, view).await {
        Ok(()) => follower.run(out, view).await,
        Err(error) => Err(error),
    };
    // NOTE: the terminal must be back in its normal mode before anything else is
    // written or the process exits, whichever way the loop ended.
    if let Some((keys, asking)) = follower.keys.take() {
        stop(keys, &asking).await;
    }
    if let Err(error) = &result {
        let size = ctx.screen.size();
        write(out, &view.close(size))?;
        if matches!(error, CliError::Interrupted) {
            let note = interrupt(ctx, client, target).await;
            write(out, &view.note(&note, ctx.screen.size()))?;
        }
    }
    result
}

/// Asks the daemon to stop the followed turn, and says how that went.
async fn interrupt(ctx: &Context, client: &Client, target: Target) -> String {
    let method = Method::TurnInterrupt(TurnInterrupt {
        command_id: ctx.command_id(),
        conversation_id: target.conversation,
        turn_id: Some(target.turn),
    });
    let call = client.call::<TurnInterruptResult>(method);
    match ctx.clock.timeout(INTERRUPT_TIMEOUT, call).await {
        Ok(Ok(_)) => "interrupted".to_owned(),
        // The turn already ended, or it still waits behind another turn, which the
        // daemon cannot take back yet.
        Ok(Err(ClientError::Server { body })) if body.code == ErrorCode::Conflict => {
            "not interrupted: the turn is not running; a queued prompt still runs in its turn"
                .to_owned()
        }
        Ok(Err(error)) => {
            tracing::debug!(error = %error, "turn.interrupt failed");
            format!("the interrupt failed: {}", crate::format::one_line(&error.to_string()))
        }
        Err(_) => "the daemon did not confirm the interrupt; the turn may still run".to_owned(),
    }
}

struct Follower<'a> {
    ctx: &'a Context,
    client: &'a Client,
    target: Target,
    last_seen: Seq,
    /// Reads keys while an approval question or an input is pending.
    keys: Option<(KeyReader, Asking)>,
    /// The silent shell call and its activity count, with the time until it offers
    /// `Ctrl+\`.
    silence: Option<((CallId, u64), Sleep)>,
    /// Waits for `Ctrl+\` while the view offers it, and only then.
    quit: Option<Stop>,
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
}

/// Stops `keys`. A reader that read an answer line throws away what is still unread,
/// so the rest of a password neither shows nor reaches the user's shell.
async fn stop(keys: KeyReader, asking: &Asking) {
    match asking {
        Asking::Approval(_) => keys.stop().await,
        Asking::Input { .. } | Asking::Discard => keys.stop_discarding().await,
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
                Event::ApprovalRequested { turn_id, .. } if *turn_id != self.target.turn => {
                    pending.push(envelope.event);
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
            self.apply(step, out).await?;
        }
        Ok(())
    }

    async fn run(&mut self, out: &mut Output, view: &mut TurnView) -> Result<(), CliError> {
        let mut interrupt = self.ctx.interrupt.wait();
        let mut resubscribes = 0;
        loop {
            let mut stream = self.subscribe().await?;
            let resubscribe = loop {
                self.watch_silence(view);
                tokio::select! {
                    () = &mut interrupt => {
                        return Err(CliError::Interrupted);
                    }
                    key = next_key(&mut self.keys) => {
                        self.key(key, out, view).await?;
                    }
                    call_id = silent(&mut self.silence) => {
                        self.silence = None;
                        let step = view.silent(call_id, self.ctx.screen.size());
                        self.apply(step, out).await?;
                    }
                    () = pressed(&mut self.quit) => {
                        self.quit = None;
                        self.pressed(out, view).await?;
                    }
                    item = stream.next() => match item {
                        Some(Ok(value)) => {
                            resubscribes = 0;
                            if let Some(end) = self.item(value, out, view).await? {
                                return finished(end);
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
        self.apply(step, out).await.map(drop)
    }

    async fn subscribe(&self) -> Result<ItemStream<Value>, CliError> {
        let method = Method::ConversationSubscribe(ConversationSubscribe {
            conversation_id: self.target.conversation,
            after_seq: Some(self.last_seen),
            // A person at this terminal can type an answer exactly when approvals can be
            // answered here; without one, the daemon stops a command that waits for a
            // password nobody can type.
            answers_input: self.ctx.keys.available(),
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
            let step =
                view.event(&envelope.event, self.ctx.screen.size(), self.ctx.keys.available());
            if let Some(end) = self.apply(step, out).await? {
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

    /// Writes a step and starts or stops reading keys as it says.
    async fn apply(&mut self, step: Step, out: &mut Output) -> Result<Option<TurnEnd>, CliError> {
        write(out, &step)?;
        match step.ask {
            Some(ask) => {
                let asking = match ask {
                    Ask::Approval(call_id) => Asking::Approval(call_id),
                    Ask::Input { call_id, kind } => {
                        Asking::Input { call_id, kind, line: AnswerLine::new() }
                    }
                    Ask::Discard(_) => Asking::Discard,
                };
                let reader = match self.keys.take() {
                    // NOTE: a reader that runs is kept, so echo never comes back between
                    // two questions; the keys in its queue were typed before this one.
                    Some((mut reader, _)) => {
                        reader.discard_queued();
                        reader
                    }
                    // Starting the reader discards typeahead, so nothing typed before
                    // the question answers it or stays queued for the shell.
                    None => self.ctx.keys.start()?,
                };
                self.keys = Some((reader, asking));
            }
            None if step.settled || step.end.is_some() => {
                if let Some((keys, asking)) = self.keys.take() {
                    stop(keys, &asking).await;
                }
            }
            None => {}
        }
        Ok(step.end)
    }

    /// Handles one key; `None` means the key source ended.
    async fn key(
        &mut self,
        key: Option<u8>,
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
        match asking {
            Asking::Discard => {
                self.keys = Some((reader, Asking::Discard));
                Ok(())
            }
            Asking::Approval(call_id) => {
                let Some(decision) = keys::decision(key) else {
                    self.keys = Some((reader, Asking::Approval(call_id)));
                    return Ok(());
                };
                reader.stop().await;
                let step = view.answered(call_id, decision, self.ctx.screen.size());
                write(out, &step)?;
                self.respond(call_id, decision, out, view).await
            }
            Asking::Input { call_id, kind, mut line } => {
                let edit = line.key(key);
                let shown = (kind.shown() && edit == Edit::Changed).then(|| line.text().to_owned());
                let text = (edit == Edit::Submit).then(|| line.take());
                // NOTE: the reader goes back before anything can fail, so the exit path
                // still stops it and restores the terminal.
                self.keys = Some((reader, Asking::Input { call_id, kind, line }));
                if let Some(shown) = shown {
                    write(out, &view.typed(&shown, self.ctx.screen.size()))?;
                }
                match text {
                    Some(text) => self.answer(call_id, kind, text, out, view).await,
                    None => Ok(()),
                }
            }
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
            Ok(_) => view.answer_sent(size),
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
        self.apply(step, out).await.map(drop)
    }

    async fn respond(
        &self,
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
                write(out, &view.note(&line, self.ctx.screen.size()))
            }
            Err(error) => Err(error.into()),
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

/// `Ctrl+\` while it is offered; never resolves otherwise.
async fn pressed(quit: &mut Option<Stop>) {
    match quit {
        Some(wait) => wait.as_mut().await,
        None => std::future::pending().await,
    }
}

/// The next key while a question or an input is pending; never resolves otherwise.
async fn next_key(keys: &mut Option<(KeyReader, Asking)>) -> Option<u8> {
    match keys {
        Some((reader, _)) => reader.next().await,
        None => std::future::pending().await,
    }
}

fn write(out: &mut Output, step: &Step) -> Result<(), CliError> {
    out.err(&step.err);
    out.out(&step.out)
}

fn finished(end: TurnEnd) -> Result<(), CliError> {
    match end {
        TurnEnd::Completed => Ok(()),
        TurnEnd::Failed(body) => Err(CliError::TurnFailed { body }),
        TurnEnd::Interrupted => Err(CliError::TurnInterrupted),
        TurnEnd::Cancelled => Err(CliError::TurnCancelled),
    }
}

#[cfg(test)]
mod tests;

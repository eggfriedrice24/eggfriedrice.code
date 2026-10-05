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
//! never logged, and zeroed once it is sent or dropped.

mod view;

use std::time::Duration;

use efr_client::{Client, ClientError, ItemStream};
use efr_protocol::{
    ApprovalDecision, ApprovalRespond, ApprovalRespondResult, CallId, ConversationHistory,
    ConversationHistoryResult, ConversationId, ConversationSubscribe, ConversationSubscribeItem,
    ErrorCode, Event, InputRespond, InputRespondResult, Method, SecretText, Seq, TurnId,
    TurnInterrupt, TurnInterruptResult,
};
use efr_stdx::time::Clock as _;
use futures::StreamExt as _;
use serde_json::Value;

use crate::answer::{AnswerLine, Edit};
use crate::context::Context;
use crate::error::CliError;
use crate::keys::{self, KeyReader};
use crate::output::Output;

pub(crate) use view::{Ask, Step, TurnEnd, TurnView};

/// How often in a row a subscription may fall behind before the command gives up.
const MAX_RESUBSCRIBES: u32 = 8;

/// The events read to find the approvals that a queued prompt waits behind.
const BLOCKING_PAGE: u32 = 500;

/// How long Ctrl+C waits for the daemon to take the interrupt before the command ends
/// anyway.
const INTERRUPT_TIMEOUT: Duration = Duration::from_secs(3);

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
    let mut follower = Follower { ctx, client, target, last_seen: target.after, keys: None };
    let result = match follower.blocking(out, view).await {
        Ok(()) => follower.run(out, view).await,
        Err(error) => Err(error),
    };
    // NOTE: the terminal must be back in its normal mode before anything else is
    // written or the process exits, whichever way the loop ended.
    if let Some((keys, _)) = follower.keys.take() {
        keys.stop().await;
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
}

/// What the keys being read answer.
#[derive(Debug)]
enum Asking {
    /// An approval, with one key.
    Approval(CallId),
    /// The input that a running call waits for, with a line.
    Input { call_id: CallId, hidden: bool, line: AnswerLine },
}

impl Follower<'_> {
    /// Shows the approvals that the turn ahead of a queued prompt waits for, so the
    /// user can answer them here. They were asked before the prompt's event, where
    /// the subscription starts, so they come from the newest page of the log; one
    /// older than that page is not found.
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
        for envelope in page.events.into_iter().filter(|envelope| envelope.seq <= self.target.after)
        {
            match &envelope.event {
                Event::ApprovalRequested { turn_id, .. } if *turn_id != self.target.turn => {
                    pending.push(envelope.event);
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
        for event in pending {
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
                tokio::select! {
                    () = &mut interrupt => {
                        return Err(CliError::Interrupted);
                    }
                    key = next_key(&mut self.keys) => {
                        self.key(key, out, view).await?;
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
        let new_question = step.ask.is_some();
        if (step.settled || step.end.is_some() || new_question)
            && let Some((keys, _)) = self.keys.take()
        {
            keys.stop().await;
        }
        if let Some(ask) = step.ask {
            let asking = match ask {
                Ask::Approval(call_id) => Asking::Approval(call_id),
                Ask::Input { call_id, hidden } => {
                    Asking::Input { call_id, hidden, line: AnswerLine::new() }
                }
            };
            // Starting the reader discards typeahead, so nothing typed before the
            // question answers it or stays queued for the shell.
            self.keys = Some((self.ctx.keys.start()?, asking));
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
            reader.stop().await;
            return Ok(());
        };
        match asking {
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
            Asking::Input { call_id, hidden, mut line } => {
                let edit = line.key(key);
                let shown = (!hidden && edit == Edit::Changed).then(|| line.text().to_owned());
                let text = (edit == Edit::Submit).then(|| line.take());
                // NOTE: the reader goes back before anything can fail, so the exit path
                // still stops it and restores the terminal.
                self.keys = Some((reader, Asking::Input { call_id, hidden, line }));
                if let Some(shown) = shown {
                    write(out, &view.typed(&shown, self.ctx.screen.size()))?;
                }
                match text {
                    Some(text) => self.answer(call_id, hidden, text, out, view).await,
                    None => Ok(()),
                }
            }
        }
    }

    /// Sends the answer line `text` to the running call `call_id`, and says how that
    /// went. The text is dropped, and so zeroed, once the call returns.
    async fn answer(
        &self,
        call_id: CallId,
        hidden: bool,
        text: SecretText,
        out: &mut Output,
        view: &mut TurnView,
    ) -> Result<(), CliError> {
        let method = Method::InputRespond(InputRespond {
            conversation_id: self.target.conversation,
            call_id,
            text,
            hidden,
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
        write(out, &step)
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

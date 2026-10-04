//! Following a turn: subscribe to the conversation after the prompt's event, show the
//! turn's events as they arrive, answer approvals with one key, and return when the
//! turn ends.
//!
//! Ctrl+C ends the command and drops the connection; the daemon sees the connection
//! close and cancels the turn. A subscription that falls behind is resumed from the
//! last event shown, so a slow terminal loses nothing.

mod view;

use efr_client::{Client, ClientError, ItemStream};
use efr_protocol::{
    ApprovalDecision, ApprovalRespond, ApprovalRespondResult, CallId, ConversationId,
    ConversationSubscribe, ConversationSubscribeItem, ErrorCode, Method, Seq,
};
use futures::StreamExt as _;
use serde_json::Value;

use crate::context::Context;
use crate::error::CliError;
use crate::keys::{self, KeyReader};
use crate::output::Output;

pub(crate) use view::{Step, TurnEnd, TurnView};

/// How often in a row a subscription may fall behind before the command gives up.
const MAX_RESUBSCRIBES: u32 = 8;

/// Where the turn to follow is; the view knows which turn it is.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Target {
    pub(crate) conversation: ConversationId,
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
    let result = follower.run(out, view).await;
    // NOTE: the terminal must be back in its normal mode before anything else is
    // written or the process exits, whichever way the loop ended.
    if let Some((keys, _)) = follower.keys.take() {
        keys.stop().await;
    }
    if let Err(error) = &result {
        let size = ctx.screen.size();
        write(out, &view.close(size))?;
        if matches!(error, CliError::Interrupted) {
            write(out, &view.note("interrupted", size))?;
        }
    }
    result
}

struct Follower<'a> {
    ctx: &'a Context,
    client: &'a Client,
    target: Target,
    last_seen: Seq,
    /// Reads keys while an approval question is pending.
    keys: Option<(KeyReader, CallId)>,
}

impl Follower<'_> {
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
        if let Some(call_id) = step.ask {
            self.keys = Some((self.ctx.keys.start()?, call_id));
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
        let Some((reader, call_id)) = self.keys.take() else {
            return Ok(());
        };
        let Some(decision) = key.and_then(keys::decision) else {
            if key.is_some() {
                self.keys = Some((reader, call_id));
            } else {
                reader.stop().await;
            }
            return Ok(());
        };
        reader.stop().await;
        let step = view.answered(call_id, decision, self.ctx.screen.size());
        write(out, &step)?;
        self.respond(call_id, decision, out, view).await
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

/// The next key while a question is pending; never resolves otherwise.
async fn next_key(keys: &mut Option<(KeyReader, CallId)>) -> Option<u8> {
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

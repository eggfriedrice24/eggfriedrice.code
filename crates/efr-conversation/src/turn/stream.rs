//! One model call: the provider's stream folded into an assistant message.

use std::sync::Arc;

use efr_protocol::Event;
use efr_provider::{CompletionBuilder, Message, ProviderError, ProviderEvent, Request};
use futures::StreamExt as _;
use tracing::Instrument as _;

use super::Turn;
use super::coalesce::{Coalescer, sleep_or_pending};
use crate::ConversationError;

/// How one model call ended.
#[derive(Debug)]
pub(super) enum Response {
    /// The answer is complete.
    Done(Message),
    /// The provider failed, before the stream or inside it.
    Failed(ProviderError),
    /// The user interrupted the turn.
    Interrupted,
}

/// How the stream itself ended.
enum Streamed {
    Ended,
    Failed(ProviderError),
    Interrupted,
}

impl Turn {
    /// Sends `request` and records the answer as it streams: coalesced
    /// `assistant_message_updated` events with the text added since the last one while
    /// text arrives, then `assistant_message_completed` with the whole text. Text that
    /// streamed before a failure or an interrupt is completed too, so the log shows
    /// what the user saw.
    pub(super) async fn respond(
        &mut self,
        request: Request,
    ) -> Result<Response, ConversationError> {
        let provider = Arc::clone(&self.shared.deps.provider);
        let clock = Arc::clone(&self.shared.deps.clock);
        let interrupt = self.control.interrupt.clone();
        let span = tracing::debug_span!(
            "provider_request",
            provider = %provider.id(),
            model = %request.model,
        );
        let opened = tokio::select! {
            biased;
            () = interrupt.raised() => return Ok(Response::Interrupted),
            opened = provider.stream(request).instrument(span) => opened,
        };
        let mut stream = match opened {
            Ok(stream) => stream,
            Err(error) => return Ok(Response::Failed(error)),
        };
        let mut builder = CompletionBuilder::new();
        let mut updates = Coalescer::new(self.shared.config.update_interval);
        self.streamed = 0;
        let streamed = loop {
            let flush = updates.flush_after(clock.now());
            tokio::select! {
                biased;
                () = interrupt.raised() => break Streamed::Interrupted,
                () = sleep_or_pending(&*clock, flush) => {
                    updates.flushed(clock.now());
                    self.record_text(builder.text()).await?;
                }
                item = stream.next() => match item {
                    None => break Streamed::Ended,
                    Some(Err(error)) => break Streamed::Failed(error),
                    Some(Ok(event)) => {
                        if let ProviderEvent::Raw(raw) = &event {
                            tracing::debug!(raw = %raw, "the provider sent an event with no canonical form");
                        }
                        if let Err(error) = builder.push(&event) {
                            break Streamed::Failed(error);
                        }
                        let text_grew = matches!(&event, ProviderEvent::TextDelta { text } if !text.is_empty());
                        if text_grew && updates.offer(clock.now()) {
                            self.record_text(builder.text()).await?;
                        }
                    }
                },
            }
        };
        // NOTE: dropping the stream closes the provider's connection, so an interrupted
        // answer stops costing tokens before the interrupt is recorded as done.
        drop(stream);
        let partial = builder.text();
        let failure = match streamed {
            Streamed::Ended => match builder.finish() {
                Ok(completion) => {
                    if let Some(usage) = completion.usage {
                        self.usage = Some(self.usage.unwrap_or_default() + usage);
                    }
                    self.complete_text(completion.message.text()).await?;
                    return Ok(Response::Done(completion.message));
                }
                Err(error) => Response::Failed(error),
            },
            Streamed::Failed(error) => Response::Failed(error),
            Streamed::Interrupted => Response::Interrupted,
        };
        if !partial.is_empty() {
            self.complete_text(partial.clone()).await?;
            self.transcript.push(Message::assistant(partial));
        }
        Ok(failure)
    }

    /// Records what `text`, the current message so far, added since the last update.
    async fn record_text(&mut self, text: String) -> Result<(), ConversationError> {
        let Some(delta) = text.get(self.streamed..).filter(|delta| !delta.is_empty()) else {
            return Ok(());
        };
        let event = Event::AssistantMessageUpdated {
            turn_id: self.turn_id(),
            index: self.assistant_index,
            offset: self.streamed as u64,
            delta: delta.to_owned(),
        };
        self.record(vec![event]).await?;
        self.streamed = text.len();
        Ok(())
    }

    /// Records the whole text of the current assistant message, when it has any, and
    /// moves on to the next index.
    async fn complete_text(&mut self, text: String) -> Result<(), ConversationError> {
        if text.is_empty() {
            return Ok(());
        }
        let event = Event::AssistantMessageCompleted {
            turn_id: self.turn_id(),
            index: self.assistant_index,
            text,
        };
        self.record(vec![event]).await?;
        self.assistant_index += 1;
        self.streamed = 0;
        Ok(())
    }
}

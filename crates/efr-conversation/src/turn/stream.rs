//! One model call: the provider's stream folded into an assistant message.
//!
//! Two `debug` lines describe each call for the log. The `phase=model_answer` line says
//! how long the answer took and how it ended: `stop_reason=` with the provider's reason
//! for an answer that ended, or `outcome=interrupted` or `outcome=error` for one that
//! did not, because an interrupt or an error is no stop reason of the API (C2.8e). The
//! line keeps its start, so a reader can grep it.
//!
//! The usage line (target [`USAGE_TARGET`], DV12) gives the numbers of each call that
//! reported its usage: the gap since the conversation's previous model call
//! (`gap_ms`, from the shared clock; `none` for the first call since efrd started), the
//! input with its cached (`read`) part, the output, and what the call wrote to the
//! prompt cache (`write`, and `write_1h` with a time to live of one hour). It holds only
//! numbers, never text. The default filter (`info`) hides it; `log =
//! "info,efr_conversation::usage=debug"` shows it, for an estimate of which time to
//! live fits the pauses of a shell agent.

use std::sync::Arc;

use efr_protocol::Event;
use efr_provider::{CompletionBuilder, Message, ProviderError, ProviderEvent, Request, StopReason};
use efr_stdx::time::Stopwatch;
use futures::StreamExt as _;
use tracing::Instrument as _;

use super::Turn;
use super::coalesce::{Coalescer, sleep_or_pending};
use crate::{ConversationError, gap};

/// The log target of the usage line of each model call.
pub(crate) const USAGE_TARGET: &str = "efr_conversation::usage";

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
        let gap_ms = self.shared.gap.start(clock.now()).map(gap::millis);
        let span = tracing::debug_span!(
            "provider_request",
            provider = %provider.id(),
            model = %request.model,
            gap_ms,
        );
        // NOTE: the `phase` lines say where a call's time goes (docs/sandbox.md): here
        // the model's part, from the request to its first event, to the first tool
        // call, and to the end of the answer.
        let watch = Stopwatch::start();
        let mut first_event = true;
        let mut first_tool_call = true;
        let opened = tokio::select! {
            biased;
            () = interrupt.raised() => return Ok(Response::Interrupted),
            opened = provider.stream(request).instrument(span.clone()) => opened,
        };
        tracing::debug!(parent: &span, phase = "model_open", elapsed_ms = %watch, "phase=model_open elapsed_ms={}", watch);
        let mut stream = match opened {
            Ok(stream) => stream,
            Err(error) => return Ok(Response::Failed(error)),
        };
        self.call_usage = None;
        let mut builder = CompletionBuilder::new();
        let mut updates = Coalescer::new(self.config.update_interval);
        self.streamed = 0;
        self.drafter.begin_call();
        let streamed = loop {
            let now = clock.now();
            let flush = updates.flush_after(now);
            let draft_flush = self.drafter.flush_after(now);
            tokio::select! {
                biased;
                () = interrupt.raised() => break Streamed::Interrupted,
                () = sleep_or_pending(&*clock, flush) => {
                    updates.flushed(clock.now());
                    self.record_text(builder.text()).await?;
                }
                () = sleep_or_pending(&*clock, draft_flush) => {
                    self.drafter.flush(clock.now(), &builder, self.assistant_index);
                }
                item = stream.next() => match item {
                    None => break Streamed::Ended,
                    Some(Err(error)) => break Streamed::Failed(error),
                    Some(Ok(event)) => {
                        if first_event {
                            first_event = false;
                            tracing::debug!(parent: &span, phase = "model_first_event", elapsed_ms = %watch, "phase=model_first_event elapsed_ms={}", watch);
                        }
                        if first_tool_call && matches!(event, ProviderEvent::ToolCallStart { .. }) {
                            first_tool_call = false;
                            tracing::debug!(parent: &span, phase = "model_first_tool_call", elapsed_ms = %watch, "phase=model_first_tool_call elapsed_ms={}", watch);
                        }
                        if let ProviderEvent::Raw(raw) = &event {
                            tracing::debug!(raw = %raw, "the provider sent an event with no canonical form");
                        }
                        if let Err(error) = builder.push(&event) {
                            break Streamed::Failed(error);
                        }
                        // NOTE: the draft goes before the update, which waits for the commit,
                        // so a live client sees the text first.
                        if self.drafter.push(&event) {
                            self.drafter.offer(clock.now(), &builder, self.assistant_index);
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
        let outcome = match streamed {
            Streamed::Ended => builder.finish().map_err(Response::Failed),
            Streamed::Failed(error) => Err(Response::Failed(error)),
            Streamed::Interrupted => Err(Response::Interrupted),
        };
        // NOTE: after the outcome, so the line says why the model stopped; the line keeps
        // its start for a reader that parses the `phase` lines. Only an answer that ended
        // has a stop reason (C2.8e).
        match &outcome {
            Ok(completion) => {
                let stop = stop_name(completion.stop_reason);
                tracing::debug!(parent: &span, phase = "model_answer", elapsed_ms = %watch, stop_reason = stop, "phase=model_answer elapsed_ms={} stop_reason={}", watch, stop);
            }
            Err(failure) => {
                let ended =
                    if matches!(failure, Response::Interrupted) { "interrupted" } else { "error" };
                tracing::debug!(parent: &span, phase = "model_answer", elapsed_ms = %watch, outcome = ended, "phase=model_answer elapsed_ms={} outcome={}", watch, ended);
            }
        }
        let failure = match outcome {
            Ok(completion) => {
                if let Some(usage) = completion.usage {
                    log_usage(&span, gap_ms, &usage);
                    self.usage = Some(self.usage.unwrap_or_default() + usage);
                    self.call_usage = Some(usage);
                }
                self.last_call = completion.usage;
                self.answer_stop = Some(completion.stop_reason);
                self.complete_text(completion.message.text()).await?;
                return Ok(Response::Done(completion.message));
            }
            Err(failure) => failure,
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

/// Writes the usage line of one model call (DV12): numbers only, never text.
fn log_usage(span: &tracing::Span, gap_ms: Option<u64>, usage: &efr_provider::TokenUsage) {
    let gap = gap_ms.map_or_else(|| "none".to_owned(), |gap| gap.to_string());
    tracing::debug!(
        target: USAGE_TARGET,
        parent: span,
        gap_ms,
        input = usage.input_tokens,
        output = usage.output_tokens,
        read = usage.cached_input_tokens,
        write = usage.cache_write_tokens,
        write_1h = usage.cache_write_1h_tokens,
        "usage gap_ms={gap} input={} output={} read={} write={} write_1h={}",
        usage.input_tokens,
        usage.output_tokens,
        usage.cached_input_tokens,
        usage.cache_write_tokens,
        usage.cache_write_1h_tokens,
    );
}

/// The provider's stop reason as the log names it, such as `max_tokens`.
fn stop_name(stop: StopReason) -> &'static str {
    match stop {
        StopReason::EndTurn => "end_turn",
        StopReason::ToolUse => "tool_use",
        StopReason::MaxTokens => "max_tokens",
        StopReason::ContentFilter => "content_filter",
        StopReason::Refusal => "refusal",
        StopReason::PauseTurn => "pause_turn",
        _ => "other",
    }
}

#[cfg(test)]
mod tests;

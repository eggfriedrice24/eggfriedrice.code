//! The stream of a Messages call to canonical provider events: a pure state machine
//! over the events that `efr_http::SseDecoder` parses.
//!
//! | Event | What efr does |
//! |---|---|
//! | `message_start` | keeps `message.usage` as the base, and logs each entry of `message.input_transformations` (a block that `drop_block` dropped) at `warn` |
//! | `content_block_start` `text` or `thinking` | starts the block |
//! | `content_block_start` `redacted_thinking` | keeps the block for `provider_raw`, no event |
//! | `content_block_start` `tool_use` | `ToolCallStart { call_id, name }` |
//! | `content_block_delta` `text_delta` | `TextDelta` |
//! | `content_block_delta` `thinking_delta` | `ReasoningDelta` when it is not empty (a summary under `display: "summarized"`) |
//! | `content_block_delta` `signature_delta` | kept for `provider_raw` only |
//! | `content_block_delta` `input_json_delta` | `ToolCallDelta`; the fragments join |
//! | `content_block_stop` of a `tool_use` | `ToolCallEnd` with the joined input; an empty input is `{}` |
//! | `message_delta` | overwrites each usage member that it sends (the counts are cumulative) and keeps `stop_reason` |
//! | `message_stop` | one `Usage` (see `usage`) and `Done { stop_reason, provider_raw }` |
//! | `ping` | nothing |
//! | `error` | the error of its `error.type` (see `failure`); the request is never sent again |
//! | any other event, block or delta type | `Raw` or nothing, never a failure: the API adds types within a version |
//!
//! `provider_raw` is the content array of the assistant message as one JSON string:
//! the exact text that efr built from the stream, with every `thinking`,
//! `redacted_thinking`, `text` and `tool_use` block, the empty ones too, so the next
//! request sends it back byte for byte.
//!
//! Stop reasons: `end_turn` and `stop_sequence` are `EndTurn`, `tool_use` is
//! `ToolUse`, `max_tokens` is `MaxTokens`, `refusal` is `ContentFilter` (the turn runs
//! none of its tools), `model_context_window_exceeded` is `MaxTokens` with a `warn`
//! line, and `pause_turn` (server tools only, which efr does not send) is `EndTurn` with
//! a `warn` line.

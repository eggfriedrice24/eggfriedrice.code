//! Canonical requests to the body of `POST /messages`, as pure functions.
//!
//! The body:
//!
//! | Member | Value |
//! |---|---|
//! | `model` | `Request::model` |
//! | `max_tokens` | `Request::max_output_tokens`, else the model's `max_tokens` from the catalog; the API needs it |
//! | `system` | one `text` block with `Request::system`, left out when it is absent or empty |
//! | `tools` | `{name, description, input_schema}` in the request's order; a freeform tool goes in its function form |
//! | `tool_choice` | `{"type": "auto"}`, never `any` or `tool` |
//! | `thinking` | `{"type": "adaptive", "display": "summarized", "block_binding": {"prefix_mismatch_behavior": "drop_block"}}` |
//! | `output_config` | `{"effort": ...}`: `Request::effort`, else [`DEFAULT_EFFORT`](crate::DEFAULT_EFFORT); always sent |
//! | `messages` | the canonical messages, by the rules below |
//! | `stream` | `true` |
//!
//! Never sent: `temperature`, `top_p`, `top_k`, `stop_sequences`, `metadata`,
//! `service_tier`, `inference_geo`, and no key of `provider_options`.
//!
//! The messages:
//!
//! 1. An assistant message whose `provider_raw` this provider and model wrote goes back
//!    as that exact JSON text, through `serde_json::value::RawValue`.
//! 2. Any other assistant message is built from its text and tool calls; its reasoning
//!    is dropped, because another model's thinking cannot go back.
//! 3. Adjacent messages of one role merge into one, so the bytes depend only on the
//!    canonical history.
//! 4. In a user message, `tool_result` blocks come first, then any text.
//! 5. A `tool_use` without a `tool_result` gets an `is_error` result with one fixed
//!    text.
//! 6. Empty text blocks are dropped, and so is a message that becomes empty.
//! 7. A tool id outside `[a-zA-Z0-9_-]` has each other character replaced by `_`.
//!
//! The prompt cache markers come from `breakpoints`, a pure function of the shape of
//! the request; the conversion puts each marker on the block that it names. Two
//! following requests of one conversation give the same bytes for `tools`, `system`
//! and every earlier message, apart from the markers.

#[cfg_attr(not(test), expect(dead_code, reason = "the body conversion is not built yet"))]
mod breakpoints;

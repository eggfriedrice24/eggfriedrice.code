//! The model providers of efr: one module per API, each an implementation of
//! `efr_provider::Provider`.
//!
//! - [`openai`]: OpenAI's models through the Responses API, over the ChatGPT
//!   subscription backend or an API key, over HTTP or a WebSocket, with the model
//!   catalog of each backend.
//! - [`anthropic`]: Anthropic's Claude models through the Messages API with an API key
//!   (provider id `anthropic-api`), with explicit prompt cache markers and the model
//!   catalog of `GET /models`.
//!
//! The two modules share no code and each keeps its own error type ([`openai::OpenAiError`],
//! [`anthropic::AnthropicError`]), as the two crates did before ADR 0014 folded them
//! into this one.
//!
//! Allowed dependencies: `efr-provider`, `efr-http`, `efr-protocol` and `efr-stdx`.
//! What does not belong here: how a token is obtained, stored or refreshed
//! (`efr-oauth-openai` and `efr-credentials`; `xtask/src/deps.rs` forbids
//! `efr-providers -> efr-oauth-openai`, so the providers never see a refresh token;
//! tokens and keys arrive through `efr_provider::TokenSource`), the conversation's turn
//! loop and history (`efr-conversation`), and composing providers from the config, or
//! when to fetch a catalog (`efr-daemon`).

pub mod anthropic;
pub mod openai;

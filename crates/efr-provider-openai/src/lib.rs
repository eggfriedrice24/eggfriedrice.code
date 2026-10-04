//! OpenAI's models through the Responses API.
//!
//! [`OpenAiProvider`] implements `efr_provider::Provider`: it converts a canonical
//! request into a streaming `POST /responses`, sends it through `efr_http`'s client,
//! and turns the server-sent events back into canonical provider events. An
//! [`OpenAiConfig`] picks the [`Backend`]: the ChatGPT subscription at
//! [`SUBSCRIPTION_BASE_URL`] (with the `chatgpt-account-id` and `originator` headers)
//! or the public API at [`API_BASE_URL`] with an API key. Each assistant message keeps
//! the response's own output items as `provider_raw`, and the next request sends them
//! back verbatim, so encrypted reasoning survives without server-side storage.
//! [`subscription_models`] and [`api_models`] list the models each backend is known to
//! serve.
//!
//! Allowed dependencies: `efr-provider`, `efr-http`, `efr-protocol` and `efr-stdx`.
//! What does not belong here: how a token is obtained or refreshed (`efr-oauth-openai`,
//! which this crate must never depend on; tokens arrive through
//! `efr_provider::TokenSource`), the conversation's turn loop and history
//! (`efr-conversation`), and composing providers from the config (`efr-daemon`).

mod config;
mod convert;
mod error;
mod models;
mod responses;
mod sse_events;
#[cfg(test)]
mod testing;

pub use config::{
    API_BASE_URL, Backend, DEFAULT_ORIGINATOR, OpenAiConfig, ReasoningMode, SUBSCRIPTION_BASE_URL,
};
pub use error::OpenAiError;
pub use models::{DEFAULT_SUBSCRIPTION_MODEL, api_models, subscription_models};
pub use responses::OpenAiProvider;

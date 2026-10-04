//! OpenAI's models through the Responses API.
//!
//! An [`OpenAiConfig`] picks the [`Backend`]: the ChatGPT subscription at
//! [`SUBSCRIPTION_BASE_URL`] (with the `chatgpt-account-id` and `originator` headers)
//! or the public API at [`API_BASE_URL`] with an API key. [`subscription_models`] and
//! [`api_models`] list the models each backend is known to serve.
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

pub use config::{
    API_BASE_URL, Backend, DEFAULT_ORIGINATOR, OpenAiConfig, ReasoningMode, SUBSCRIPTION_BASE_URL,
};
pub use error::OpenAiError;
pub use models::{DEFAULT_SUBSCRIPTION_MODEL, api_models, subscription_models};

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
//!
//! The models come from a [`Catalog`]: the backend's own list, which a
//! [`CatalogClient`] fetches, its cache file ([`read_cache`], [`write_cache`]), or the
//! table built into efr. A [`ModelCatalog`] holds the current one for the provider.
//!
//! Allowed dependencies: `efr-provider`, `efr-http`, `efr-protocol` and `efr-stdx`.
//! What does not belong here: how a token is obtained or refreshed (`efr-oauth-openai`,
//! which this crate must never depend on; tokens arrive through
//! `efr_provider::TokenSource`), the conversation's turn loop and history
//! (`efr-conversation`), and composing providers from the config, or when to fetch the
//! catalog (`efr-daemon`).

mod catalog;
mod config;
mod convert;
mod error;
mod models;
mod responses;
mod sse_events;
#[cfg(test)]
mod testing;
mod timing;
mod websocket;

pub use catalog::{
    Applied, CLIENT_VERSION, Catalog, CatalogClient, CatalogOrigin, Fetched, ModelCatalog,
    read_cache, write_cache,
};
pub use config::{
    API_BASE_URL, Backend, DEFAULT_ORIGINATOR, OpenAiConfig, ReasoningMode, SUBSCRIPTION_BASE_URL,
    WebSocketMode,
};
pub use error::OpenAiError;
pub use responses::OpenAiProvider;

//! Anthropic's Claude models through the Messages API, with an API key.
//!
//! [`AnthropicProvider`] implements `efr_provider::Provider`: it converts a canonical
//! request into a streaming `POST <base_url>/messages` with explicit prompt cache
//! markers, sends it through `efr_http`'s client, and turns the server-sent events back
//! into canonical provider events. An [`AnthropicConfig`] holds the base URL
//! ([`API_BASE_URL`] by default), the models of the config, the retry policy, the
//! [`CacheTtl`] and the optional workspace id. Each assistant message keeps its content
//! as the exact JSON text that the stream built, as `provider_raw`, and the next request
//! writes that text back unchanged, so signed thinking survives.
//!
//! The models come only from the API: a [`CatalogClient`] fetches `GET /models`,
//! [`read_cache`] and [`write_cache`] keep the list in a file, and a [`ModelCatalog`]
//! holds the current [`Catalog`]. efr has no table of Claude models; it knows only
//! [`DEFAULT_MODEL`], [`DEFAULT_EFFORT`] and [`CACHE_MIN_TOKENS`]. [`check_key`] checks
//! an API key with one request that runs no model, before a login stores the key.
//!
//! Allowed dependencies: `efr-provider`, `efr-http`, `efr-protocol` and `efr-stdx`.
//! What does not belong here: how a key is stored or entered (`efr-credentials`,
//! `efr-daemon`, `efr-cli`; the key arrives through `efr_provider::TokenSource`), the
//! conversation's turn loop and history (`efr-conversation`), and composing providers
//! from the config, or when to fetch the catalog (`efr-daemon`).

mod catalog;
mod config;
#[cfg_attr(not(test), expect(dead_code, reason = "the model call is not built yet"))]
mod convert;
mod error;
mod failure;
mod messages;
mod models;
#[cfg_attr(not(test), expect(dead_code, reason = "the model call is not built yet"))]
mod sse_events;
#[cfg(test)]
mod testing;
mod timing;
#[cfg_attr(not(test), expect(dead_code, reason = "the model call is not built yet"))]
mod usage;

pub use catalog::{
    Applied, Catalog, CatalogClient, CatalogOrigin, ModelCatalog, check_key, read_cache,
    write_cache,
};
pub use config::{ANTHROPIC_VERSION, API_BASE_URL, AnthropicConfig, CacheTtl};
pub use error::AnthropicError;
pub use messages::AnthropicProvider;
pub use models::{CACHE_MIN_TOKENS, DEFAULT_EFFORT, DEFAULT_MODEL};

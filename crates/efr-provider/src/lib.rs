//! The model boundary: what the conversation sends to a model and what comes back,
//! whatever the provider.
//!
//! A [`Request`] carries canonical [`Message`]s made of [`ContentBlock`]s (text, tool
//! calls, tool results, reasoning, images) and [`ToolDefinition`]s. Each assistant
//! message keeps the provider's own items in `provider_raw`, so a provider gets back
//! exactly what it produced.
//!
//! Allowed dependencies: `efr-protocol` (for `Base64Bytes`) and `efr-stdx`. What does
//! not belong here: any provider's API, endpoints or event names
//! (`efr-provider-openai`), HTTP (`efr-http`), how a token is obtained or refreshed
//! (`efr-oauth-openai`), and tools themselves (`efr-tools`).

// NOTE: missing_docs is set here and not in Cargo.toml, because Cargo rejects a
// `[lints]` table that both inherits the workspace lints and adds its own.
#![warn(missing_docs)]

mod error;
mod message;
mod request;

pub use error::ProviderError;
pub use message::{ContentBlock, Message, Role};
pub use request::{Request, ToolDefinition};

//! The model boundary: what the conversation sends to a model and what comes back,
//! whatever the provider.
//!
//! A [`Request`] carries canonical [`Message`]s made of [`ContentBlock`]s (text, tool
//! calls, tool results, reasoning, images) and [`ToolDefinition`]s. Each assistant
//! message keeps the provider's own items in `provider_raw`, so a provider gets back
//! exactly what it produced. The answer streams back as [`ProviderEvent`]s, ending with
//! a [`StopReason`] and the call's [`TokenUsage`].
//!
//! [`Provider`] is the dyn-compatible trait every model client implements, named by a
//! [`ProviderId`]: [`Provider::stream`] is the one model call it must provide, returning
//! a [`ProviderStream`], and [`Provider::complete`] folds that stream into a
//! [`Completion`] with a [`CompletionBuilder`], which the conversation also uses while
//! it forwards events.
//!
//! A provider gets its credentials from a [`TokenSource`] ([`StaticToken`] for an API
//! key) as an [`AccessToken`], the token with the account it belongs to, and never
//! learns how a token was obtained. `SecretString` and `ExposeSecret` are re-exported
//! from `secrecy` because they appear in that trait.
//!
//! Allowed dependencies: `efr-protocol` (for `Base64Bytes` and `Usage`) and
//! `efr-stdx`. What does not belong here: any provider's API, endpoints or event names
//! (`efr-provider-openai`), HTTP (`efr-http`), how a token is obtained or refreshed
//! (`efr-oauth-openai`), and tools themselves (`efr-tools`).

// NOTE: missing_docs is set here and not in Cargo.toml, because Cargo rejects a
// `[lints]` table that both inherits the workspace lints and adds its own.
#![warn(missing_docs)]

mod completion;
mod error;
mod event;
mod message;
mod provider;
mod provider_id;
mod request;
mod token_source;
mod usage;

pub use completion::{Completion, CompletionBuilder};
pub use error::ProviderError;
pub use event::{ProviderEvent, StopReason};
pub use message::{ContentBlock, Message, Role};
pub use provider::{ModelInfo, Provider, ProviderStream};
pub use provider_id::ProviderId;
pub use request::{Request, ToolDefinition};
pub use secrecy::{ExposeSecret, SecretString};
pub use token_source::{AccessToken, StaticToken, TokenSource};
pub use usage::TokenUsage;

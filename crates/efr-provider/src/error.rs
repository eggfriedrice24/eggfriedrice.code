//! The one public error type of the crate.

use std::time::Duration;

/// Every way a model request can fail, for every provider.
///
/// Provider crates map their own failures onto these variants, so the conversation
/// decides what to do (refresh, back off, give up) without knowing the provider. A
/// token source reports through the same type, because its failures surface as a
/// failed request.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ProviderError {
    /// The provider rejected the credentials, after the token source was invalidated
    /// and asked once more.
    #[error("the provider rejected the credentials")]
    Unauthorized,

    /// The provider is rate limiting requests.
    #[error("the provider is rate limiting requests{}", retry_hint(*.retry_after))]
    RateLimited {
        /// How long the provider asked the client to wait, when it said.
        retry_after: Option<Duration>,
    },

    /// No credentials are stored for the provider, so there is no token to send.
    #[error("no credentials are stored for the provider")]
    NotLoggedIn,

    /// The token source could not produce an access token, for example because a
    /// refresh failed.
    #[error("the token source could not produce an access token")]
    Token {
        /// The token source's own error.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync + 'static>,
    },

    /// The request could not be sent, or the response could not be read.
    #[error("the request to the provider failed in transit")]
    Transport {
        /// The HTTP layer's error.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync + 'static>,
    },

    /// The provider answered with an error of its own.
    #[error("the provider answered with an error: {message}")]
    Api {
        /// The HTTP status, when the error came as a response rather than inside a
        /// stream.
        status: Option<u16>,
        /// The provider's machine-readable error code, when it sent one.
        code: Option<String>,
        /// The provider's message.
        message: String,
    },

    /// A response body or a stream event could not be parsed.
    #[error("the provider sent a response that could not be parsed")]
    Decode {
        /// The parser's error.
        #[source]
        source: serde_json::Error,
    },

    /// The provider does not serve the requested model.
    #[error("the provider does not serve the model {model:?}")]
    UnknownModel {
        /// The model id of the request.
        model: String,
    },

    /// The event stream broke the order that [`ProviderEvent`](crate::ProviderEvent)
    /// documents, such as arguments for a tool call that never started.
    #[error("the provider's event stream is malformed: {problem}")]
    InvalidStream {
        /// What was wrong.
        problem: &'static str,
    },

    /// The event stream ended before its `Done` event.
    #[error("the provider's event stream ended before the response was done")]
    Incomplete,

    /// A provider id breaks the naming rules of [`ProviderId`](crate::ProviderId).
    #[error("{id:?} is not a valid provider id")]
    InvalidProviderId {
        /// The rejected id.
        id: String,
    },
}

/// The tail of the rate-limit message: the delay in whole seconds, rounded up so a
/// reader never retries early.
fn retry_hint(retry_after: Option<Duration>) -> String {
    match retry_after {
        Some(delay) => {
            let seconds = delay.as_secs() + u64::from(delay.subsec_nanos() > 0);
            format!("; retry after {seconds}s")
        }
        None => String::new(),
    }
}

#[cfg(test)]
mod tests;

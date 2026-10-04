//! The one public error type of the crate.

use efr_http::HttpError;

/// Every way building an OpenAI provider's configuration can fail.
///
/// A model request fails with `efr_provider::ProviderError`, the error every provider
/// shares, so that the conversation handles all providers alike. This type covers only
/// the settings the daemon passes in, which are checked once, when the provider is
/// configured, instead of failing every request later.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum OpenAiError {
    /// The base URL is not an absolute `http` or `https` URL. The URL is not kept,
    /// because a URL may carry a credential in its query.
    #[error("the base URL is not an absolute http or https URL")]
    InvalidBaseUrl {
        /// Why the URL was refused.
        #[source]
        source: HttpError,
    },

    /// The `originator` value holds bytes that an HTTP header cannot carry.
    #[error("the originator {originator:?} is not a valid header value")]
    InvalidOriginator {
        /// The refused value.
        originator: String,
    },
}

#[cfg(test)]
mod tests;

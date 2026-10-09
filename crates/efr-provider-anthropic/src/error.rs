//! The one public error type of the crate.

use std::io;
use std::path::PathBuf;

use efr_http::HttpError;
use efr_stdx::StdxError;

/// Every way building an Anthropic provider's configuration, or reading and writing the
/// cache of its model catalog, can fail.
///
/// A model request, a fetch of the catalog and a key check fail with
/// `efr_provider::ProviderError`, the error every provider shares, so that the
/// conversation and the daemon handle all providers alike. This type covers the
/// settings that the daemon passes in, which are checked once, when the provider is
/// configured, and the cache file.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum AnthropicError {
    /// The base URL is not an absolute `http` or `https` URL. The URL is not kept,
    /// because a URL may carry a credential in its query.
    #[error("the base URL is not an absolute http or https URL")]
    InvalidBaseUrl {
        /// Why the URL was refused.
        #[source]
        source: HttpError,
    },

    /// The workspace id is empty or holds bytes that an HTTP header cannot carry.
    #[error("the workspace id {workspace_id:?} is not a valid header value")]
    InvalidWorkspaceId {
        /// The refused value.
        workspace_id: String,
    },

    /// The cache file of the model catalog could not be read.
    #[error("the model catalog cache {} could not be read", path.display())]
    CacheRead {
        /// The file.
        path: PathBuf,
        /// Why.
        #[source]
        source: io::Error,
    },

    /// The cache file of the model catalog is not JSON of the catalog's form.
    #[error("the model catalog cache {} is not a catalog", path.display())]
    CacheParse {
        /// The file.
        path: PathBuf,
        /// Why.
        #[source]
        source: serde_json::Error,
    },

    /// The model catalog could not be turned into the text of its cache file.
    #[error("the model catalog for {} could not be encoded", path.display())]
    CacheEncode {
        /// The file.
        path: PathBuf,
        /// Why.
        #[source]
        source: serde_json::Error,
    },

    /// The directory of the cache file could not be made.
    #[error("the directory {} of the model catalog cache could not be made", path.display())]
    CacheDir {
        /// The directory.
        path: PathBuf,
        /// Why.
        #[source]
        source: io::Error,
    },

    /// The cache file of the model catalog could not be written.
    #[error("the model catalog cache {} could not be written", path.display())]
    CacheWrite {
        /// The file.
        path: PathBuf,
        /// Why.
        #[source]
        source: StdxError,
    },
}

#[cfg(test)]
mod tests;

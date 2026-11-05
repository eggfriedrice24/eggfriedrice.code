//! `admin.login_openai`: log in to the OpenAI subscription, for `efr login openai`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The params of `admin.login_openai`, a streaming admin method (Unix socket only).
///
/// The daemon starts the login, sends the authorize URL as the first item, waits for
/// the browser to come back to its loopback listener, stores the credentials and ends
/// the stream. A second login while one is running gets `busy`. The login of one more
/// account asks the browser for a new sign-in, and a login that comes back with the
/// ChatGPT account of another login is refused before anything is stored.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AdminLoginOpenAi {
    /// The login: `openai-subscription`, or one more account of it such as
    /// `openai-subscription.b`. Absent: `openai-subscription`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
}

/// One item of an `admin.login_openai` stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum AdminLoginOpenAiItem {
    /// The URL to open in a browser. The CLI prints it, and opens it only when
    /// `EFR_OPEN_BROWSER` is on.
    AuthorizeUrl {
        /// The URL.
        url: String,
    },
    /// The login finished and the credentials are stored.
    Completed {
        /// The provider that is now logged in.
        provider: String,
    },
}

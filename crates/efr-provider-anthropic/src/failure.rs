//! What an error answer of the API means, and whether to send the request again.
//!
//! The provider sends a request through `efr_http::RetryPolicy::run`. For a status
//! that is not a success, one attempt reads the small error body, `{"type": "error",
//! "error": {"type", "message"}}`, and classifies it before the policy decides, so a
//! spend cap is never sent four times. `efr_http::is_retryable_status` does not decide
//! here, and no Anthropic rule goes into `efr-http`. The classes:
//!
//! | Answer | `ProviderError` | Again |
//! |---|---|---|
//! | 400 `invalid_request_error` whose message starts `prompt is too long` | `ContextOverflow` | no; the conversation compacts |
//! | 400 whose message starts `You have reached your specified API usage limits` | `Api` | no |
//! | any other 400, such as a missing `anthropic-workspace-id` | `Api` with the server's message | no |
//! | 401 `authentication_error` | `Unauthorized { message }`, at once (a key cannot refresh) | no |
//! | 402 `billing_error`, 403 `permission_error` | `Api` | no |
//! | 404 `not_found_error` for the model | `UnknownModel` | no |
//! | 413 `request_too_large` (a body over 32 MB) | `Api`, built here and not through `ProviderError::api`, which reads 413 as an overflow | no |
//! | 429 with `error.details.error_code: "enforced_spend_limit_reached"` | `Api` | no |
//! | any other 429 | `RateLimited { retry_after }` from `retry-after` | yes, after that wait |
//! | 500 `api_error`, 504 `timeout_error` | `Api` after the last try | yes, with backoff |
//! | 529 `overloaded_error` | `Overloaded` after the last try | yes, with backoff |
//! | an `error` event after a 200 | the same class by its `error.type` | never: the stream has started |
//!
//! A 500 or a 504 may come after the server ran the request, but a Messages call keeps
//! no state on the server and its stream has not started, so a second copy is safe.
//! Every failure is logged with the response's `request-id` header. Error messages come
//! only from the server's error body, never from the request.

use efr_provider::ProviderError;

/// The error code of a part of the provider that efr does not have yet.
const NOT_BUILT: &str = "not_built";

/// The error of a call into a part of the provider that is not built yet. It is an
/// `Api` error without a status, so the conversation fails the turn with `internal`
/// and never retries it.
pub(crate) fn not_built() -> ProviderError {
    ProviderError::Api {
        status: None,
        code: Some(NOT_BUILT.to_owned()),
        message: "efr cannot reach Anthropic's models yet".to_owned(),
    }
}

//! The rate limit of an API key that ran out, from the headers of a 429.
//!
//! The API tells its buckets in the headers of each answer:
//! `anthropic-ratelimit-<bucket>-remaining` and `anthropic-ratelimit-<bucket>-reset`,
//! for the buckets `requests`, `tokens`, `input-tokens` and `output-tokens`, with the
//! reset as an RFC 3339 time (platform.claude.com/docs/en/api/rate-limits). efr reads
//! them only from a 429 that outlived the retries: the boards show the rate limit of a
//! key only when it was hit.

use std::time::Duration;

use efr_http::HeaderMap;
use efr_provider::{LimitKind, LimitReached};
use jiff::{SignedDuration, Timestamp};

/// The buckets of a key: the kind and the name of the bucket in its headers.
const BUCKETS: [(LimitKind, &str); 4] = [
    (LimitKind::Requests, "requests"),
    (LimitKind::Tokens, "tokens"),
    (LimitKind::InputTokens, "input-tokens"),
    (LimitKind::OutputTokens, "output-tokens"),
];

/// The bucket that ran out: the one with nothing left whose reset comes last, because
/// the key serves again only when each one has room again. Its reset is the time of
/// its header, else `now` plus `retry_after`. `None` when no bucket in the headers is
/// empty: the server then only said "too many requests".
pub(crate) fn exhausted_bucket(
    headers: &HeaderMap,
    now: Timestamp,
    retry_after: Option<Duration>,
) -> Option<LimitReached> {
    let fallback = retry_after
        .and_then(|wait| SignedDuration::try_from(wait).ok())
        .and_then(|wait| now.checked_add(wait).ok());
    let text = |name: String| {
        headers.get(name).and_then(|value| value.to_str().ok()).map(|text| text.trim().to_owned())
    };
    BUCKETS
        .iter()
        .filter(|(_, bucket)| {
            text(format!("anthropic-ratelimit-{bucket}-remaining"))
                .and_then(|left| left.parse::<u64>().ok())
                == Some(0)
        })
        .map(|(kind, bucket)| {
            let resets_at = text(format!("anthropic-ratelimit-{bucket}-reset"))
                .and_then(|at| at.parse::<Timestamp>().ok())
                .or(fallback);
            LimitReached { kind: *kind, resets_at }
        })
        .max_by_key(|reached| reached.resets_at)
}

#[cfg(test)]
mod tests;

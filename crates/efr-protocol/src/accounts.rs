//! The accounts of the model providers: how full the limits of an account are, why an
//! account could not serve a turn, and the other accounts that can serve its model.
//!
//! An account is a provider id, such as `openai-subscription` (the default account of
//! its kind) or `anthropic-api.work`. efr learns the limits of an account from the
//! answers of the provider to its model calls, so the values are as old as the last
//! call. efrd keeps them in memory only.

use jiff::Timestamp;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// How full the limits of one account are, from the last answer of its provider that
/// told efr. efrd removes a window or a [`reached`](Self::reached) after its reset
/// time, sends none for an account that is signed out, and clears them at a login or a
/// logout.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AccountLimits {
    /// When efr learned the values: the time of the answer that carried them. A client
    /// shows the age of an old value, such as `12m ago`.
    pub observed_at: Timestamp,
    /// The usage windows of a plan, such as the 5-hour and the weekly window of a
    /// ChatGPT plan, in the order of the provider. Empty for an API key, whose provider
    /// tells no percent.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub windows: Vec<LimitWindow>,
    /// A limit that the account reached and that stops its calls until its reset, such
    /// as the requests limit of an API key. Absent while no limit stops the account.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reached: Option<LimitReached>,
}

/// One usage window of a plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LimitWindow {
    /// The length of the window in minutes, such as 300 or 10080. A client names the
    /// window from it: `5-hour`, `daily`, `weekly` or `monthly`. Absent when the
    /// provider does not tell it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minutes: Option<u32>,
    /// How much of the window the account used, in percent, from 0 to 100. At 100 the
    /// account reached the limit of the window.
    pub used_percent: u8,
    /// When the window starts again. Absent when the provider does not tell it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<Timestamp>,
}

/// A limit that an account reached.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LimitReached {
    /// Which limit.
    pub kind: LimitKind,
    /// When the account can serve again. Absent when the provider does not tell it,
    /// such as for an account with no credit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<Timestamp>,
}

/// Which limit an account reached.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum LimitKind {
    /// The usage limit of a plan, such as a ChatGPT plan. The provider does not say
    /// which window, so a client names none.
    Usage,
    /// The requests per minute of an API key.
    Requests,
    /// The tokens per minute of an API key.
    Tokens,
    /// The input tokens per minute of an API key.
    InputTokens,
    /// The output tokens per minute of an API key.
    OutputTokens,
    /// The provider answered "too many requests" to each try of a model call and told
    /// no limit, so efr stopped trying. Only in the cause of a failed turn, never in
    /// [`AccountLimits`].
    TooManyRequests,
    /// The account has no credit left, such as an API key with no balance. It has no
    /// reset.
    Credit,
    /// The account reached a spend limit: the limit of its tier, or the one that the
    /// user set at the provider.
    SpendLimit,
    /// A limit that this build does not know.
    #[serde(other)]
    Unknown,
}

impl LimitKind {
    /// Every kind, in the order of the enum.
    pub const ALL: [LimitKind; 9] = [
        LimitKind::Usage,
        LimitKind::Requests,
        LimitKind::Tokens,
        LimitKind::InputTokens,
        LimitKind::OutputTokens,
        LimitKind::TooManyRequests,
        LimitKind::Credit,
        LimitKind::SpendLimit,
        LimitKind::Unknown,
    ];
}

#[cfg(test)]
mod tests;

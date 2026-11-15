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
    /// no limit, so efr stopped trying. Only in [`AccountOut`], never in
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

/// Why the account of a turn could not serve it, the cause of a
/// [`Event::TurnFailed`](crate::Event::TurnFailed), with the other accounts that serve
/// the same model. efrd decides the list when the turn fails, so a client shows it as
/// it is. efrd never switches the account by itself: the user switches with
/// `,model <account>/<model>` or the model sheet of the phone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AccountOut {
    /// The account of the turn, as a provider id such as `openai-subscription`.
    pub provider: String,
    /// The user's name for the account, as in `models.list`. Absent when the config
    /// gives none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// The model of the turn, such as `gpt-6.1-sol`.
    pub model: String,
    /// The limit that stopped the account, with its reset when the provider told it.
    pub reached: LimitReached,
    /// The other accounts that list [`model`](Self::model), in the order of `catalogs`
    /// in `models.list`: each one that is [ready](AccountState::Ready), each one that
    /// is [signed out](AccountState::SignedOut) (a client names its login command and
    /// offers no switch), and each one that is [full](AccountState::Full) whose reset
    /// comes before the reset of [`reached`](Self::reached), or any known reset when
    /// `reached` has none. Empty when no other account runs the model: a client says
    /// so and offers nothing.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub others: Vec<OtherAccount>,
    /// The other accounts that list the model but that `others` leaves out, because each
    /// one is full and its reset does not come first. Absent when efrd leaves out none.
    /// When `others` offers no account, a client says that no other account has room for
    /// the model before the soonest of their resets, not that no other account runs it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub others_full: Option<OthersFull>,
}

/// The other accounts of an [`AccountOut`] that are full for longer than the stopped
/// account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct OthersFull {
    /// The soonest reset of those accounts, such as `06:02`: before it, no other account
    /// has room for the model. Absent when none of them has a known reset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<Timestamp>,
}

/// Another account that lists the model of a failed turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct OtherAccount {
    /// The account, as a provider id such as `openai-subscription.b`.
    pub provider: String,
    /// The user's name for the account, as in `models.list`. Absent when the config
    /// gives none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Whether the account can serve the model now.
    pub state: AccountState,
    /// For a [full](AccountState::Full) account: when it can serve again. Absent for
    /// the other states.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<Timestamp>,
}

/// Whether an account can serve a model now, as efrd knows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum AccountState {
    /// It has a login and reached no limit that efr knows of.
    Ready,
    /// It has no login. A turn on it fails until the user logs in.
    SignedOut,
    /// It reached a limit, which ends at its `resets_at`.
    Full,
    /// A state that this build does not know.
    #[serde(other)]
    Unknown,
}

impl AccountState {
    /// Every state, in the order of the enum.
    pub const ALL: [AccountState; 4] =
        [AccountState::Ready, AccountState::SignedOut, AccountState::Full, AccountState::Unknown];
}

#[cfg(test)]
mod tests;

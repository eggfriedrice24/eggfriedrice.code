//! `[accounts]`: the user's own names for the accounts of the model providers.
//!
//! ```toml
//! [accounts.labels]
//! "anthropic-api.work" = "Work"
//! openai-subscription = "Home plan"
//! ```
//!
//! A key is a provider id: a provider alone for its default account, or a provider
//! with an account, such as `anthropic-api.work`. efrd lists each account that the
//! table names in `models.list`, also while it has no login, so a client can show it
//! as signed out.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The longest label, in characters.
pub(crate) const LABEL_MAX_CHARS: usize = 40;

/// `[accounts]`: the user's names for the accounts. Every key applies at once.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
pub struct AccountsSettings {
    /// A name of your own for each account, such as `"anthropic-api.work" = "Work"`. A
    /// key is a provider id: `openai-subscription`, `openai-api` or `anthropic-api` for
    /// the default account of a kind, or one of them with `.` and the account name.
    /// A label has 1 to 40 characters and no control characters. `efr status`, the
    /// model list and the phone show the label in place of the id. efr lists each
    /// account that this table names, also while it has no login.
    pub labels: BTreeMap<String, String>,
}

impl AccountsSettings {
    /// The label of the account `provider`, such as `Work` for `anthropic-api.work`.
    pub fn label(&self, provider: &str) -> Option<&str> {
        self.labels.get(provider).map(String::as_str)
    }

    /// Every account that the table names, as a provider id, in the order of the ids.
    pub fn named(&self) -> impl Iterator<Item = &str> {
        self.labels.keys().map(String::as_str)
    }
}

/// True when `label` is a label that `[accounts.labels]` takes: 1 to
/// [`LABEL_MAX_CHARS`] characters, not only white space, and no control characters.
pub(crate) fn is_label(label: &str) -> bool {
    let chars = label.chars().count();
    (1..=LABEL_MAX_CHARS).contains(&chars)
        && !label.trim().is_empty()
        && !label.chars().any(char::is_control)
}

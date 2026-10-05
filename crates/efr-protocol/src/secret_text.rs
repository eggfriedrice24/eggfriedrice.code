//! [`SecretText`]: text that a user typed and that must never reach a log.

use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use zeroize::Zeroize as _;

/// Text that a user typed and that can be a secret, such as a password typed for `sudo`.
///
/// On the wire it is a plain JSON string. `Debug` prints a placeholder and never the
/// text, because requests are logged. The text is overwritten with zeros when the value
/// is dropped, so a password does not stay in freed memory. The one place that must use
/// the text reads it with [`SecretText::expose_secret`].
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct SecretText(String);

impl SecretText {
    /// Wraps `text`.
    pub fn new(text: impl Into<String>) -> Self {
        SecretText(text.into())
    }

    /// The text. The name makes every place that reads a secret easy to find.
    pub fn expose_secret(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SecretText {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretText(<redacted>)")
    }
}

impl Drop for SecretText {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

#[cfg(test)]
mod tests;

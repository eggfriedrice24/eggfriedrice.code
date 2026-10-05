//! [`SecretText`]: text that a user typed and that must never reach a log.

use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use zeroize::Zeroize as _;

/// Text that a user typed and that can be a secret, such as a password typed for `sudo`.
///
/// On the wire it is a plain JSON string. `Debug` prints a placeholder and never the
/// text, because requests are logged. The value's own buffer is overwritten with zeros
/// when it is dropped, and so is each clone's, which is a buffer of its own. Copies made
/// outside the value are not: [`SecretText::new`] copies text given as a `&str` (a
/// `String` moves in), serde_json unescapes a string that holds an escape (a quote, a
/// backslash, `\u`) in a scratch buffer that it frees without zeroing, and the encoded
/// frames are the business of `efr-client` and `efr-transport`, whose READMEs say which
/// of their buffers they zero. The one place that must use the text reads it with
/// [`SecretText::expose_secret`].
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

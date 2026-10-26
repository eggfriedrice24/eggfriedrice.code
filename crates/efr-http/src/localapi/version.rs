//! The version of tailscaled, from its `Tailscale-Version` header.

use std::fmt;

use http::HeaderMap;

use super::VERSION_HEADER;

/// A tailscaled version such as `1.104.1`, read from the long form that tailscaled
/// sends (`1.104.1-t0123abcd-g4567ef89`, or `1.105.0-dev20261025-t0123abcd` for a
/// development build).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TailscaleVersion {
    major: u32,
    minor: u32,
    patch: u32,
    long: String,
}

impl TailscaleVersion {
    /// The oldest tailscaled that efrd's remote listener supports: 1.100 sends the
    /// initial status on the IPN bus.
    pub const MINIMUM_REMOTE: (u32, u32, u32) = (1, 100, 0);

    /// The version in `text`, or `None` when it does not start with
    /// `major.minor.patch` (each a number).
    pub fn parse(text: &str) -> Option<TailscaleVersion> {
        let text = text.trim();
        let short = text.split(['-', '+', ' ']).next().unwrap_or_default();
        let mut parts = short.split('.');
        let mut number = || parts.next().and_then(|part| part.parse::<u32>().ok());
        let (major, minor, patch) = (number()?, number()?, number()?);
        if parts.next().is_some() {
            return None;
        }
        Some(TailscaleVersion { major, minor, patch, long: text.to_owned() })
    }

    /// The version in the `Tailscale-Version` header of `headers`, if it has one that
    /// parses.
    pub fn from_headers(headers: &HeaderMap) -> Option<TailscaleVersion> {
        let value = headers.get(VERSION_HEADER)?.to_str().ok()?;
        TailscaleVersion::parse(value)
    }

    /// `(major, minor, patch)`.
    pub fn triple(&self) -> (u32, u32, u32) {
        (self.major, self.minor, self.patch)
    }

    /// The text as tailscaled sent it, for a log line.
    pub fn long(&self) -> &str {
        &self.long
    }

    /// True when this version is `(major, minor, patch)` or newer.
    pub fn at_least(&self, minimum: (u32, u32, u32)) -> bool {
        self.triple() >= minimum
    }
}

impl fmt::Display for TailscaleVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

#[cfg(test)]
mod tests;

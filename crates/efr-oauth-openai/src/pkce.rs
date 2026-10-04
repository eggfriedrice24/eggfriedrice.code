//! PKCE with the S256 method (RFC 7636), and the OAuth `state`.
//!
//! Both are drawn from the injected `Rng`, so a test with a seeded generator gets the
//! same verifier on every run. Neither is ever logged: the verifier turns a stolen
//! authorization code into tokens, and the state is what tells a real callback from a
//! forged one.

use std::fmt;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use efr_stdx::rng::Rng;
use secrecy::{ExposeSecret as _, SecretString};
use sha2::{Digest as _, Sha256};
use zeroize::Zeroizing;

/// Random bytes behind a verifier: 64 bytes make 86 characters, inside the 43 to 128 of
/// RFC 7636 section 4.1. codex draws the same amount
/// (`codex-rs/login/src/oauth/pkce.rs:14-19`).
const VERIFIER_BYTES: usize = 64;

/// Random bytes behind a `state`: 32 bytes, 43 characters, as codex draws
/// (`codex-rs/login/src/oauth/authorization.rs:44-48`).
const STATE_BYTES: usize = 32;

/// A PKCE verifier and its S256 challenge.
pub(crate) struct Pkce {
    verifier: SecretString,
    challenge: String,
}

impl Pkce {
    /// A fresh verifier from `rng`.
    pub(crate) fn generate(rng: &dyn Rng) -> Self {
        let mut bytes = Zeroizing::new([0_u8; VERIFIER_BYTES]);
        rng.fill_bytes(&mut *bytes);
        Pkce::from_bytes(&*bytes)
    }

    /// The verifier that encodes `bytes`: URL-safe base64 without padding, whose
    /// alphabet is a subset of the unreserved characters RFC 7636 allows.
    pub(crate) fn from_bytes(bytes: &[u8]) -> Self {
        let verifier = SecretString::from(URL_SAFE_NO_PAD.encode(bytes));
        let challenge = challenge(verifier.expose_secret());
        Pkce { verifier, challenge }
    }

    /// The verifier, sent with the code exchange.
    pub(crate) fn verifier(&self) -> &SecretString {
        &self.verifier
    }

    /// The challenge, sent in the authorize URL.
    pub(crate) fn challenge(&self) -> &str {
        &self.challenge
    }
}

impl fmt::Debug for Pkce {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Pkce")
            .field("verifier", &self.verifier)
            .field("challenge", &self.challenge)
            .finish()
    }
}

/// The S256 challenge of `verifier`: `BASE64URL(SHA256(ASCII(verifier)))` without
/// padding (RFC 7636 section 4.2).
pub(crate) fn challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// A fresh OAuth `state` from `rng`.
pub(crate) fn state(rng: &dyn Rng) -> SecretString {
    let mut bytes = Zeroizing::new([0_u8; STATE_BYTES]);
    rng.fill_bytes(&mut *bytes);
    SecretString::from(URL_SAFE_NO_PAD.encode(bytes.as_slice()))
}

#[cfg(test)]
mod tests;

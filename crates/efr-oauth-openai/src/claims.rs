//! The claims of OpenAI's JWTs, read without checking a signature.
//!
//! The tokens come straight from the token endpoint over TLS, so their origin is
//! already settled; the claims only tell efr which ChatGPT account to name in the
//! `chatgpt-account-id` header, whose email to show, and when the access token ends.
//! No claim decides anything about security, so checking the signature (and fetching
//! the issuer's keys to do it) would add a network call and nothing else.
//!
//! The account id lives in the `https://api.openai.com/auth` claim, where codex reads
//! it (`codex-rs/login/src/token_data.rs:71-99`); opencode and goose also accept a
//! top-level `chatgpt_account_id` (`codex.ts:59-65`, `chatgpt_codex.rs:455-470`), which
//! is the fallback here. Their last fallback, the first organization's id, is left out:
//! it names an organization, not a ChatGPT account.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD_INDIFFERENT;
use jiff::Timestamp;
use secrecy::{ExposeSecret as _, SecretString};
use serde::Deserialize;
use zeroize::Zeroizing;

use crate::OAuthError;

/// What a token says about itself.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Claims {
    /// The ChatGPT account the token acts for.
    pub(crate) account_id: Option<String>,
    /// The user's email address.
    pub(crate) email: Option<String>,
    /// The `exp` claim.
    pub(crate) expires_at: Option<Timestamp>,
}

/// The claims of `jwt`. Fails when it is not three dot-separated parts or its payload
/// is not a base64url JSON object; claims that are missing or of an unexpected shape
/// are `None`.
pub(crate) fn parse(jwt: &str) -> Result<Claims, OAuthError> {
    let mut parts = jwt.split('.');
    let (Some(header), Some(payload), Some(_signature), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(OAuthError::MalformedJwt { problem: "it does not have three parts" });
    };
    if header.is_empty() || payload.is_empty() {
        return Err(OAuthError::MalformedJwt { problem: "a part is empty" });
    }
    let json = Zeroizing::new(
        URL_SAFE_NO_PAD_INDIFFERENT
            .decode(payload)
            .map_err(|_| OAuthError::MalformedJwt { problem: "the payload is not base64url" })?,
    );
    // An object first: a derived struct would also accept a JSON array, field by field.
    let object: serde_json::Map<String, serde_json::Value> =
        serde_json::from_slice(&json).map_err(|source| OAuthError::JwtClaims { source })?;
    let raw: RawClaims = serde_json::from_value(serde_json::Value::Object(object))
        .map_err(|source| OAuthError::JwtClaims { source })?;
    Ok(raw.into_claims())
}

/// The claims of a login's tokens, as far as they can be read: the account and the
/// email from the ID token, falling back to the access token, and the expiry from the
/// access token. A token that cannot be parsed counts as one without claims; codex and
/// goose treat it the same way, since a login with opaque tokens still works.
pub(crate) fn of_tokens(id_token: Option<&SecretString>, access_token: &SecretString) -> Claims {
    let readable = |token: &SecretString| match parse(token.expose_secret()) {
        Ok(claims) => Some(claims),
        Err(error) => {
            tracing::debug!(%error, "a token's claims could not be read");
            None
        }
    };
    let from_id = id_token.and_then(readable).unwrap_or_default();
    let from_access = readable(access_token).unwrap_or_default();
    Claims {
        account_id: from_id.account_id.or(from_access.account_id),
        email: from_id.email.or(from_access.email),
        expires_at: from_access.expires_at,
    }
}

/// The claims efr reads. Each field is optional and a field of an unexpected type
/// reads as missing, so a change elsewhere in OpenAI's claims cannot break a login.
#[derive(Deserialize)]
struct RawClaims {
    #[serde(default, deserialize_with = "lenient")]
    exp: Option<f64>,
    #[serde(default, deserialize_with = "lenient")]
    email: Option<String>,
    #[serde(default, deserialize_with = "lenient")]
    chatgpt_account_id: Option<String>,
    #[serde(rename = "https://api.openai.com/auth", default, deserialize_with = "lenient")]
    auth: Option<AuthClaims>,
    #[serde(rename = "https://api.openai.com/profile", default, deserialize_with = "lenient")]
    profile: Option<ProfileClaims>,
}

#[derive(Deserialize)]
struct AuthClaims {
    #[serde(default, deserialize_with = "lenient")]
    chatgpt_account_id: Option<String>,
}

#[derive(Deserialize)]
struct ProfileClaims {
    #[serde(default, deserialize_with = "lenient")]
    email: Option<String>,
}

impl RawClaims {
    fn into_claims(self) -> Claims {
        let non_empty = |value: Option<String>| value.filter(|text| !text.trim().is_empty());
        let nested_account = self.auth.and_then(|auth| auth.chatgpt_account_id);
        let nested_email = self.profile.and_then(|profile| profile.email);
        Claims {
            account_id: non_empty(nested_account).or_else(|| non_empty(self.chatgpt_account_id)),
            email: non_empty(self.email).or_else(|| non_empty(nested_email)),
            expires_at: self.exp.and_then(timestamp),
        }
    }
}

/// `exp` is a NumericDate: seconds since the epoch, possibly with a fraction (RFC 7519
/// section 2). A value outside the range of `Timestamp` reads as missing.
fn timestamp(exp: f64) -> Option<Timestamp> {
    if !exp.is_finite() {
        return None;
    }
    // NOTE: `as` saturates; a saturated value is outside the range of `Timestamp` and is
    // rejected by `from_second`.
    Timestamp::from_second(exp.floor() as i64).ok()
}

/// Reads a value of type `T`, or `None` when the JSON value has another shape.
fn lenient<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::de::DeserializeOwned,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(serde_json::from_value(value).ok())
}

#[cfg(test)]
mod tests;

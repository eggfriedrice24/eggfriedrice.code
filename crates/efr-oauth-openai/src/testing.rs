//! Fakes and builders shared by the unit tests of this crate.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use efr_credentials::{CredentialId, CredentialRecord, CredentialsError, OAuthTokens, SecretStore};
use efr_http::{HttpClient, HttpConfig};
use efr_test_support::{TestClock, TestRng};
use jiff::Timestamp;
use secrecy::{ExposeSecret as _, SecretString};
use wiremock::MockServer;

use crate::OAuthConfig;

/// The ChatGPT account the fixtures name.
pub(crate) const ACCOUNT: &str = "6b1c4a52-0f6e-4d55-9a3e-2f8b7c1d0e9a";

/// The email address the fixtures name.
pub(crate) const EMAIL: &str = "user@example.com";

/// The id every test stores its credential under.
pub(crate) fn credential() -> CredentialId {
    CredentialId::new("openai-subscription").unwrap()
}

/// The default config, pointed at `server` and at a free callback port.
pub(crate) fn config(server: &MockServer) -> OAuthConfig {
    OAuthConfig { issuer: server.uri(), callback_port: 0, ..OAuthConfig::default() }
}

/// An HTTP client whose retries wait on `clock`.
pub(crate) fn http(clock: &TestClock) -> HttpClient {
    HttpClient::new(&HttpConfig::default(), clock.shared(), Arc::new(TestRng::new(1))).unwrap()
}

/// A token endpoint body from `fixtures/tokens/`.
pub(crate) fn fixture(name: &str) -> Vec<u8> {
    let path = efr_test_support::fixtures::path(file!(), format!("tokens/{name}")).unwrap();
    std::fs::read(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

/// An unsigned JWT with `claims` as its payload.
pub(crate) fn jwt(claims: &serde_json::Value) -> String {
    let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"RS256","typ":"JWT"}"#);
    let payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(claims).unwrap());
    format!("{header}.{payload}.c2lnbmF0dXJl")
}

/// The instant `seconds` after the test clock's start.
pub(crate) fn at(seconds: i64) -> Timestamp {
    Timestamp::from_second(TestClock::START.as_second() + seconds).unwrap()
}

/// Tokens as a login leaves them: an access token named `access`, a refresh token
/// named `refresh`, expiring at `expires_at`, for [`ACCOUNT`].
pub(crate) fn tokens(
    access: &str,
    refresh: Option<&str>,
    expires_at: Option<Timestamp>,
) -> OAuthTokens {
    let mut tokens = OAuthTokens::new(SecretString::from(access));
    tokens.refresh_token = refresh.map(SecretString::from);
    tokens.expires_at = expires_at;
    tokens.account_id = Some(ACCOUNT.to_owned());
    tokens
}

/// A `SecretStore` in memory that counts its saves.
#[derive(Debug, Default)]
pub(crate) struct MemoryStore {
    records: Mutex<BTreeMap<String, CredentialRecord>>,
    saves: AtomicUsize,
    fail_saves: bool,
}

impl MemoryStore {
    /// A store holding `tokens` under [`credential`].
    pub(crate) fn with_tokens(tokens: OAuthTokens) -> Arc<Self> {
        let store = MemoryStore::default();
        store.put(CredentialRecord::OAuth(tokens));
        Arc::new(store)
    }

    /// A store whose saves fail, holding `tokens`.
    pub(crate) fn failing_saves(tokens: OAuthTokens) -> Arc<Self> {
        let store = MemoryStore { fail_saves: true, ..MemoryStore::default() };
        store.put(CredentialRecord::OAuth(tokens));
        Arc::new(store)
    }

    /// Replaces the record under [`credential`] without counting a save.
    pub(crate) fn put(&self, record: CredentialRecord) {
        self.records.lock().unwrap().insert(credential().as_str().to_owned(), record);
    }

    /// The OAuth tokens under [`credential`].
    pub(crate) fn tokens(&self) -> Option<OAuthTokens> {
        match self.records.lock().unwrap().get(credential().as_str()) {
            Some(CredentialRecord::OAuth(tokens)) => Some(tokens.clone()),
            _ => None,
        }
    }

    /// The stored access token, as text.
    pub(crate) fn access_token(&self) -> Option<String> {
        self.tokens().map(|tokens| tokens.access_token.expose_secret().to_owned())
    }

    /// How many times `save` ran.
    pub(crate) fn saves(&self) -> usize {
        self.saves.load(Ordering::SeqCst)
    }
}

impl SecretStore for MemoryStore {
    fn load(&self, id: &CredentialId) -> Result<Option<CredentialRecord>, CredentialsError> {
        Ok(self.records.lock().unwrap().get(id.as_str()).cloned())
    }

    fn save(&self, id: &CredentialId, record: &CredentialRecord) -> Result<(), CredentialsError> {
        self.saves.fetch_add(1, Ordering::SeqCst);
        if self.fail_saves {
            return Err(CredentialsError::MissingField { id: id.clone(), field: "test" });
        }
        self.records.lock().unwrap().insert(id.as_str().to_owned(), record.clone());
        Ok(())
    }

    fn delete(&self, id: &CredentialId) -> Result<bool, CredentialsError> {
        Ok(self.records.lock().unwrap().remove(id.as_str()).is_some())
    }
}

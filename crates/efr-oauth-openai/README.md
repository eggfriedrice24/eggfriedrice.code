# efr-oauth-openai

## Purpose

The ChatGPT subscription login: how efr obtains the OAuth tokens of a ChatGPT plan,
keeps them fresh, and hands them to the OpenAI provider.

Modules:

- `authorize`: OpenAI's constants (issuer `https://auth.openai.com`, client id
  `app_EMoamEEZ73f0CkXaXp7hrann`, port 1455, scopes
  `openid profile email offline_access`) and the authorize URL. The module doc cites
  the lines of codex, opencode and goose that each constant was checked against.
- `config`: `OAuthConfig`, those values as one struct with OpenAI's defaults, so tests
  point the flow at a local server and the daemon can change the `originator`.
- `pkce`: the S256 verifier and challenge (RFC 7636) and the OAuth `state`, drawn from
  the injected `efr_stdx::rng::Rng`.
- `callback`: the one-shot loopback listener on `127.0.0.1:1455` serving
  `/auth/callback`, and `LoginSlot`, which lets one login run at a time.
- `token`: the authorization code exchange and the refresh grant, behind one function
  whose body encoding is one constant (`ENCODING`, form by default, JSON one line
  away).
- `claims`: the claims of the ID and access tokens, read without checking a
  signature, for the ChatGPT account id, the email and the expiry.
- `login`: `OpenAiLogin` and `PendingLogin`, the flow the daemon drives for
  `admin.login_openai`.
- `token_source`: `OpenAiTokenSource`, the `efr_provider::TokenSource` over the saved
  tokens.
- `persist`: the `SecretStore` calls, run on tokio's blocking pool.

The daemon drives a login like this:

```rust
let pending = login.start().await?;             // binds 127.0.0.1:1455
stream_to_cli(pending.authorize_url());         // the CLI prints it
let completed = pending.complete().await?;      // callback, exchange, save
token_source.clear_cache();                     // the next request uses the new login
```

`start` fails with `LoginInProgress` while another login is pending. The listener is
bound only between `start` and the callback; dropping the `PendingLogin` cancels the
login and frees the port. A login that sees no callback within ten minutes (on the
injected clock) fails with `TimedOut`.

`OpenAiTokenSource` returns the access token with its ChatGPT account id. It refreshes
when less than five minutes remain on the injected clock, sends one refresh for any
number of concurrent callers (a `tokio::sync::Mutex` held only for the refresh), saves
the new record through the `SecretStore`, and after `invalidate` (the provider's 401)
refreshes once more. A refresh that fails while the token has not yet expired is logged
and the current token is used.

## Tier

Tier 2.

## Allowed dependencies

`efr-http` (the token requests), `efr-credentials` (the `SecretStore` the tokens live
in), `efr-provider` (the `TokenSource` trait) and `efr-stdx` (the `Clock` and `Rng`).
`xtask/src/deps.rs` holds the allowlist; `efr-provider-openai -> efr-oauth-openai` is
a forbidden edge, so the Responses client never sees a refresh token.

Third-party crates: `hyper` (with `server`), `hyper-util`, `http-body-util` and `bytes`
for the callback listener; `sha2` and `base64` for PKCE and the JWT payload; `tokio`,
`secrecy` (with `serde`), `zeroize`, `serde`, `serde_json`, `url`, `jiff`,
`async-trait`, `thiserror` and `tracing`.

## Invariant

- No token, authorization code, PKCE verifier or `state` reaches `Debug` output, a log
  line or an `OAuthError`: they are `secrecy::SecretString` values, error text from the
  server has the request's secrets replaced before it is kept, and token response
  bodies are zeroed when this crate holds the only copy.
- The callback listener binds the IPv4 loopback address only and exists only for the
  duration of one login; a callback whose `state` does not match is answered and
  otherwise ignored.
- Refresh timing and the login timeout use the injected `Clock`; the PKCE verifier and
  the `state` use the injected `Rng`.
- A request that may have reached the token endpoint is never sent twice: a code works
  once and a refresh may rotate the refresh token.

## Tests

Run the tests of this crate alone, without the rest of the workspace:

```sh
cargo nextest run -p efr-oauth-openai
```

The token tests answer from `wiremock` on the loopback interface with the bodies in
`fixtures/tokens/`; the callback tests bind free loopback ports and play the browser
with `efr-http`; refresh timing and the login timeout run on `efr-test-support`'s
`TestClock`, so nothing sleeps on real time. No test opens a browser, reaches OpenAI or
reads a real home directory, `~/.codex` or a real credential store.

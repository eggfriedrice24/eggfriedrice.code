# efr-provider

## Purpose

The model boundary. The conversation talks to every model through the types in this
crate and never sees a provider's API: `efr-provider-openai` (and later
`efr-provider-anthropic`) converts them to and from the wire.

Modules:

- `message`: canonical `Message { role, content, provider_raw }` with `Role` (user,
  assistant) and `ContentBlock` (text, tool call, tool result, reasoning, image).
  `provider_raw` is native passthrough: the provider's own items for an assistant
  message (for the Responses API, the encrypted `reasoning` item and the exact
  `function_call` items), stored verbatim in the event log and sent back unchanged on
  the next request to the same provider. Only that provider reads it; when the
  conversation switches providers the history assembler drops it.
- `request`: `Request { model, system, messages, tools, max_output_tokens,
  provider_options }` and `ToolDefinition`, the tool as the model sees it.
  `efr-tools` has its own `ToolSpec`; the conversation converts between the two,
  because neither crate depends on the other.
- `event`: `ProviderEvent` (`TextDelta`, `ReasoningDelta`, `ToolCallStart`,
  `ToolCallDelta`, `ToolCallEnd`, `Usage`, `Done`, `Raw`) and `StopReason`. The order
  a provider keeps is documented on the type: a tool call is start, deltas, end (with
  the complete arguments); `Done` is last and carries the message's `provider_raw`;
  provider events with no canonical form become `Raw` instead of being dropped.
- `usage`: `TokenUsage` with cached and reasoning parts; usages add with saturation
  and convert to the wire's `efr_protocol::Usage`.
- `provider`: the `Provider` trait, dyn-compatible through `async-trait`.
  `stream(Request) -> ProviderStream` is the one model call a provider implements;
  `complete` collects it by default, and `models` defaults to an empty list. `id`
  is also required: a `ProviderId` such as `openai-subscription` or `openai-api`,
  which decides whose `provider_raw` a message carries. `ModelInfo` describes a
  model's limits and the reasoning efforts it takes, with the backend's default.
- `provider_id`: `ProviderId`, 1 to 64 bytes of `[a-z0-9-]`, because it appears in
  logs, the config and the event log.
- `completion`: `Completion` and `CompletionBuilder`, which fold a stream into the
  assistant message by the order rules of `ProviderEvent` and reject a stream that
  breaks them. The conversation pushes each event into a builder as it forwards it,
  so the stored message and what clients saw come from the same events.
- `token_source`: the `TokenSource` trait (`access_token`, `invalidate`) and
  `StaticToken` for an API key. A provider holds an `Arc<dyn TokenSource>` and never
  sees a refresh token; `efr-oauth-openai` implements the trait for the subscription
  login. `access_token` returns an `AccessToken`: the token as a
  `secrecy::SecretString` (re-exported with `ExposeSecret`), whose `Debug` is
  redacted, and the account it belongs to, which the subscription path sends as
  `chatgpt-account-id`. The source reads the account from the token's claims, so a
  provider never parses a JWT and the token and its account come from one login.
- `error`: `ProviderError`, the crate's one error type, shared by every provider.
  `Unauthorized` is what a provider reports after a 401 survived one
  `TokenSource::invalidate` and retry; `RateLimited` carries the delay the provider
  asked for.

Serde forms: names are snake_case, internally tagged enums use the member `kind`,
optional members are left out when empty and unknown members are ignored, as on the
daemon's own socket.

## Tier

Tier 1.

## Allowed dependencies

`efr-protocol` (for `Base64Bytes` and `Usage`) and `efr-stdx`. The crate uses only
`efr-protocol`: nothing here reads a clock or draws randomness; token refresh timing
lives in `efr-oauth-openai`. `xtask/src/deps.rs` holds the allowlist.

Third-party crates: `async-trait`, `futures`, `secrecy`, `serde`, `serde_json` and
`thiserror`.

## Invariant

- The conversation never depends on a provider's API: everything it sends or receives
  is a type from this crate.
- A provider client never sees how its token was obtained: tokens arrive only through
  `TokenSource`, which is why `efr-provider-openai` may not depend on
  `efr-oauth-openai` (a forbidden edge in `xtask/src/deps.rs`). Tokens never reach a
  `Debug` string.
- `provider_raw` is opaque outside the provider that wrote it and survives storage
  unchanged: the same `serde_json::Value` comes back, numbers and strings exactly
  (object members come back sorted by key, so a provider must not depend on their
  order, and floats round trip only as well as serde_json's default parser allows).

## Tests

Run the tests of this crate alone, without the rest of the workspace:

```sh
cargo nextest run -p efr-provider
```

The tests pin the JSON shape of every canonical type and round-trip it, including a
proptest that any JSON value (without floats) survives as `provider_raw`; decision
tables cover the stream order rules of `CompletionBuilder` and the `ProviderId`
naming rules; an in-memory provider that implements only the required methods is
driven through `Arc<dyn Provider>`. They use no network, no real-time sleeps and no
Zig.

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

Third-party crates: `serde`, `serde_json` and `thiserror`.

## Invariant

- The conversation never depends on a provider's API: everything it sends or receives
  is a type from this crate.
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
proptest that any JSON value (without floats) survives as `provider_raw`. They use no
network, no real-time sleeps and no Zig.

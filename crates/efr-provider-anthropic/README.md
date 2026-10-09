# efr-provider-anthropic

## Purpose

The Anthropic Messages API client. `AnthropicProvider` implements
`efr_provider::Provider` over a streaming `POST <base_url>/messages` at
`https://api.anthropic.com/v1`, with an API key. Its provider id is `anthropic-api`.

Status: this crate is a skeleton. The types, the config, the usage mapping and the
placement of the cache markers are built; the model call, the stream mapping, the
catalog fetch, its cache file and the key check are not. Until they are,
`AnthropicProvider::stream`, `CatalogClient::fetch` and `check_key` fail with an `Api`
error of code `not_built`, `read_cache` finds no file and `write_cache` writes nothing.
The daemon does not build this provider yet.

Modules:

- `config`: `AnthropicConfig` (base URL, the models that the config lays over the
  catalog, retry policy, `CacheTtl`, optional workspace id), `API_BASE_URL` and
  `ANTHROPIC_VERSION` (`2023-06-01`). `CacheTtl` is `Auto` (the default),
  `FiveMinutes` or `OneHour`, from `[anthropic] cache_ttl` (`auto`, `5m`, `1h`).
- `models`: the only facts that efr knows without the API: `DEFAULT_MODEL`
  (`claude-opus-5-5`), `DEFAULT_EFFORT` (`medium`) and `CACHE_MIN_TOKENS` (512). There
  is no table of models.
- `catalog`: the model list from `GET <base_url>/models` (see "Catalog" below).
  `Catalog` is one list with its `CatalogOrigin` (`Backend` or `Cache`) and the time
  of the fetch; `ModelCatalog` holds the current one, or none; `ModelCatalog::apply`
  makes a list current unless it offers no model (`Applied::Refused`). `CatalogClient`
  fetches, `read_cache` and `write_cache` keep the list in a file of its own name, and
  `check_key` checks a key.
- `convert`: the request body and its messages, as pure functions; its
  `breakpoints` module places the prompt cache markers (see "Prompt cache" below).
- `sse_events`: a pure state machine from the stream's events to `ProviderEvent`s.
- `usage`: Anthropic's counts in efr's canonical sense (see "Token counts" below).
- `failure`: what an error answer means and whether to send the request again (see
  "Failures" below).
- `messages`: `AnthropicProvider`, the HTTP exchange, the retries and the event
  stream.
- `timing`: the `provider_accepted` and `provider_first_event` debug lines, in the
  form of `efr-provider-openai`, with `transport=http`.
- `error`: `AnthropicError`, for settings refused when the config is built (a base URL
  that is not `http` or `https`, a workspace id that cannot be a header value) and for
  the cache file. A request fails with `efr_provider::ProviderError`.
- `testing` (tests only): fakes and fixtures.

## The request

- Headers: `Authorization: Bearer <key>` through `HttpRequest::bearer_auth`, so the
  header is sensitive and no record shows it; `anthropic-version: 2023-06-01`;
  `anthropic-workspace-id` when `[anthropic] workspace_id` is set (a key that is not
  scoped to one workspace needs it); `anthropic-beta:
  thinking-binding-controls-2026-08-01`. No other beta header: an unknown one is a 400.
- Body: `model`; `max_tokens` (the request's limit, else the model's `max_tokens`);
  `system` as one text block, left out when empty; `tools` as function tools in the
  request's order; `tool_choice: {"type": "auto"}`; `thinking: {"type": "adaptive",
  "display": "summarized", "block_binding": {"prefix_mismatch_behavior":
  "drop_block"}}`; `output_config: {"effort": ...}`; `messages`; `stream: true`.
- Effort: `Request::effort`, else `DEFAULT_EFFORT`, always sent, because the API's
  own default differs by model. A change of the effort inside a conversation costs a
  rebuild of the messages cache.
- The provider reads no key of `Request::provider_options`: the conversation sends
  OpenAI's `prompt_cache_key` to every provider, and an unknown body member is a 400.
- An assistant message that this provider wrote for the same model goes back as the
  exact JSON text of its content (`provider_raw`, written through
  `serde_json::value::RawValue`), thinking blocks included. Any other assistant
  message is built from its text and tool calls, and its reasoning is dropped.
  Adjacent messages of one role merge; `tool_result` blocks come first in a user
  message; empty text blocks are dropped; a tool id outside `[a-zA-Z0-9_-]` gets `_`
  for each other character.
- Edit tool: every catalog model has `ModelInfo::edit_tool` `EditTool::Replace`, so
  the request offers `edit` (`path`, `old_string`, `new_string`, `replace_all`) and not
  `apply_patch`.

## Prompt cache

The markers are a pure function, `convert::breakpoints::place_breakpoints`:

```rust
pub(crate) fn place_breakpoints(layout: &Layout<'_>, ttl: CacheTtl) -> Vec<Breakpoint>;

pub(crate) struct Layout<'a> {
    pub(crate) system: bool,          // the body has a system block
    pub(crate) tools: bool,           // the body has at least one tool
    pub(crate) messages: &'a [Shape], // the body's messages, after the merge
    pub(crate) side_call: bool,       // Request::side_call
}
pub(crate) struct Shape { pub(crate) role: Role, pub(crate) opens_turn: bool, pub(crate) tokens: u64 }
pub(crate) struct Breakpoint { pub(crate) slot: Slot, pub(crate) target: Target, pub(crate) ttl: Ttl }
pub(crate) enum Slot { System, Anchor, Previous, Tail }
pub(crate) enum Target { System, LastTool, Message(usize) } // Message(i): the last block of message i
pub(crate) enum Ttl { FiveMinutes, OneHour }                 // "5m", "1h"
```

`opens_turn` is true for a user message that holds no `tool_result`; `tokens` is the
conversion's estimate of a message, the same for the same bytes. The rules:

- At most four markers, in the order of the prefix: S (the system block, else the last
  tool), A (the last anchor before the tail), P (the previous tail: the last user
  message before the tail, where the call before put its T) and T (the tail, the last
  message). Markers sit only on the system block, a tool or the last block of a user
  message, never on a `thinking` block. No top-level automatic marker.
- Anchors: every user message ends the request of one call. That call marks an anchor
  at it when the message opens a turn, or when the messages after the last anchor, that
  one included, hold more than `ANCHOR_STEP_TOKENS` (20,000). A side call never marks
  one. Because the history is append-only, every later request finds the same anchors,
  also after a restart.
- `CacheTtl::Auto`: S and A are one hour. A call that marks an anchor puts one hour on
  every marker, because the API refuses a one-hour marker after a five-minute one; any
  other call puts five minutes on P and T. Side calls, such as a compaction's summary
  request, keep five minutes on P and T. So a pause of any length loses at most the
  tool loop after the last anchor.
- `CacheTtl::FiveMinutes` and `CacheTtl::OneHour` put their one time on every marker.
- When two places are one message, it gets one marker with the longer time.
- Known gap: a new prompt after an interrupted call merges into the user message of
  the call's results, so it does not open a turn; it marks an anchor only by size.

Kept stable for a whole conversation: the tool list and its order, the system prompt,
the thinking mode and display, and the effort. A change of the tools invalidates every
level; a change of the effort invalidates the messages. Prices: a five-minute write
costs 1.25 times the input price, a one-hour write 2 times, a read 0.1 times or less.
Before each call, efrd logs the time since the conversation's last call at debug level,
to measure which time to live pays.

## Catalog

- `GET <base_url>/models?limit=1000`, more pages through `after_id` while `has_more` is
  true. A model is offered only when its `lifecycle` is `active` (the API's default
  filter also returns `deprecated` ones). An entry that cannot be read is left out.
- An offered model: `context_window` and `max_context_window` from `max_input_tokens`
  (1M on the current models; `[anthropic] models` can lower the window),
  `max_output_tokens` from `max_tokens`, `efforts` from `capabilities`,
  `default_effort` `DEFAULT_EFFORT` when the model takes it, `edit_tool`
  `EditTool::Replace`, no freeform tools, no WebSockets.
- `Catalog::default_model` is `DEFAULT_MODEL` when it is on offer, else the first model
  on offer. The compaction stays at efr's `auto_at` (76%) of the window.
- No table of models and no guessed window: without a list the provider offers no
  model. The daemon fetches at start, after a login and every hour; when no list exists
  at the start of a turn, it fetches first and waits for it. After a failed fetch it
  tries again in 15 s, 30 s, 1 min, 2 min, then every 5 min.
- The cache file holds `version`, `base_url`, `fetched_at` and the entries in the
  API's form, with mode 0600, under its own name beside the OpenAI catalog's file.
- `check_key(http, config, key)`: `GET <base_url>/models?limit=1` with the request's
  headers. `Ok` for a 200; a 401 is `Unauthorized { message }` and a 403 an `Api`
  error, each with the server's message; another status is `Api` with the status, and
  no answer is `Transport`.

## Token counts

The canonical counts of one call (`efr_provider::TokenUsage`):

| Canonical | Anthropic |
|---|---|
| `input_tokens` | `input_tokens + cache_creation_input_tokens + cache_read_input_tokens` |
| `cached_input_tokens` | `cache_read_input_tokens` |
| `cache_write_tokens` | `cache_creation_input_tokens` |
| `cache_write_1h_tokens` | `cache_creation.ephemeral_1h_input_tokens` |
| `output_tokens` | `output_tokens` |
| `reasoning_tokens` | `output_tokens_details.thinking_tokens` |

The API's `input_tokens` is only the input after the last marker, so efr adds the
three parts; without the sum, the context gauge of a warm cache reads about 0% and the
compaction never runs. A missing count is zero. The stream's `message_delta` counts
are cumulative: the mapper overwrites each count that an event sends and converts once,
at `message_stop`.

## Failures

| Answer | `ProviderError` | Sent again |
|---|---|---|
| 400 whose message starts `prompt is too long` | `ContextOverflow` | no; the conversation compacts |
| 400 whose message starts `You have reached your specified API usage limits` | `Api` | no |
| any other 400 | `Api` with the server's message | no |
| 401 `authentication_error` | `Unauthorized { message }`, at once | no |
| 402 `billing_error`, 403 `permission_error` | `Api` | no |
| 404 `not_found_error` for the model | `UnknownModel` | no |
| 413 `request_too_large` | `Api` (not `ContextOverflow`: a body over 32 MB) | no |
| 429 with `error.details.error_code` `enforced_spend_limit_reached` | `Api` | no |
| any other 429 | `RateLimited { retry_after }` | yes, after `retry-after` |
| 500 `api_error`, 504 `timeout_error` | `Api` after the last try | yes, with backoff |
| 529 `overloaded_error` | `Overloaded` after the last try | yes, with backoff |
| an `error` event after a 200 | the same class by `error.type` | never |

The attempts go through `efr_http::RetryPolicy::run` on the injected clock: one
attempt reads the error body and classifies it before the policy decides, so a spend
cap is never sent four times. `efr_http::is_retryable_status` does not decide here.
The conversation fails a turn with `unauthorized` for `Unauthorized`, `busy` for
`RateLimited` and `Overloaded`, and `invalid` for `UnknownModel`.

## Tier

Tier 2.

## Allowed dependencies

`efr-provider`, `efr-http`, `efr-protocol` and `efr-stdx`; `xtask/src/deps.rs` holds
the allowlist. The crate uses `efr-provider` (the trait and the canonical types),
`efr-http` (the client, the retry policy and the SSE parser) and `efr-stdx` (the
`Clock` and the `Rng`). `efr-protocol` is allowed but not needed.

It must never depend on `efr-oauth-openai`: the key arrives through
`efr_provider::TokenSource`, which says that it cannot refresh. That edge is
forbidden in `xtask/src/deps.rs`.

Third-party crates: `async-trait`, `jiff` (the time of a fetched catalog),
`serde_json` (with `raw_value`, for the exact replay of assistant content) and
`thiserror`.

## Invariant

- The client never sees how a key was stored or entered, and the key never reaches a
  log, an error text, a `Debug` string or a recorded request: it goes in a sensitive
  `Authorization` header, and error messages come from the server's error body, never
  from the request.
- `provider_raw` goes back to the API exactly as it came, byte for byte.
- Two following requests of one conversation have the same bytes for `tools`, `system`
  and every earlier message, apart from the cache markers.
- A model call is never sent again after its stream has started.
- No model table and no guessed window: every model fact comes from the API.

## Tests

Run the tests of this crate alone, without the rest of the workspace:

```sh
cargo nextest run -p efr-provider-anthropic
```

`config` tests pin the defaults, the URLs and the refused settings. `usage` tests check
the three-part input sum, the one-hour writes and the thinking count. `catalog` tests
check the default model and what `apply` does with a list. `convert::breakpoints` tests
are table tests of the placement: the walkthrough of two turns with a tool loop, the
anchor of a grown tool loop, a side call, each time to live, the fallbacks of S, and
every short history under every setting against the rules of the API (at most four
markers, in order, no one-hour marker after a five-minute one). The tests make no
network call, sleep on no real time and need no Zig.

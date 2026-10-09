# efr-provider-anthropic

## Purpose

The Anthropic Messages API client. `AnthropicProvider` implements
`efr_provider::Provider` over a streaming `POST <base_url>/messages` at
`https://api.anthropic.com/v1`, with an API key. Its provider id is `anthropic-api`.

Status: the config, the request body with its cache markers, the stream mapping, the
usage sums, the HTTP exchange with its retries, the catalog fetch, its cache file and
the key check are built. efrd builds the provider for `[model] provider =
"anthropic-api"` (`efr-daemon`, `providers.rs`) and shares one `ModelCatalog` between
the provider and its fetch of the list (`catalog.rs`, cache file
`anthropic_model_catalog.json`).

Modules:

- `config`: `AnthropicConfig` (base URL, the models that the config lays over the
  catalog, retry policy, `CacheTtl`, optional workspace id), `API_BASE_URL` and
  `ANTHROPIC_VERSION` (`2023-06-01`). `CacheTtl` is `Auto` (the default),
  `FiveMinutes` or `OneHour`, from `[anthropic] cache_ttl` (`auto`, `5m`, `1h`).
- `models`: the only facts that efr knows without the API: `DEFAULT_MODEL`
  (`claude-opus-5-5`) and `DEFAULT_EFFORT` (`medium`). There is no table of models.
  The cache minimum of 512 tokens is a fact of its module doc, not a constant, because
  no code reads it.
- `catalog`: the model list from `GET <base_url>/models` (see "Catalog" below).
  `Catalog` is one list with its `CatalogOrigin` (`Backend` or `Cache`) and the time
  of the fetch; `ModelCatalog` holds the current one, or none; `ModelCatalog::apply`
  makes a list current unless it offers no model (`Applied::Refused`). `CatalogClient`
  fetches, `read_cache` and `write_cache` keep the list in a file of its own name, and
  `check_key` checks a key.
- `convert`: `request_body`, the request body and its messages as a pure function,
  and `THINKING_BINDING_BETA`, the one beta header; its `breakpoints` module places
  the prompt cache markers (see "Prompt cache" below). The provider calls
  `request_body(&Request, &AnthropicConfig, Option<&ModelInfo>) -> Result<MessagesBody,
  ProviderError>` once per call and sends `MessagesBody::betas()` as the
  `anthropic-beta` header.
- `sse_events`: `EventMapper`, a pure state machine from the stream's events to
  `ProviderEvent`s (see "The stream" below). The provider calls `EventMapper::new()`,
  then `map(&SseEvent) -> Result<Vec<ProviderEvent>, ProviderError>` for each event
  until `is_done()`; an `error` event goes through `failure::event`.
- `usage`: Anthropic's counts in efr's canonical sense, and `StreamUsage`, which keeps
  the cumulative counts of one stream (see "Token counts" below).
- `failure`: what an error answer means and whether to send the request again (see
  "Failures" below).
- `messages`: `AnthropicProvider`, the HTTP exchange, the retries and the event
  stream, and `sign`, the headers of every request to the API.
- `timing`: the `provider_accepted` and `provider_first_event` debug lines, in the
  form of `efr-provider-openai`, with `transport=http`.
- `error`: `AnthropicError`, for settings refused when the config is built (a base URL
  that is not `http` or `https`, a workspace id that cannot be a header value) and for
  the cache file. A request fails with `efr_provider::ProviderError`.
- `testing` (tests only): a clock whose sleeps end at once, a fixed random source, a
  token source of one test key, model list entries and pages, server-sent events, a
  log that a test reads and the loader of the stream fixtures in `fixtures/messages/`.
  The fake API is `wiremock`.

## The request

- Headers: `Authorization: Bearer <key>` through `HttpRequest::bearer_auth`, so the
  header is sensitive and no record shows it; `anthropic-version: 2023-06-01`;
  `anthropic-workspace-id` when `[anthropic] workspace_id` is set (a key that is not
  scoped to one workspace needs it); `anthropic-beta:
  thinking-binding-controls-2026-08-01`. No other beta header: an unknown one is a 400.
  No `x-api-key`: the API takes Bearer as its main form and calls `x-api-key` the
  legacy one, and efr sends one credential header only. Also `Accept:
  text/event-stream` and `Content-Type: application/json`. The model call goes to the
  `efr_http` recorder with the key hidden; the fetch of the list and the key check do
  not.
- Body, in this member order: `model`; `stream: true`; `max_tokens`; `system` as one
  text block, left out when empty; `tools` as function tools in the request's order
  (a freeform tool in its function form); `tool_choice: {"type": "auto"}`, left out
  with the tools when there is none; `thinking: {"type": "adaptive", "display":
  "summarized", "block_binding": {"prefix_mismatch_behavior": "drop_block"}}`;
  `output_config: {"effort": ...}`, left out without an effort; `messages`. Never
  sent: sampling members, `stop_sequences`, `metadata`, `service_tier` and
  `inference_geo`.
- `max_tokens`: the request's limit, at most the model's `max_tokens` from the
  catalog; without a request limit, the model's. When neither is known, the body is
  not built and the call fails with `UnknownModel`: efr guesses no model fact. A model
  that the list does not hold first needs a key: without one, the call fails with
  `NotLoggedIn`, because a missing login is also why no list came.
- Effort: `Request::effort`, else the model's default effort from the catalog
  (`DEFAULT_EFFORT` when the model lists it), because the API's own default differs
  by model. A model without such an effort, such as one whose `capabilities.effort`
  is not supported or one that only `[anthropic] models` names, gets no
  `output_config`: the API refuses an effort that the model does not take. A change
  of the effort inside a conversation costs a rebuild of the messages cache.
- The provider reads no key of `Request::provider_options`: the conversation sends
  OpenAI's `prompt_cache_key` to every provider, and an unknown body member is a 400.
- An assistant message that this provider wrote goes back as the exact JSON text of
  its content (`provider_raw`, a JSON string, written through
  `serde_json::value::RawValue`), thinking blocks included. The conversation drops
  `provider_raw` when the model changes. An empty `text` block in a `provider_raw` is
  dropped, and the other blocks then go back one by one, each exact. Any other
  assistant message, or a `provider_raw` that is not such a text, is built from its
  text and tool calls, and its reasoning is dropped; a tool call input that is not an
  object goes as `{"input": <value>}`.
- Adjacent messages of one role merge, and the blocks of merged raw messages stay
  exact. `tool_result` blocks come first in a user message, and a result with an empty
  output has no `content`. Empty text blocks are dropped, and so is a message that
  becomes empty. A `tool_use` that the next message does not answer gets a failed
  result with one fixed text. A tool id outside `[a-zA-Z0-9_-]` gets `_` for each
  other character.
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
conversion's estimate of a message: its JSON bytes without markers over 4, rounded up,
so the same for the same bytes. A marker is `"cache_control": {"type": "ephemeral",
"ttl": "5m"}` (or `"1h"`), always the last member of its block. The rules:

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
- State: the placement needs the place of the last anchor and the tokens after it. No
  one keeps these numbers. The placement computes them again from the messages on every
  call, so the state is the history itself, which `efr-conversation` keeps and sends
  whole. The provider keeps nothing between calls.
- `CacheTtl::Auto`: S and A are one hour. A call that marks an anchor puts one hour on
  every marker, because the API refuses a one-hour marker after a five-minute one; any
  other call puts five minutes on P and T. Side calls, such as a compaction's summary
  request, keep five minutes on P and T. So a pause of any length loses at most the
  tool loop after the last anchor.
- `CacheTtl::FiveMinutes` and `CacheTtl::OneHour` put their one time on every marker.
- When two places are one message, it gets one marker with the longer time.
- On a call that marks an anchor, P is one hour too, over the entry that the call
  before wrote for five minutes. Whether the API then keeps a full one-hour entry is a
  check on a test key before the release; if it fails, `auto` falls back to one hour on
  every marker.
- Known gap: a new prompt after an interrupted call merges into the user message of
  the call's results, so it does not open a turn; it marks an anchor only by size. The
  tool loop after the last anchor then has a five-minute entry only, and a pause of
  more than five minutes in the new turn loses it, which can be more than
  `ANCHOR_STEP_TOKENS`. A prompt after a turn that failed before an answer merges into
  that turn's prompt, which opens a turn, so it marks an anchor.

Kept stable for a whole conversation: the tool list and its order, the system prompt,
the thinking mode and display, and the effort. A change of the tools invalidates every
level; a change of the effort invalidates the messages. Prices: a five-minute write
costs 1.25 times the input price, a one-hour write 2 times, a read 0.1 times or less.
Each request writes one debug line with the setting and its markers in order, the
letter and the time of each place, such as `cache_ttl=auto markers=S1h,A1h,P5m,T5m`
(`convert::breakpoints::summary`). Under `auto`, `T1h` means that the call marked a new
anchor. The line runs inside the conversation's `provider_request` span, whose
`gap_ms` field is the time since the start of the conversation's call before (absent
for the first call after a start of efrd; see the README of `efr-conversation`). So
the line, the gap and the call's `usage` measure which time to live pays.

## The stream

`sse_events::EventMapper` reads the events of one answer in order; its module doc has
the full table. In short:

- Text deltas are `TextDelta`, thinking deltas are `ReasoningDelta` (a blank line
  between two thinking blocks), a `tool_use` is `ToolCallStart`, a `ToolCallDelta` for
  each non-empty `partial_json` fragment and `ToolCallEnd` at its `content_block_stop`.
  An input that is not a JSON object at that point fails the stream with
  `InvalidStream`, so a call that the output limit cut off never runs.
- `message_stop` gives one `Usage` and `Done`. `provider_raw` is a JSON string that
  holds the content array: `text`, `thinking` (with its signature) and `tool_use`
  blocks built from the stream with the API's member order and the input text that the
  model wrote, and `redacted_thinking` or unknown blocks as the API sent them. An
  empty `thinking` block stays, because its signature counts. An empty `text` block is
  left out: the API refuses one in a request (`text content blocks must be
  non-empty`), and it would go back in every later request. An answer without blocks
  has no `provider_raw`.
- Stop reasons: `end_turn` and `stop_sequence` are `EndTurn`, `tool_use` is `ToolUse`,
  `max_tokens` and `model_context_window_exceeded` are `MaxTokens`, `refusal` is
  `ContentFilter`, `pause_turn` is `EndTurn`; an unknown or missing one is `ToolUse`
  with a tool call, else `EndTurn`.
- `ping` is ignored, `citations_delta` too. Any other unknown event, block or delta is
  `Raw`, never a failure.
- An `error` event fails the stream with the class of its `error.type` (see
  "Failures"); `message.input_transformations` entries of `message_start` are logged at
  `warn`.

## Catalog

- `GET <base_url>/models?limit=1000`, more pages through `after_id` (the page's
  `last_id`) while `has_more` is true, with the headers of a model call but no
  `anthropic-beta`. A page that says `has_more` but names no new `last_id`, or a list
  of more than 100 pages, fails the fetch. A model is offered only when its
  `lifecycle` is `active` (the API's default filter also returns `deprecated` ones);
  an entry without a `lifecycle` counts as `active`. An entry that cannot be read is
  left out.
- An offered model: `context_window` and `max_context_window` from `max_input_tokens`
  (1M on the current models; `[anthropic] models` can lower the window),
  `max_output_tokens` from `max_tokens` (a missing or zero count stays unknown),
  `efforts` from `capabilities.effort` (each level whose `supported` is true, from
  `low` to `max`, then any new level by name), `default_effort` `DEFAULT_EFFORT` when
  the model takes it, `edit_tool` `EditTool::Replace`, no freeform tools, no
  WebSockets.
- `AnthropicProvider::models` lays the models of the config over the catalog: an
  entry of the same id sets the window and the output limit, each up to the catalog's
  limit, and any other entry comes after the catalog's models.
- A 401 fails the fetch with `Unauthorized { message }` at once; a token source that
  can refresh gets one refresh first. Each page goes through the retry policy with the
  classes of "Failures" below, as a model call does: a 529 is sent again, a spend cap
  is not. Because the request is a `GET`, a timeout or a broken connection is sent
  again too.
- `Catalog::default_model` is `DEFAULT_MODEL` when it is on offer, else the first model
  on offer. The compaction stays at efr's `auto_at` (76%) of the window.
- No table of models and no guessed window: without a list the provider offers no
  model. The daemon fetches at start, after a login and every hour; when no list exists
  when a prompt arrives, it fetches first and waits for it (`Models::ready`). After a failed fetch it
  tries again in 15 s, 30 s, 1 min, 2 min, then every 5 min.
- The cache file holds `version`, `base_url`, `fetched_at` and the entries in the
  API's form, with mode 0600, under its own name beside the OpenAI catalog's file.
- `check_key(http, config, key)`: `GET <base_url>/models?limit=1` with the request's
  headers. `Ok` for a 200; a 401 is `Unauthorized { message }`; any other status is
  `Api` with the status and the server's message (such as a 403, or the 400 that asks
  for `anthropic-workspace-id`), else the status's reason; no answer is `Transport`.
  The check goes once, with no retry, because a person waits for it (as the OpenAI
  check does). It is never recorded, and no error holds the key: a server message
  that quotes the key shows `<the key>`.

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
| 500 `api_error`, 504 `timeout_error`, and 502 or 503 from a proxy | `Api` after the last try | yes, with backoff |
| 529 `overloaded_error` | `Overloaded` after the last try | yes, with backoff |
| any other status | `Api` with the server's message | no |
| an `error` event after a 200 | the same class by `error.type` | never |

The attempts of a model call and of each page of the model list go through
`efr_http::RetryPolicy::run` on the injected clock and random source: one attempt
reads the error body and classifies it before the policy decides, so a spend cap is
never sent four times. `efr_http::is_retryable_status` does not decide here. A
`retry-after` header sets the wait; a wait above the policy's longest ends the
attempts at once. A model call that could not connect is sent again, because the
server never saw it; a model call that timed out or broke after it was sent is not (a
`GET` of the list is). A message comes from the error body's `error.message`, else
from the body as text, else from the status, clipped to 1000 characters. A body that
quotes the key, as a proxy at `[anthropic] base_url` can, shows `<the key>` in place
of it, for a model call, a fetch of the list and a key check alike. Each failed
attempt writes one debug line with the status (and, for a model call, the
`request-id`, which is also a field of the request's span).
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

Third-party crates: `async-trait`, `futures` (the event stream), `jiff` (the time of a
fetched catalog), `serde` (the typed body, the blocks of `provider_raw`, the entries of
the catalog and its cache file), `serde_json` (with `raw_value`, for the exact replay
of assistant content), `thiserror` and `tracing`. The tests also use `insta` (the body
snapshots), `rstest`, `pretty_assertions`, `tempfile`, `tokio`, `tracing-subscriber`
(to read the log) and `wiremock` (the fake API).

## Invariant

- The client never sees how a key was stored or entered, and the key never reaches a
  log, an error text, a `Debug` string or a recorded request: it goes in a sensitive
  `Authorization` header, and error messages come from the server's error body, never
  from the request, with each copy of the key in that body replaced.
- `provider_raw` goes back to the API byte for byte as efr wrote it. These are not the
  bytes that the API sent: efr builds the `text`, `thinking` and `tool_use` blocks
  from the stream, in the API's member order, with the tool input as the model wrote
  it and without an empty `text` block. Only a `redacted_thinking` block, or a block
  of a type that efr does not know, holds the API's own text. So every request sends
  the same bytes for an old answer, but whether the thinking binding reads these bytes
  as the ones that it signed is a check on a test key.
- Two following requests of one conversation have the same bytes for `tools`, `system`
  and every earlier message, apart from the cache markers and one case: a user message
  that ended the request before gets more blocks at its end when the next request
  merges a new message of the same role into it. This can occur after a turn that
  ended on its tool results (an interrupt while a tool ran) or on its prompt (a turn
  that failed before an answer). The blocks before stay the same.
- A model call is never sent again after its stream has started.
- No model table and no guessed window: every model fact comes from the API.

## Sources

The request, the stream, the errors and the catalog follow Anthropic's documentation,
read on 2026-10-09, under `https://platform.claude.com/docs/en/`:

- `api/messages/create` (the body, the roles, the merge of adjacent messages of one
  role), `api/overview` and `manage-claude/authentication` (`Authorization: Bearer`,
  `anthropic-version`, `anthropic-workspace-id`), `api/beta-headers` (an unknown beta is
  a 400), `api/versioning` (new event, block and delta types within a version),
  `api/errors` (the error body, the types and their statuses, 529),
  `api/rate-limits` (`retry-after`), `api/models/list` (the pages, `max_input_tokens`,
  `max_tokens`, `capabilities`).
- `build-with-claude/streaming` (the events and their order),
  `build-with-claude/thinking` (adaptive thinking, `display`),
  `build-with-claude/preserved-thinking` (`block_binding` and
  `input_transformations`), `build-with-claude/effort`
  (`output_config.effort`), `build-with-claude/prompt-caching` (the markers, at most
  four, the order of the times to live, the minimum of a cache entry),
  `build-with-claude/context-windows`, `build-with-claude/handling-stop-reasons` (the
  stop reasons, text after tool results), `about-claude/pricing` (the prices of a
  write and a read).
- Claude Code, under `https://code.claude.com/docs/en/`: `prompt-caching` (a stable
  prefix), `tools-reference` (the shape of its `Edit` tool, which every Claude model
  gets as `edit`), `errors` (what it sends again).

The reference implementations (shallow clones of 2026-10-09):

- openai/codex at `c0c230e`: `codex-rs/model-provider-info/src/lib.rs` (which
  statuses it sends again; no 429 at the HTTP layer).
- sst/opencode at `3f393d7`: `packages/opencode/src/provider/transform.ts` (its cache
  markers on the first two system and the last two other messages, which efr does not
  copy), `packages/opencode/src/session/retry.ts` (the statuses that it sends again,
  `retry-after-ms` and `retry-after`), `packages/opencode/src/session/session.ts` (the
  cache counts, without the one-hour part).

## Tests

Run the tests of this crate alone, without the rest of the workspace:

```sh
cargo nextest run -p efr-provider-anthropic
```

`config` tests pin the defaults, the URLs and the refused settings. `usage` tests check
the three-part input sum, the one-hour writes, the thinking count and the cumulative
stream counts. `catalog` tests check an entry as a model, the `lifecycle` filter, the
efforts, the default model, what `apply` does with a list and the models of the config
over the catalog; its `cache` tests the round trip of the file, its form, its 0600 mode
and the files that are not used; its `client` tests, on `wiremock`, the pages, the
headers, the refused key, the broken lists and every result of `check_key`. `failure`
tests are table tests of each answer's class and of each `error` event.
`convert::breakpoints` tests are table tests of the placement: the walkthrough of two
turns with a tool loop, a steer, a summary request and the calls after a compaction,
the anchor of a grown tool loop, a side call, each time to live, the fallbacks of S,
and every short history under every setting against the rules of the API (at most four
markers, in order, no one-hour marker after a five-minute one).

`convert` tests pin the body with insta snapshots (`src/convert/snapshots/`: the
first call, a tool loop, a steer, the head after a compaction, a summary request, no
system prompt) and check each rule of "The request" in table tests, the raw replay
byte for byte among them. `convert::tests::prefix` builds the requests of four turns
from the stream fixtures and checks that each body, without its markers, is a byte
prefix of the next. `sse_events` tests read the hand-written streams in
`fixtures/messages/` (text, a tool call, thinking, omitted thinking, redacted
thinking, an error after the start, `max_tokens`, `refusal`) and cover each stop
reason, each error type, unknown events and broken tool calls.

`messages` tests, on `wiremock` and a clock whose sleeps end at once, check the
headers of a model call, that no `provider_options` key reaches the body, the retries
of a 429, a 5xx and a 529 with their waits, the classes that are sent once, an `error`
event after the stream started, a cut stream, a refused connection, and that no
recorded request, error or log line holds the key.

The tests make no network call, sleep on no real time and need no Zig.

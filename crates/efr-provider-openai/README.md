# efr-provider-openai

## Purpose

The OpenAI Responses API client. `OpenAiProvider` implements `efr_provider::Provider`
over a streaming `POST <base_url>/responses`, or over a WebSocket to the same URL (see
"WebSocket transport" below), on one of two backends:

- the ChatGPT subscription backend, `https://chatgpt.com/backend-api/codex`, with a
  token from the subscription login, the `chatgpt-account-id` header from the token's
  account and the `originator` header (`efr` by default). A request with a
  `prompt_cache_key` also sends it as the `session-id` header, as Codex does with its
  session id: that backend routes a request to its prompt cache by the header. The
  conversation sends its id as the key on every request;
- the public API, `https://api.openai.com/v1`, with an API key.

Modules:

- `config`: `OpenAiConfig` (backend, base URL, originator, the models that the config
  lays over the catalog, retry policy, reasoning mode, default reasoning effort and
  summary, parallel tool calls, the WebSocket switch) and the `Backend`,
  `ReasoningMode` and `WebSocketMode` enums. A request's `provider_options` override the defaults: `reasoning_effort`,
  `reasoning_summary`, `parallel_tool_calls`, `prompt_cache_key`, `service_tier` and
  `text_verbosity`; other keys are ignored.
- `catalog`: the model catalog. The backend lists its models at
  `GET <base_url>/models?client_version=<efr's version>`, as Codex asks for a ChatGPT
  login, with the same token, `originator` and account headers as a model request.
  `CatalogClient::fetch` sends it, with `If-None-Match` and the tag of the list that
  efr has when the same backend sent that list to the same version of efr. A 304 is
  `Fetched::NotModified`; a 401 makes the token source forget its token, and the
  request goes once more. A `Catalog` is one list and its origin (`CatalogOrigin`:
  `Backend`, `Cache` or `Builtin`) with the time of the fetch and the tag. Each entry
  has the members of Codex's `ModelInfo` that efr reads: `slug`, `display_name`,
  `description`, `priority`, `visibility`, `context_window`, `max_context_window`,
  `supported_reasoning_levels`, `default_reasoning_level`, `apply_patch_tool_type`,
  `prefer_websockets`, `minimal_client_version` and `supported_in_api`. Unknown
  members are ignored, a broken window or priority counts as unknown, and an entry
  that cannot be read is left out. `Catalog::models` gives the models on offer, best
  (lowest) priority first: `visibility` must be `list` (or absent), the
  `minimal_client_version` must not be above `CLIENT_VERSION` (efr's own version,
  never another client's), and the API key backend also needs `supported_in_api`.
  A missing window takes the largest one, and the largest one is never below the
  window. `apply_patch_tool_type: "freeform"` gives `ModelInfo::freeform_tools`;
  `prefer_websockets` gives `ModelInfo::prefer_websockets`, which picks the WebSocket
  transport under `WebSocketMode::Auto`. `Catalog::default_model` is the first model on offer.
  `ModelCatalog` holds the current catalog for the provider and the daemon: a reader
  takes it from memory, and `ModelCatalog::apply` makes a fetch current, except a list
  that offers no model to this version of efr (`Applied::Refused`), which keeps the
  current one. `read_cache` and `write_cache` keep a fetched list in a file (JSON:
  `version`, `base_url`, `client_version`, `fetched_at`, `etag` and the entries in
  the backend's form), written in one step with mode 0600; a file of another version
  or another backend is not used. They block; the daemon calls them off its async
  workers. When to fetch is the daemon's choice.
- `models`: the table built into efr, the last fallback of the catalog: the listed
  models of Codex's bundled catalog (`codex-rs/models-manager/models.json`, read at
  c0c230e on 2026-10-08), in priority order, with gpt-6.1-sol first and gpt-5.5 as the
  legacy model. It stands in when no fetch worked yet and no cache is on disk, and for
  the API key backend, whose `/v1/models` says nothing about windows. Every model in
  it takes freeform tools. The lists are hints: a model that is not listed is still
  sent, and an answer that says the model is not served becomes
  `ProviderError::UnknownModel`. Every model outside the catalog gets the function
  form of a freeform tool, which every model with function calls takes.
- `convert`: canonical `Request` and `Message` to the Responses body and `input`
  items, and output items back to their canonical parts. The body follows Codex:
  `stream: true`, `store: false`, the system prompt as `instructions`,
  `tool_choice: "auto"`, function tools with `strict: false`, and for reasoning models
  `reasoning: {effort, summary}` with `include: ["reasoning.encrypted_content"]`. The
  subscription path sends no `max_output_tokens`, which that backend refuses. The API
  path sends the model's own limit from its entry in `[openai] models`, else the
  request's. The provider reads the catalog from memory for each request, with the
  models of the config laid over it, so a new catalog decides the tool form from the
  next request on. A freeform tool goes to a model that takes it as
  `{"type": "custom", name, description, "format": {"type": "grammar", syntax,
  definition}}`, and to any other model as a function tool with its function form. A
  canonical freeform call goes back to a model that takes freeform tools as a
  `custom_tool_call` item (`call_id`, `name`, `input`), and the result of a call
  that was a `custom_tool_call` (in the raw items or the canonical content) as a
  `custom_tool_call_output`. To any other model, such as after `,model o3` in a
  conversation that started on a model of the catalog, the canonical freeform call
  goes as a `function_call` with the text in `arguments` as `{"input": text}`, and
  its result as a `function_call_output`, so each tool has one shape in the request.
- `sse_events`: `EventMapper`, a pure state machine from Responses events to
  `ProviderEvent`s (text, reasoning, tool call start, deltas and end, usage, done).
  A `custom_tool_call` item is a freeform call: `response.custom_tool_call_input.delta`
  events grow its text (found by item id, output index or call id), and its finished
  item holds the whole text as `input`.
  Unknown event types and output items without a canonical form become
  `ProviderEvent::Raw` and are logged at debug, never dropped silently.
- `responses`: `OpenAiProvider`, the HTTP exchange, the choice of the transport and
  the event stream.
- `websocket`: `Sockets`, the WebSocket connection of each conversation; its
  `connection` module holds the task that owns one connection, and its
  `continuation` module decides when a call sends only its new input.
- `timing`: the `provider_accepted` and `provider_first_event` debug lines.
- `error`: `OpenAiError`, for settings refused when the config is built (a base URL
  that is not `http` or `https`, an originator that cannot be a header value). A
  request fails with `efr_provider::ProviderError`, the error every provider shares.

Native passthrough: the `Done` event carries every `response.output_item.done` item
of the response, verbatim and in order (the encrypted `reasoning` item, the
`message` items and the exact `function_call` and `custom_tool_call` items), as the
assistant message's
`provider_raw`. The next request to the same provider sends those items back
unchanged in place of the message's canonical content. An assistant message without
usable `provider_raw` is rebuilt from its canonical content; its reasoning text is
dropped, because a reasoning item without its encrypted content is refused when
`store` is false.

Failures:

- a 401 makes the provider call `TokenSource::invalidate`, fetch a new token and send
  the request once more; a second 401 is `ProviderError::Unauthorized`;
- other retries follow the config's `efr_http::RetryPolicy`, which sends a `POST`
  again only when the server certainly did not act on it (no connection, or 408, 429
  or 503), so a model call never runs twice;
- a 429 is `RateLimited`, with the wait from `Retry-After`, from the subscription's
  `resets_at` (on the injected clock), or from the message; a 429 for a used-up quota
  or a plan without access is an `Api` error, because waiting does not help;
- an error answer, as a response or as an event, goes through `ProviderError::api`:
  the code `context_length_exceeded` and HTTP 413 are `ContextOverflow`, which no
  retry sends again;
- `response.failed` and `error` events end the stream with the provider's error;
  `response.incomplete` ends it as `MaxTokens` or `ContentFilter`, and a tool call
  cut off by the limit never ends, so a truncated command cannot run;
- a stream that ends before `response.completed` ends with `ProviderError::Incomplete`.

## WebSocket transport

A model call can go out as a `response.create` message on a WebSocket instead of a
`POST`. The server answers with the same events as the HTTP stream, one JSON event per
text message, and `EventMapper` turns them into the same canonical events, so drafts,
tool calls, freeform calls and usage are the same on both transports. The transport
cuts the time to the first token: the connection stays open, so a call skips the TCP
and TLS setup, and on a connection that holds the previous answer a call sends only
its new input items.

Which calls use it:

- `[openai] websocket` in `config.toml` (`OpenAiConfig::with_websocket`): `auto`, the
  default, follows the model's `prefer_websockets` in the current catalog (from the
  backend, the cache or the built-in table; a model that the catalog does not list
  uses HTTP); `on` uses a WebSocket for every model; `off` never uses one. The key
  needs a restart. A new catalog changes the choice from the next call.
- A call needs a `prompt_cache_key`, which the conversation sets to its id. A call
  without one goes over HTTP.

The connections:

- One connection for each conversation, by its `prompt_cache_key`. It serves one call
  at a time; a call that finds it busy goes over HTTP.
- It stays open between the calls of a turn and between turns. It closes after 10
  minutes without a call, when the server closes it, after a failure, and it takes no
  call after 55 minutes, before the server's limit of 60 minutes.
- The handshake is an HTTP/1.1 `GET <base_url>/responses` through `efr_http`'s client
  (the same rustls stack and `User-Agent: efr/<version>`), with the headers of the
  HTTP path (`Authorization`, and on the subscription `chatgpt-account-id`,
  `originator` and `session-id`) and `OpenAI-Beta: responses_websockets=2026-02-06`.
  It may take 15 seconds.

Incremental input: with `store: false` the server keeps no answer, except the last
answer of an open connection. A call on the same connection sends
`previous_response_id` and only the items after that answer when the previous answer
completed (`response.completed`), every field of the request but `input` is the same,
and the new input starts with the previous input and the previous answer's output
items, item for item. Any other call sends the whole input: a new connection, an
interrupted or failed answer, a changed model, effort, instructions or tool list, and a
history that a compaction or a rebuild changed. The HTTP body never carries
`previous_response_id`.

Interrupt: when the conversation drops the stream of a running answer, the connection
sends `{"type": "response.interrupt", "response_id", "mode": "discard_partial_items"}`
and reads until the answer ends (`response.incomplete`), at most 10 seconds, so the
connection is clean for the next call. It closes when the answer does not end in time.

Fallback to HTTP: a call that the socket cannot serve goes over HTTP, so a failed
WebSocket never fails a call that HTTP could serve:

- the connection cannot be opened, or the server refuses the upgrade;
- the server does not take the call: it sends an `error` event, or closes the
  connection, before the first event of an answer (`response.*`). The server has then
  started no answer, so the HTTP request is the first and only model call.

After a failure that says that WebSockets do not work now (a refused upgrade, a failed
handshake, an `error` event other than a refused token, a rate limit,
`previous_response_not_found` or `websocket_connection_limit_reached`), every call goes
over HTTP for 5 minutes. A closed stale connection does not pause them.

Once the server has started an answer, a failure is the call's failure, as on the HTTP
path: a close or a lost connection is `ProviderError::Incomplete` or a transport error,
and the server's `error` and `response.failed` events map as they do over HTTP. The
call is not sent again, because the model may have run.

Timeouts: 15 seconds for the handshake, 300 seconds of silence inside an answer (the
HTTP client's read timeout), 30 seconds to send a message and 10 seconds for an
interrupted answer to end. They run on the injected `Clock`.

The debug lines: each call writes `phase=provider_accepted` when the server's first
event arrives and `phase=provider_first_event` when the first canonical event is
ready, with `transport=http|websocket`, `connection=new|reused` (`pool` for HTTP) and
`input=full|incremental`. A new connection also writes `phase=provider_connect`.
`docs/sandbox.md` tells how to read them.

What efr copies from Codex (`codex-rs/codex-api/src/endpoint/responses_websocket.rs`
and `codex-rs/core/src/client.rs`, read on 2026-10-08):

- the `response.create` message: the fields of the HTTP body, routing fields first,
  `type` and, for a continuation, `previous_response_id`
  (`ResponseCreateWsRequest`);
- the `OpenAI-Beta` header value and the session header of the handshake
  (`build_websocket_headers`);
- one event per text message, parsed by the same event parser as the SSE stream; a
  binary message, a close or the end of the connection before `response.completed`
  fail the stream (`run_websocket_response_stream`);
- `response.interrupt` with `discard_partial_items`, sent only once the answer's id is
  known (`run_websocket_response_stream`);
- the conditions for incremental input (`get_incremental_items`,
  `responses_request_properties_match`);
- the 15 second connect timeout and the 300 second idle timeout of a stream
  (`DEFAULT_WEBSOCKET_CONNECT_TIMEOUT_MS`, `DEFAULT_STREAM_IDLE_TIMEOUT_MS`);
- the two routine error codes, `websocket_connection_limit_reached` and
  `previous_response_not_found`, which Codex answers with a new connection and the
  whole input;
- the fallback to HTTP on a refused upgrade, and that it lasts beyond the one call
  (`force_http_fallback`).

What efr does not copy:

- Codex reads `supports_websockets` from the provider and keeps `prefer_websockets`
  only in its bundled catalog; efr chooses per model from that catalog value.
- Codex retries a stream that failed after the server took it, and switches to HTTP
  only after its retries; efr never sends a call again once an answer has started, and
  goes over HTTP at once when the server did not take a call. Codex's switch lasts for
  the session; efr's lasts 5 minutes.
- Codex keeps a connection for one turn and caches it between turns; efr keeps one
  for each conversation and closes it after 10 minutes without a call.
- The prewarm request (`generate: false`), the `x-codex-turn-state` sticky routing,
  the `client_metadata`, the `codex.*` events, safety buffering and
  `permessage-deflate` compression. Events that the parser does not know become
  `ProviderEvent::Raw`, as on the HTTP path.
- A WebSocket exchange is not recorded through `efr_http`'s `Recorder`.

The usage of a finished response gives `input_tokens` (cached ones included),
`output_tokens` (reasoning included), `input_tokens_details.cached_tokens` and
`output_tokens_details.reasoning_tokens`; a missing or `null` part counts as zero. The
tests check them on the recorded streams in `fixtures/responses/`, and the overflow on
`context_length_exceeded.sse`, a 400 with that code and a 413.

## Tier

Tier 2.

## Allowed dependencies

`efr-provider`, `efr-http`, `efr-protocol` and `efr-stdx`; `xtask/src/deps.rs` holds
the allowlist. The crate uses `efr-provider` (the trait and the canonical types),
`efr-http` (the client, the retry policy, the SSE parser and the WebSocket client) and
`efr-stdx` (the `Clock`). `efr-protocol` is allowed but not needed: nothing here touches the wire
types of the daemon's socket.

It must never depend on `efr-oauth-openai`: tokens arrive through
`efr_provider::TokenSource` as an `AccessToken` with its account id, so this crate
never sees a refresh token and never parses a JWT. That edge is forbidden in
`xtask/src/deps.rs`.

Third-party crates: `async-trait`, `futures`, `jiff` (the time of the clock and of a
fetched catalog), `serde`, `serde_json`, `thiserror`, `tokio` (the task and the
channels of a WebSocket connection) and `tracing`. The tests also use `fastwebsockets`
for the fake server.

## Invariant

- The Responses client never knows how a token was obtained, and never sees a refresh
  token.
- `provider_raw` goes back to the provider exactly as it came: the items are sent
  verbatim, never edited, truncated or re-ordered.
- No secret reaches a log or a transcript: the token goes in a sensitive
  `Authorization` header, requests are recorded only through `efr_http`'s redacting
  recorder, and error messages come from the server's error body, never from the
  request.
- A model call is never sent twice after the server may have acted on it. Over a
  WebSocket, a call goes over HTTP only when the server has not started an answer for
  it.
- `previous_response_id` goes only on the WebSocket connection that holds that answer,
  and only when the rest of the request is unchanged.
- efr names itself honestly: a fetch of the catalog sends efr's own version as
  `client_version` and the configured `originator` (`efr` by default), never the
  values of another client.
- A request never waits for a fetch of the catalog: it reads the current catalog from
  memory.

## Sources

The request shape, the headers and the event handling follow these files of the
reference implementations (shallow clones of 2026-10-04):

- openai/codex at `afb436d`:
  `codex-rs/core/src/client.rs` (`build_responses_request`: `store: false`, the
  `include` list, `tool_choice`, `parallel_tool_calls`, `instructions`; the
  `originator` header),
  `codex-rs/codex-api/src/common.rs` (`ResponsesApiRequest` and its field order),
  `codex-rs/codex-api/src/endpoint/responses.rs` (`POST /responses` with
  `Accept: text/event-stream`),
  `codex-rs/codex-api/src/sse/responses.rs` (`process_responses_event`: event names,
  `response.failed`, the `response.incomplete` reasons),
  `codex-rs/codex-api/src/api_bridge.rs` (429 bodies: `usage_limit_reached` with
  `resets_at`, the quota codes),
  `codex-rs/model-provider-info/src/lib.rs` (`CHATGPT_CODEX_BASE_URL` and the API base
  URL),
  `codex-rs/model-provider/src/bearer_auth_provider.rs` (the `ChatGPT-Account-ID`
  header),
  `codex-rs/login/src/auth/default_client.rs` (`originator`),
  `codex-rs/protocol/src/models.rs` (`ResponseItem` and `ContentItem` shapes),
  `codex-rs/tools/src/responses_api.rs` (function tools with `strict`, and
  `FreeformTool`, the `custom` tool with its grammar `format`),
  `codex-rs/core/src/tools/handlers/apply_patch_spec.rs` (the freeform `apply_patch`
  tool), `codex-rs/protocol/src/openai_models.rs` (`ApplyPatchToolType`; read at
  c0c230e on 2026-10-08), `codex-rs/protocol/src/models.rs` (`CustomToolCall` and
  `CustomToolCallOutput`),
  `codex-rs/models-manager/models.json` (the built-in table: model ids, priorities,
  windows, the reasoning levels and the default level; read again at c0c230e on
  2026-10-08),
  `codex-rs/model-provider/src/models_endpoint.rs` (the `/models` endpoint that
  Codex asks for a ChatGPT login),
  `codex-rs/codex-api/src/endpoint/models.rs` (`GET <base>/models?client_version=`
  and the `ETag` of the answer),
  `codex-rs/models-manager/src/manager.rs` and `cache.rs` (the cache with its tag,
  the fallback to the bundled list, the sort by `priority` and the default: the first
  model in the picker), `codex-rs/protocol/src/openai_models.rs` (`ModelInfo`,
  `ModelVisibility`, `max_context_window` for config overrides),
  `codex-rs/core/tests/common/responses.rs` (stream shapes for the fixtures).
- sst/opencode at `907b3bc`:
  `packages/opencode/src/plugin/openai/codex.ts` (the endpoint, `ChatGPT-Account-Id`,
  `originator`, no `maxOutputTokens` on the subscription, the allowed models and their
  output limits),
  `packages/opencode/src/provider/transform.ts` (`store: false`, encrypted reasoning,
  `reasoningSummary: "auto"`),
  `packages/opencode/src/session/llm/request.ts` (the system prompt as `instructions`
  on the subscription path).
- block/goose at `591edd4`:
  `crates/goose/src/providers/chatgpt_codex.rs` (`CODEX_API_ENDPOINT`,
  `build_input_items`, `create_codex_request`, the `chatgpt-account-id` header, the
  default model, the `Error:` prefix on a failed tool result).

Which model ids and `originator` values the subscription backend accepts from a client
other than Codex is open question 7 of the structure document; both are configurable.

## Tests

Run the tests of this crate alone, without the rest of the workspace:

```sh
cargo nextest run -p efr-provider-openai
```

`convert` tests pin the request body and the item shapes, including the verbatim
passthrough of `provider_raw`, the fallbacks and the two forms of a freeform tool by
model. `sse_events` tests run the mapper over the streams in `fixtures/responses/`
(plain text, a tool call, a freeform `apply_patch` call, reasoning with encrypted
content, an error event, a failed response with a rate limit, a response stopped at
the output limit) and over small inline streams. `responses` tests drive
`OpenAiProvider` against a `wiremock` server on the loopback interface: the headers of
both backends, the reasoning round trip across two requests, the 401 refresh (with
`fixtures/responses/unauthorized.json`), retries on the injected clock, the error
mappings, the redacted transcript record, and a new catalog that changes the tool form
from the next request. `catalog` tests read `fixtures/catalog/models.json` (a list,
a hidden model, a model too new for efr, a broken window and two broken entries): the
models on offer and their order, the windows, the default, the version compare, the
tag and what `apply` does with each answer. `catalog::cache` tests write and read the
cache file in a temporary directory, and `catalog::client` tests fetch from `wiremock`:
the query and the headers, a 304 for the tag, the 401 refresh and the failures. `websocket` tests drive it against a fake
server (`testing/responses_server.rs`) that speaks the WebSocket protocol as Codex
expects it and also answers `POST /responses`: every fixture streams the same events
over both transports, the handshake headers and the `response.create` body, the reuse
of a connection with only the new items, the whole input after a changed setting or a
compaction, the interrupt, the fallback to HTTP on a refused upgrade, a close and an
`error` event before the answer, the failure of a close after the answer started, a
lost previous answer, the idle close, and the switch with the catalog's
`prefer_websockets`. `continuation` tests check when a call may send only its new
items. The fixtures are hand-written in the Responses wire format, since the tests
make no real network or model calls. Nothing
sleeps on real time and nothing needs Zig.

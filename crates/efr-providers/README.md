# efr-providers

## Purpose

The model providers of efr. Each one is a module that implements
`efr_provider::Provider` for one API:

- `openai` (`efr_providers::openai`): the OpenAI Responses API client, on the ChatGPT
  subscription backend or the public API with an API key (see "The openai module").
- `anthropic` (`efr_providers::anthropic`): the Anthropic Messages API client with an
  API key, provider id `anthropic-api` (see "The anthropic module").

The two modules share no code, and each one keeps its own error type
(`OpenAiError`, `AnthropicError`). They were two crates until ADR 0014 folded them
into this one: they have the same dependencies, and only `efr-daemon` uses them.

## The openai module

The OpenAI Responses API client. `OpenAiProvider` implements `efr_provider::Provider`
over a streaming `POST <base_url>/responses`, or over a WebSocket to the same URL (see
"WebSocket transport" below), on one of two backends:

- the ChatGPT subscription backend, `https://chatgpt.com/backend-api/codex`, with a
  token from the subscription login, the `chatgpt-account-id` header from the token's
  account and the `originator` header (`efr` by default). A request with a
  `prompt_cache_key` also sends it as the `session-id` header, as Codex does with its
  session id: that backend routes a request to its prompt cache by the header. The
  conversation sends its id as the key on every request;
- the public API, `https://api.openai.com/v1`, with an API key. A request names the
  `OpenAI-Organization` and `OpenAI-Project` headers when the config gives them
  (`[openai] organization` and `project`), on HTTP, on the WebSocket handshake, on
  the catalog fetch and on the key check alike.

Modules:

- `config`: `OpenAiConfig` (backend, base URL, originator, organization and project of
  the API path, the models that the config
  lays over the catalog, retry policy, reasoning mode, default reasoning effort and
  summary, parallel tool calls, the WebSocket switch) and the `Backend`,
  `ReasoningMode` and `WebSocketMode` enums. A request's `effort` overrides the
  default reasoning effort, and its `provider_options` override the other defaults:
  `reasoning_summary`, `parallel_tool_calls`, `prompt_cache_key`, `service_tier` and
  `text_verbosity`; other keys are ignored.
- `catalog`: the model catalog. The backend lists its models at
  `GET <base_url>/models?client_version=<efr's version>`, as Codex asks for a ChatGPT
  login, with the same token, `originator` and account headers as a model request.
  `CatalogClient::fetch` sends it, with `If-None-Match` and the tag of the list that
  efr has when the same backend sent that list to the same version of efr. A 304 is
  `Fetched::NotModified`; a 401 makes the token source forget its token, and the
  request goes once more. The source forgets the token only when it still holds the
  token that the backend refused, so a token that a model call refreshed meanwhile
  stays. When the second request gets a 401 too, a 401 forces no refresh until a fetch
  works: the backend then refuses the catalog to a token that it takes for model
  calls, and a refresh every few minutes would only use up refresh tokens. A `Catalog` is one list and its origin (`CatalogOrigin`:
  `Backend`, `Cache` or `Builtin`) with the time of the fetch and the tag. Each entry
  has the members of Codex's `ModelInfo` that efr reads: `slug`, `display_name`,
  `description`, `priority`, `visibility`, `context_window`, `max_context_window`,
  `supported_reasoning_levels`, `default_reasoning_level`, `apply_patch_tool_type`,
  `prefer_websockets` and `supported_in_api`. Unknown
  members are ignored, a broken window or priority counts as unknown, and an entry
  that cannot be read is left out. `Catalog::models` gives the models on offer, best
  (lowest) priority first: `visibility` must be `list` (or absent), the
  `apply_patch_tool_type` must be absent, `freeform` or `function`, and the API key
  backend also needs `supported_in_api`. efr ignores `minimal_client_version`,
  because it is a version of Codex.
  A missing window takes the largest one, and the largest one is never below the
  window. `apply_patch_tool_type: "freeform"` gives `ModelInfo::freeform_tools`;
  `prefer_websockets` gives `ModelInfo::prefer_websockets`, which picks the WebSocket
  transport under `WebSocketMode::Auto`. `Catalog::default_model` is the first model on offer.
  `ModelCatalog` holds the current catalog for the provider and the daemon: a reader
  takes it from memory, and `ModelCatalog::apply` makes a fetch current, except a list
  that offers no model that efr can use (`Applied::Refused`), which keeps the
  current one. The API key backend answers `GET <base_url>/models` (no
  `client_version`) with ids only, `{"object": "list", "data": [{"id": ...}]}`. Its
  catalog is the built-in table cut down to the ids that the key lists
  (`Catalog::from_api`): a model that the key cannot use is not offered, and an id
  that the table does not know is not offered either, because efr does not know its
  tools; `[openai] models` can add it. A list that keeps no model of the table is
  refused like an empty one. `check_key` sends the same request once with a key
  that a login has not stored yet: 200 is `Ok`, 401 `Unauthorized` and any other
  status an `Api` error, each with the server's message, and no answer a
  `Transport` error. A server message that quotes the key shows `<the key>` in its
  place. The request runs no model. `read_cache` and `write_cache` keep a fetched list in a file (JSON:
  `version`, `base_url`, `client_version`, `fetched_at`, `etag` and the entries in
  the backend's form), written in one step with mode 0600; a file of another version
  or another backend is not used. They block; the daemon calls them off its async
  workers. When to fetch is the daemon's choice.
- `models`: the table built into efr, the last fallback of the catalog: the listed
  models of Codex's bundled catalog (`codex-rs/models-manager/models.json`, read at
  c0c230e on 2026-10-08), in priority order, with gpt-6.1-sol first and gpt-5.5 as the
  legacy model. `CLASSIFIER_MODEL` (`gpt-6-luna`) names the small model of the table
  that efrd asks first when it reviews an action of the `auto` sandbox, on both
  backends; the catalog decides at run time whether the backend serves it. The table
  stands in when no fetch worked yet and no cache is on disk. The API
  key backend takes its windows, efforts and tool forms from it, because the API's
  `/v1/models` gives only ids. On the API, the efforts are those of the API's model
  pages: the subscription's list without `ultra`, which no page lists, and `medium`
  as the default of every model. efr leaves out `none`, which some pages list,
  because it asks every request of these models to reason. Every model in
  it takes freeform tools. The lists are hints: a model that is not listed is still
  sent, and an answer that says the model is not served becomes
  `ProviderError::UnknownModel`. Every model outside the catalog gets the function
  form of a freeform tool, which every model with function calls takes.
- `convert`: canonical `Request` and `Message` to the Responses body and `input`
  items, and output items back to their canonical parts. The body follows Codex:
  `stream: true`, `store: false`, the system prompt as `instructions`,
  `tool_choice: "auto"`, function tools with `strict: false`, and for reasoning models
  `reasoning: {effort, summary}` with `include: ["reasoning.encrypted_content"]`. A
  request without tools, such as a review by efrd's classifier, sends neither
  `tool_choice` nor `parallel_tool_calls`: the API's defaults fit it, and a member
  that is not sent cannot be refused. `Request::output` becomes `text.format`,
  `{"type": "json_schema", name, schema, "strict": true}`, beside `verbosity` when
  the request names one; strict mode needs every property in `required` and
  `additionalProperties: false`, so the caller's schema gives both. The
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
  that is not `http` or `https`, an originator, organization or project that cannot
  be a header value). A
  request fails with `efr_provider::ProviderError`, the error every provider shares.

Native passthrough: the `Done` event carries every `response.output_item.done` item
of the response, verbatim and in order (the encrypted `reasoning` item, the
`message` items and the exact `function_call` and `custom_tool_call` items), as the
assistant message's
`provider_raw`. The next request to the same provider sends those items back
unchanged in place of the message's canonical content. An assistant message without
usable `provider_raw` is rebuilt from its canonical content; its reasoning text is
dropped, because a reasoning item without its encrypted content is refused when
`store` is false. Raw items with a `function_call` or `custom_tool_call` of a tool
that the request does not offer are not usable: the canonical content shows that
call as text (`efr_provider::unoffered_call_text`).

Failures:

- a 401 makes the provider call `TokenSource::invalidate`, fetch a new token and send
  the request once more; a second 401 is `ProviderError::Unauthorized` with the
  message of the server's body. A token source that is not `refreshable`, such as an
  API key, gets no second try: its first 401 is `Unauthorized`. The catalog fetch
  follows the same rule;
- other retries follow the config's `efr_http::RetryPolicy`, which sends a `POST`
  again only when the server certainly did not act on it (no connection, or 408, 429
  or 503), so a model call never runs twice. The provider runs the policy itself
  (`RetryPolicy::run`, with the injected clock and generator) and reads the body of a
  429 before the policy decides: a 429 for a used-up quota or a plan without access
  (`insufficient_quota`, `usage_not_included`, `credit_balance_exhausted` and the
  spend and usage limit codes) is never sent again;
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

### WebSocket transport

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
  at a time; a call that finds it busy goes over HTTP. When two calls of one
  conversation both find no connection and both open one, the first to open stays the
  conversation's connection, and the other serves only its own call. Each answer
  holds its connection, so no answer is cut when the map holds another one.
- It stays open between the calls of a turn and between turns. It closes after 10
  minutes without a call, when the server closes it, after a failure, and it takes no
  call after 55 minutes, before the server's limit of 60 minutes.
- A call on a connection that had no call for 30 seconds first sends a ping. A
  connection can die without a close, such as during a sleep of the computer, a change
  of network or a NAT that forgets the flow. When the pong does not come in 5 seconds,
  the connection closes and the call goes over HTTP, because its request did not go
  out. Without the ping, such a call would wait for the read timeout of 300 seconds.
  The calls of one turn follow each other within seconds and skip the ping.
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
`previous_response_id`. In practice, the conversation puts its live state into the
newest user message of each turn's first request and keeps it out of the history, so
the first call of a turn sends the whole input and the later calls of the turn send
only their new items. The connection does not keep the items of the previous call: it
keeps the length and a 128-bit hash of the JSON text of each item, and it compares the
new input with them. Thus it compares the bytes that went on the wire. An item that
cannot be encoded sends the whole input.

Interrupt: when the conversation drops the stream of a running answer, the connection
sends `{"type": "response.interrupt", "response_id", "mode": "discard_partial_items"}`
and reads until the answer ends (`response.incomplete`), at most 10 seconds, so the
connection is clean for the next call. It closes when the answer does not end in time.

Fallback to HTTP: a call goes over HTTP when the server certainly did not act on its
request, so the HTTP request is the first and only model call:

- the connection cannot be opened, or the server refuses the upgrade;
- a connection that had no call for 30 seconds does not answer its ping;
- the request could not be written whole;
- the server does not take the call: it closes the connection with a close frame
  before the first event of an answer (`response.*`), or it sends an `error` event
  without an error status, with a 401, a 408 or a 5xx, or with the code
  `previous_response_not_found` or `websocket_connection_limit_reached`.

An `error` event before the answer with any other status from 400 to 499, such as a
429 or a context overflow, is the server's answer to the request itself, as Codex
reads it. The call fails with the same error as an HTTP answer with that status and
body (the event's `headers` member gives the `Retry-After`), and it does not go out a
second time over HTTP.

After a failure that says that WebSockets do not work now (a refused upgrade, a failed
handshake, an `error` event without an error status or with a 408 or a 5xx, a binary
message or a message that is not JSON), every call goes over HTTP for 5 minutes. A
refused token pauses them only when the call before was refused for its token too, so
the HTTP path can refresh the token first. A closed stale connection, a dead
connection that does not answer its ping, a routine code and an error status that
answers the request do not pause them.

Once the request has gone out, a failure that does not show that the server refused
it is the call's failure, also before the first event of an answer: a broken
connection, a connection that ends without a close frame, no event for 300 seconds, or
a request that could not be sent in 30 seconds. The server may have taken the request,
so the call is not sent again, as on the HTTP path, where `efr_http::RetryPolicy` does
not send a `POST` again after a broken connection or a timeout. Once the server has
started an answer, every failure is the call's failure: a close or a lost connection is
`ProviderError::Incomplete` or a transport error, and the server's `error` and
`response.failed` events map as they do over HTTP. The call is not sent again, because
the model may have run.

Timeouts: 15 seconds for the handshake, 5 seconds for the pong of the ping before a
call after 30 quiet seconds, 300 seconds of silence inside an answer (the HTTP
client's read timeout), 30 seconds to send a message and 10 seconds for an interrupted
answer to end. They run on the injected `Clock`.

The debug lines: each call writes `phase=provider_accepted` when the server's first
event arrives and `phase=provider_first_event` when the first canonical event is
ready, with `transport=http|websocket`, `connection=new|reused` (`pool` for HTTP) and
`input=full|incremental`. A new connection also writes `phase=provider_connect`.
`docs/sandbox.md` tells how to read them. An answer that ends in an error writes one
`info` line, `the model call failed`, with the error, and over HTTP with the
`x-request-id` as `request_id`.

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
- an `error` event with an HTTP error status as that HTTP error answer
  (`map_wrapped_websocket_error_event`);
- the fallback to HTTP on a refused upgrade, and that it lasts beyond the one call
  (`force_http_fallback`).

What efr does not copy:

- Codex reads `supports_websockets` from the provider and keeps `prefer_websockets`
  only in its catalog; efr chooses per model from the `prefer_websockets` of its own
  current catalog (fetched, cached or built in).
- Codex retries a stream that failed after the server took it, and switches to HTTP
  only after its retries; efr never sends a call again once an answer has started, and
  goes over HTTP at once when the server did not take a call. Codex's switch lasts for
  the session; efr's lasts 5 minutes.
- Codex keeps a connection for one turn and caches it between turns; efr keeps one
  for each conversation and closes it after 10 minutes without a call. Codex sends no
  ping; efr pings a connection that had no call for 30 seconds before it uses it.
- The prewarm request (`generate: false`), the `x-codex-turn-state` sticky routing,
  the `client_metadata`, the `codex.*` events, safety buffering and
  `permessage-deflate` compression. Events that the parser does not know become
  `ProviderEvent::Raw`, as on the HTTP path.
- A WebSocket exchange is not recorded through `efr_http`'s `Recorder`.

The usage of a finished response gives `input_tokens` (cached and written ones
included), `output_tokens` (reasoning included), `input_tokens_details.cached_tokens`,
`input_tokens_details.cache_write_tokens` and `output_tokens_details.reasoning_tokens`;
a missing or `null` part counts as zero. OpenAI has no cache time to live to choose, so
`cache_write_1h_tokens` is always zero. The tests check them on the recorded streams in
`fixtures/openai/responses/` (`tool_call.sse` has a cache write), and the overflow on
`context_length_exceeded.sse`, a 400 with that code and a 413.

## The anthropic module

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
  (`claude-opus-5-5`), `DEFAULT_EFFORT` (`medium`) and `CLASSIFIER_MODEL`
  (`claude-haiku-5-5`), the small model that efrd asks first when it reviews an action
  of the `auto` sandbox. Each names an id, never a window or a limit; the API's list
  decides at run time whether the key may use the model. There is no table of models.
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
  form of the module `openai`, with `transport=http`.
- `error`: `AnthropicError`, for settings refused when the config is built (a base URL
  that is not `http` or `https`, a workspace id that cannot be a header value) and for
  the cache file. A request fails with `efr_provider::ProviderError`.
- `testing` (tests only): a clock whose sleeps end at once, a fixed random source, a
  token source of one test key, model list entries and pages, server-sent events, a
  log that a test reads and the loader of the stream fixtures in `fixtures/anthropic/messages/`.
  The fake API is `wiremock`.

### The request

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
  `output_config: {"effort": ..., "format": ...}`, each member left out when nothing
  gives it and the whole member when neither is there; `messages`. Never
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
- Output schema: `Request::output` becomes `output_config.format`, `{"type":
  "json_schema", "schema": ...}`, so the answer is one JSON object that the schema
  describes, such as the verdict of efrd's classifier. The schema's name stays with
  efr: the API takes none. The API takes no length keywords (`maxLength`) and needs
  `additionalProperties: false` on every object, so the caller sends a schema that
  follows these rules and checks lengths itself. A refusal can still give an answer
  that does not fit the schema.
- The provider reads no key of `Request::provider_options`: the conversation sends
  OpenAI's `prompt_cache_key` to every provider, and an unknown body member is a 400.
- An assistant message that this provider wrote goes back as the exact JSON text of
  its content (`provider_raw`, a JSON string, written through
  `serde_json::value::RawValue`), thinking blocks included. The conversation drops
  `provider_raw` when the model changes. An empty `text` block in a `provider_raw` is
  dropped, and a `tool_use` of a tool that the request does not offer goes as a text
  block with `efr_provider::unoffered_call_text`, the text that the conversation put
  in the canonical content; the other blocks then go back one by one, each exact. So
  the last assistant message of a tool loop keeps its thinking block when the model
  also called a tool that the request does not offer. A user message right after an
  assistant message with tool calls opens no turn for the cache markers, also when it
  holds their results as text. Any other
  assistant message, or a `provider_raw` that is not such a text, is built from its
  text and tool calls, and its reasoning is dropped; a tool call input that is not an
  object goes as `{"input": <value>}`.
- Adjacent assistant messages merge, and the blocks of merged raw messages stay exact.
  Adjacent user messages stay apart, such as a new prompt after a turn that ended on
  its tool results (an interrupt while a tool ran) or on its prompt (a model call that
  failed before an answer), or a steer after tool results: the API joins adjacent
  messages of one role itself, and the user message that ended the request before
  keeps its bytes. `tool_result` blocks come first in a user message, and a result
  with an empty output has no `content`. Empty text blocks are dropped, and so is a message that
  becomes empty. A `tool_use` that the next message does not answer gets a failed
  result with one fixed text. A tool id outside `[a-zA-Z0-9_-]` gets `_` for each
  other character.
- Edit tool: every catalog model has `ModelInfo::edit_tool` `EditTool::Replace`, so
  the request offers `edit` (`path`, `old_string`, `new_string`, `replace_all`) and not
  `apply_patch`.

### Prompt cache

The markers are a pure function, `convert::breakpoints::place_breakpoints`:

```rust
pub(crate) fn place_breakpoints(layout: &Layout<'_>, ttl: CacheTtl) -> Vec<Breakpoint>;

pub(crate) struct Layout<'a> {
    pub(crate) system: bool,          // the body has a system block
    pub(crate) tools: bool,           // the body has at least one tool
    pub(crate) messages: &'a [Shape], // the body's messages
    pub(crate) side_call: bool,       // Request::side_call
    pub(crate) one_shot: bool,        // Request::one_shot
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

- Places: the conversion never merges user messages, and the API joins a run of
  adjacent user messages into one. A run is one place, at its last message, and
  markers sit only there. An inner message of a run can have ended the request of a
  call that failed, such as the tool results before a new prompt; the API's lookback
  from the markers after it reads the entry of that call.
- At most four markers, in the order of the prefix: S (the system block, else the last
  tool), A (the last anchor before the tail), P (the previous tail: the last place
  before the tail, where the call before put its T) and T (the tail, the last
  message). Markers sit only on the system block, a tool or the last block of a user
  message, never on a `thinking` block. No top-level automatic marker.
- Anchors: every place ends the request of one call. That call marks an anchor at it
  when a message of its run opens a turn, or when the messages after the last anchor,
  the run included, hold more than `ANCHOR_STEP_TOKENS` (20,000). A side call never
  marks one. Because the history is append-only, every later request finds the same
  anchors, also after a restart.
- State: the placement needs the place of the last anchor and the tokens after it. No
  one keeps these numbers. The placement computes them again from the messages on every
  call, so the state is the history itself, which `efr-conversation` keeps and sends
  whole. The provider keeps nothing between calls.
- Every setting puts one time on every marker: `CacheTtl::FiveMinutes` five minutes,
  `CacheTtl::OneHour` and `CacheTtl::Auto` one hour. `auto` was planned as one-hour
  anchors over five-minute tool loops. A check on the API (2026-10-09) showed that a
  one-hour entry cannot build on a five-minute one: a request with one-hour markers
  after a five-minute entry reads nothing and writes its whole prefix again at the
  one-hour price. A one-hour marker alone over a five-minute entry does read it. So
  each anchor would write the tool loop before it a second time, and `auto` is one
  hour on every marker. The anchors still give each request one more place to read.
- When two places are one message, it gets one marker.
- A one-shot request (`Request::one_shot`), such as a review by efrd's classifier, gets
  only S, for five minutes under every setting, and no A, P or T: no later request
  reads its messages, and the next review usually comes within minutes. Its debug line
  says `markers=S5m`.
- A new prompt is always a user message of its own, also after a turn that ended on
  its tool results or on its prompt, so the first call of every turn marks an anchor,
  at the new prompt. A steer after tool results is a message of its own too, so the
  call that sends it marks an anchor: it holds a person's words, and a pause can
  follow it.

Kept stable for a whole conversation: the tool list and its order, the system prompt,
the thinking mode and display, and the effort. A change of the tools invalidates every
level; a change of the effort invalidates the messages. Prices: a five-minute write
costs 1.25 times the input price, a one-hour write 2 times, a read 0.1 times or less.
Each request writes one debug line with the setting and its markers in order, the
letter and the time of each place, such as `cache_ttl=auto markers=S1h,A1h,P1h,T1h`
(`convert::breakpoints::summary`). The line runs inside the conversation's `provider_request` span, whose
`gap_ms` field is the time since the start of the conversation's call before (absent
for the first call after a start of efrd; see the README of `efr-conversation`). So
the line, the gap and the call's `usage` measure which time to live pays.

### The stream

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
  `Refusal`, `pause_turn` is `PauseTurn` (efr does not continue it); an unknown or
  missing one is `ToolUse` with a tool call, else `EndTurn`.
- `ping` is ignored, `citations_delta` too. Any other unknown event, block or delta is
  `Raw`, never a failure.
- An `error` event fails the stream with the class of its `error.type` (see
  "Failures"); `message.input_transformations` entries of `message_start` are logged at
  `warn`.

### Catalog

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

### Token counts

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

### Failures

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
`request-id`, which is also a field of the request's span). An answer that ends in an
error after the 200 writes one `info` line, `the model call failed`, with the error
and the `request_id`, so the journal names the request of a failed turn.
The conversation fails a turn with `unauthorized` for `Unauthorized`, `busy` for
`RateLimited` and `Overloaded`, and `invalid` for `UnknownModel`.

## Tier

Tier 2.

## Allowed dependencies

`efr-provider`, `efr-http`, `efr-protocol` and `efr-stdx`; `xtask/src/deps.rs` holds
the allowlist. The crate uses `efr-provider` (the trait and the canonical types),
`efr-http` (the client, the retry policy, the SSE parser and the WebSocket client) and
`efr-stdx` (the `Clock` and the `Rng`). `efr-protocol` is allowed but not needed:
nothing here touches the wire types of the daemon's socket.

It must never depend on `efr-oauth-openai`: tokens and keys arrive through
`efr_provider::TokenSource`. The OpenAI provider gets an `AccessToken` with its account
id, so it never sees a refresh token and never parses a JWT. The Anthropic provider
gets a key that its `TokenSource` says it cannot refresh. The edge
`efr-providers -> efr-oauth-openai` is forbidden in `xtask/src/deps.rs`, and CI checks
it again with `cargo tree`.

Third-party crates: `async-trait`, `futures` (the event streams), `jiff` (the time of
the clock and of a fetched catalog), `serde` (the typed bodies, the blocks of
`provider_raw`, the entries of a catalog and its cache file), `serde_json` (with
`raw_value`, for the exact replay of Claude's assistant content), `thiserror`, `tokio`
(the task and the channels of a WebSocket connection) and `tracing`. The tests also
use `fastwebsockets` (the fake WebSocket server), `insta` (the body snapshots of the
Anthropic requests), `rstest`, `pretty_assertions`, `tempfile`, `tracing-subscriber`
(to read the log) and `wiremock` (the fake APIs). The benches use the dev crate
`efr-test-support` (the perf corpus and the bench report).

## Invariant

The two modules use nothing of each other. Each provider keeps these rules.

### openai

- The Responses client never knows how a token was obtained, and never sees a refresh
  token.
- `provider_raw` goes back to the provider exactly as it came: the items are sent
  verbatim, never edited, truncated or re-ordered. Raw items with a call of a tool
  that the request does not offer are not sent at all.
- No secret reaches a log or a transcript: the token goes in a sensitive
  `Authorization` header, requests are recorded only through `efr_http`'s redacting
  recorder, and error messages come from the server's error body, never from the
  request.
- A model call is never sent twice after the server may have acted on it. Over a
  WebSocket, a call goes over HTTP only when the server certainly did not act on its
  request: the request did not go out, or the server refused it with a close frame or
  an `error` event before an answer.
- `previous_response_id` goes only on the WebSocket connection that holds that answer,
  and only when the rest of the request is unchanged.
- efr names itself honestly: a fetch of the catalog sends efr's own version as
  `client_version` and the configured `originator` (`efr` by default), never the
  values of another client.
- A request never waits for a fetch of the catalog: it reads the current catalog from
  memory.

### anthropic

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
  as the ones that it signed is a check on a test key. A `tool_use` of a tool that the
  request does not offer is the one block that goes as other bytes: as text, the same
  in every request with the same tools.
- Two following requests of one conversation have the same bytes for `tools`, `system`
  and every earlier message, apart from the cache markers. The conversion never merges
  user messages, so a user message that ended the request before keeps its bytes when
  a new prompt follows it, as after a turn that ended on its tool results (an
  interrupt while a tool ran) or on its prompt (a turn that failed before an answer).
- A model call is never sent again after its stream has started.
- No model table and no guessed window: every model fact comes from the API.

## Sources

### openai

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

### anthropic

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
cargo nextest run -p efr-providers
```

Add `-E 'test(/^openai::/)'` or `-E 'test(/^anthropic::/)'` to run the tests of one
provider.

### openai

`convert` tests pin the request body and the item shapes, including the verbatim
passthrough of `provider_raw`, the fallbacks and the two forms of a freeform tool by
model. `sse_events` tests run the mapper over the streams in `fixtures/openai/responses/`
(plain text, a tool call, a freeform `apply_patch` call, reasoning with encrypted
content, an error event, a failed response with a rate limit, a response stopped at
the output limit) and over small inline streams. `responses` tests drive
`OpenAiProvider` against a `wiremock` server on the loopback interface: the headers of
both backends (the organization and project on the API only), a quota 429 that is
sent once and a rate limit that is sent again after its wait, the reasoning round trip across two requests, the 401 refresh (with
`fixtures/openai/responses/unauthorized.json`), retries on the injected clock, the error
mappings, the redacted transcript record, and a new catalog that changes the tool form
from the next request. `catalog` tests read `fixtures/openai/catalog/models.json` (a list,
a hidden model, a model with a Codex version, a tool form that efr does not know, a
broken window and two broken entries): the models on offer and their order, the
windows, the default, the tag and what `apply` does with each answer. `catalog::cache` tests write and read the
cache file in a temporary directory, and `catalog::client` tests fetch from `wiremock`:
the query and the headers, a 304 for the tag, the 401 refresh (no refresh after a
second 401 until a fetch works, and none for a token that was refreshed meanwhile) and
the failures, the API list cut down to the key's ids, and `check_key` (a good key with
its headers, a refused key whose message hides the key, a 403 and a 500 with their
status, and no answer). `models` tests pin the table of each backend. `websocket` tests drive it against a fake
server (`src/openai/testing/responses_server.rs`) that speaks the WebSocket protocol as Codex
expects it and also answers `POST /responses`: every fixture streams the same events
over both transports, the handshake headers and the `response.create` body, the reuse
of a connection with only the new items, the whole input after a changed setting or a
compaction, the interrupt, the fallback to HTTP on a refused upgrade, a close and an
`error` event before the answer, the failure of a close after the answer started, the
failure of a broken connection and of a silent server after the request went out
(with no HTTP request), an error status that fails the call (a context overflow and a
429 with its wait), a second refused token in a row (in the event and at the upgrade)
that pauses WebSockets, a second connection of one conversation that does not cut the
answer of the first, a lost previous answer, the idle close, the ping before a call after a quiet spell
(answered, and not answered by a connection that hangs), and the switch with the catalog's
`prefer_websockets`. `continuation` tests check when a call may send only its new
items. The fixtures are hand-written in the Responses wire format, since the tests
make no real network or model calls. Nothing
sleeps on real time and nothing needs Zig.

### anthropic

`config` tests pin the defaults, the URLs and the refused settings. `usage` tests check
the three-part input sum, the one-hour writes, the thinking count and the cumulative
stream counts. `catalog` tests check an entry as a model, the `lifecycle` filter, the
efforts, the default model, what `apply` does with a list and the models of the config
over the catalog; its `cache` tests the round trip of the file, its form, its 0600 mode
and the files that are not used; its `client` tests, on `wiremock`, the pages, the
headers, the refused key, a 529 that is sent again and a spend cap that is not, a key
that the answer quotes, the broken lists and every result of `check_key`. `failure`
tests are table tests of each answer's class and of each `error` event.
`convert::breakpoints` tests are table tests of the placement: the walkthrough of two
turns with a tool loop, a steer, a summary request and the calls after a compaction,
the anchor of a prompt after a turn that ended on its tool results or on its prompt,
the anchor of a grown tool loop, a side call, each time to live, the fallbacks of S,
and every short history under every setting against the rules of the API (at most four
markers, in order, no one-hour marker after a five-minute one).

`convert` tests pin the body with insta snapshots (`src/anthropic/convert/snapshots/`: the
first call, a tool loop, a steer, the head after a compaction, a summary request, no
system prompt) and check each rule of "The request" in table tests, the raw replay
byte for byte among them. `convert::tests::prefix` builds the requests of four turns
from the stream fixtures, and of turns that end on their tool results or on their
prompt after a failed call, and checks that each body, without its markers, is a byte
prefix of the next, and that the first call of each turn marks an anchor. `sse_events` tests read the hand-written streams in
`fixtures/anthropic/messages/` (text, a tool call, thinking, omitted thinking, redacted
thinking, an error after the start, `max_tokens`, `refusal`) and cover each stop
reason, each error type, unknown events and broken tool calls.

`messages` tests, on `wiremock` and a clock whose sleeps end at once, check the
headers of a model call, that no `provider_options` key reaches the body, the retries
of a 429, a 5xx and a 529 with their waits, the classes that are sent once, an `error`
event after the stream started, a cut stream, a refused connection, and that no
recorded request, error or log line holds the key, also when the answer quotes it.

The tests make no network call, sleep on no real time and need no Zig.

## Benches

The `bench_perf_*` benches are ignored. `just bench-perf` runs them in release, one at
a time. They report and do not fail on a time. A bench with a budget of a count or a
slope fails when that budget fails. The history comes from the perf corpus
of `efr-test-support`. The test binary has no counting allocator, so the benches report
the time and the growth of the peak resident set.

- `bench_perf_ws_body` (`src/openai/websocket/tests/perf.rs`): the HTTP body, and the
  `response.create` message of the WebSocket with the parts that the connection keeps,
  for the whole input and for a continuation, at 200 and 2,000 input items and at 3 MB
  of input. Budget: the connection keeps less than a tenth of the bytes of the input's
  JSON, and each slope over the items is less than 1.3.
- `bench_perf_messages_body` (`src/anthropic/convert/tests/perf.rs`): the Messages body
  of the same histories without the provider items.

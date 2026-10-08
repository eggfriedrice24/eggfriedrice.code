# efr-provider-openai

## Purpose

The OpenAI Responses API client. `OpenAiProvider` implements `efr_provider::Provider`
over a streaming `POST <base_url>/responses`, on one of two backends:

- the ChatGPT subscription backend, `https://chatgpt.com/backend-api/codex`, with a
  token from the subscription login, the `chatgpt-account-id` header from the token's
  account and the `originator` header (`efr` by default);
- the public API, `https://api.openai.com/v1`, with an API key.

Modules:

- `config`: `OpenAiConfig` (backend, base URL, originator, models, retry policy,
  reasoning mode, default reasoning effort and summary, parallel tool calls) and the
  `Backend` and `ReasoningMode` enums. A request's `provider_options` override the
  defaults: `reasoning_effort`, `reasoning_summary`, `parallel_tool_calls`,
  `prompt_cache_key`, `service_tier` and `text_verbosity`; other keys are ignored.
- `models`: the models the subscription is known to serve, with their limits, the
  reasoning efforts each takes and its default effort, verified against Codex's
  bundled catalog and opencode on 2026-10-05 (the sources are in the code). Codex
  fetches the list from `/backend-api/codex/models` for a ChatGPT login; efr does
  not call it yet. The lists are hints: a model that is not listed is still sent, and
  an answer that says the model is not served becomes `ProviderError::UnknownModel`.
  The catalog also says which models take freeform (`custom`) tools
  (`takes_freeform_tools`): Codex's `apply_patch_tool_type` is `"freeform"` for every
  listed model (read at c0c230e on 2026-10-08), so each of them does, on both
  backends. Every model outside the catalog gets the function form of a freeform
  tool, which every model with function calls takes.
- `convert`: canonical `Request` and `Message` to the Responses body and `input`
  items, and output items back to their canonical parts. The body follows Codex:
  `stream: true`, `store: false`, the system prompt as `instructions`,
  `tool_choice: "auto"`, function tools with `strict: false`, and for reasoning models
  `reasoning: {effort, summary}` with `include: ["reasoning.encrypted_content"]`. The
  subscription path sends no `max_output_tokens`, which that backend refuses. The API
  path sends the model's own limit from its entry in `[openai] models`, else the
  request's. A
  freeform tool goes to a model that takes it as
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
- `responses`: `OpenAiProvider`, the HTTP exchange and the event stream.
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

## Tier

Tier 2.

## Allowed dependencies

`efr-provider`, `efr-http`, `efr-protocol` and `efr-stdx`; `xtask/src/deps.rs` holds
the allowlist. The crate uses `efr-provider` (the trait and the canonical types),
`efr-http` (the client, the retry policy and the SSE parser) and `efr-stdx` (the
`Clock`). `efr-protocol` is allowed but not needed: nothing here touches the wire
types of the daemon's socket.

It must never depend on `efr-oauth-openai`: tokens arrive through
`efr_provider::TokenSource` as an `AccessToken` with its account id, so this crate
never sees a refresh token and never parses a JWT. That edge is forbidden in
`xtask/src/deps.rs`.

Third-party crates: `async-trait`, `futures`, `serde`, `serde_json`, `thiserror` and
`tracing`.

## Invariant

- The Responses client never knows how a token was obtained, and never sees a refresh
  token.
- `provider_raw` goes back to the provider exactly as it came: the items are sent
  verbatim, never edited, truncated or re-ordered.
- No secret reaches a log or a transcript: the token goes in a sensitive
  `Authorization` header, requests are recorded only through `efr_http`'s redacting
  recorder, and error messages come from the server's error body, never from the
  request.
- A model call is never sent twice after the server may have acted on it.

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
  `codex-rs/models-manager/models.json` (model ids, `context_window`, the reasoning
  levels and the default level; read again at 823ea83 on 2026-10-05),
  `codex-rs/model-provider/src/models_endpoint.rs` (the `/models` endpoint that
  Codex asks for a ChatGPT login),
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
mappings and the redacted transcript record. The fixtures are hand-written in the
Responses wire format, since the tests make no real network or model calls. Nothing
sleeps on real time and nothing needs Zig.

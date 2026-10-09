# Providers

A provider is the service that runs the model. efrd uses one provider for all new
conversations. `[model] provider` in `config.toml` sets it:

| Provider | Models | Login |
|---|---|---|
| `openai-subscription` (the default) | OpenAI models on your ChatGPT plan | `efr login openai` in a browser |
| `openai-api` | OpenAI models, billed to an OpenAI API key | `efr login openai-api` with a key |
| `anthropic-api` | Anthropic's Claude models, billed to an Anthropic API key | `efr login anthropic` with a key |

This file tells how to log in with an API key, how to change the provider, which
settings the Claude models have, and how the prompt cache works. [`config.md`](config.md)
lists every key.

## Log in with an API key

Make a key in the Claude Console (Anthropic) or in the dashboard of the OpenAI
platform. Then give it to efr in one of three ways:

```sh
efr login anthropic                   # type the key at a prompt that does not show it
printenv ANTHROPIC_API_KEY | efr login anthropic
efr login anthropic --from-env        # read ANTHROPIC_API_KEY of this shell
```

`efr login openai-api` works the same, with `OPENAI_API_KEY` for `--from-env`.
`efr login anthropic-api` is the same as `efr login anthropic`.

- efr never takes a key as an argument. Other users can read the arguments of a
  process, and the shell keeps them in its history.
- efrd checks the key with one request that runs no model. For Anthropic, this is
  `GET /v1/models?limit=1`. When the API refuses the key, efr shows the message of the
  API and keeps nothing.
- After the check, efrd keeps the key in `secrets/` of its data root
  (`~/.local/share/efr/secrets/` by default), in a file that only you can read. The
  permission rules of efr deny this directory to the tools of the agent.
- `--no-check` keeps the key without the check, for a computer that is offline.
- `efr status` shows each provider with its login. A key shows only as its start and
  its last four characters, such as `sk-ant-...a1b2`. `(active)` marks the provider of
  new conversations.
- `efr logout anthropic` deletes the key from efrd. The key stays valid at Anthropic:
  revoke it in the Claude Console.
- A key can expire, such as a Console key with an expiry date. Then every request
  fails as `unauthorized`. Make a new key and log in again.

### Keep the key out of the environment of efrd

Do not put an API key in the environment of the efrd service: not in
`~/.config/environment.d`, not with `systemctl --user set-environment` and not in the
unit file. efrd does not read `ANTHROPIC_API_KEY` or `OPENAI_API_KEY` from its
environment. But efrd gives its whole environment to the hidden shells. In the
`manual` and `cautious` modes, a command of the agent can then read the key with
`env`. Only the `auto` sandbox removes names such as `*API_KEY*`. Use `efr login`.

## Change the provider

A login does not change the provider of new conversations. After the login, efr tells
you what to do when the provider is not the active one:

```sh
efr config set model.provider anthropic-api
systemctl --user restart efrd
```

`[model] provider` needs a restart of efrd. `efr models` then lists the models of the
new provider, and its last line names the provider, such as `models: anthropic-api,
from the backend, fetched 5m ago`. A terminal that chose a model with `,model` keeps
that choice: choose a model of the new provider, or clear the choice with `,model
default`.

## Claude models

efr has no built-in list of Claude models. The models, their context windows, their
output limits and their efforts come from the model list of the API (`GET
/v1/models`, only the models that are `active`).

- efrd gets the list at start, after each login and then every hour. A failed request
  tries again after 15 s, 30 s, 1 min and 2 min, then every 5 min.
- efrd keeps the list in `anthropic_model_catalog.json` in its state root
  (`~/.local/state/efr/` by default). A start without network uses that file.
- Before the first list (the first start, or a start before the network is up), a
  prompt waits for one request of the list, for at most 60 s. `efr models` then says
  `no list yet; efrd fetches it before the next prompt`.
- The default model is `claude-opus-5-5` when the API lists it. `[model] name` sets
  another model, such as `claude-sonnet-5-5`. `,model` sets one for a terminal.
- The default effort is `medium`, as in Claude Code. `[model] effort` and `,effort`
  set another one. efr sends the effort to each model that lists it. A model that
  takes no effort gets none.
- The context window is the window of the list: 1M tokens on the current models.
  efr compacts at `[compaction] auto_at` of the window (76% by default).
- efr sends the signed thinking of each answer back unchanged, so the model keeps its
  reasoning through a tool loop. While the model thinks, the status row shows
  `thinking` and the title of the summary of its thinking.
- Claude models change files with the `edit` tool. OpenAI models use `apply_patch`.
- Without a key, a turn fails as `unauthorized` and efr says to log in.

### The `[anthropic]` keys

| Key | What it does |
|---|---|
| `models` | Adds a model that the API does not list yet, or lowers the window or the output limit of a listed model. An entry is a model id or a table `{ id, context_window, max_output_tokens }`. Applies to the next turn. |
| `cache_ttl` | How long the prompt cache keeps what a request writes: `auto` (the default), `5m` or `1h`. See "The prompt cache" below. Needs a restart. |
| `workspace_id` | The workspace of your key, sent as the `anthropic-workspace-id` header of every request and of the login check. Needs a restart. |
| `base_url` | Replaces `https://api.anthropic.com/v1`, such as for a proxy. Needs a restart. |

### A smaller window costs less

A large window lets a conversation grow, and each call sends the whole conversation.
Cached input is cheap, but it is not free. To compact sooner, lower the window:

```toml
[anthropic]
models = [{ id = "claude-opus-5-5", context_window = 272000 }]
```

Claude Haiku 5.5 has a price tier: when a request is longer than 100k tokens, the
whole request costs 5 times as much. We recommend a window of 100k for Haiku, so that
efr compacts before the tier:

```toml
[anthropic]
models = [{ id = "claude-haiku-5-5", context_window = 100000 }]
```

`efr config check` notes a window above the largest window of its model. efrd then
uses the largest window.

### The workspace id

A key that belongs to one workspace needs no workspace id. A key that is not scoped
to one workspace, such as a key of a personal or a service account, needs one. Without
it, the API refuses every request with a 400 that names `anthropic-workspace-id`, and
the login check shows that message. You can:

- make a new key in the Console that is scoped to one workspace (the simpler way); or
- set the id of the workspace, then log in again:

```sh
efr config set anthropic.workspace_id wrkspc_01AbCd
systemctl --user restart efrd
```

The workspace id is not a secret, so it can be in a dotfiles repository.

## The prompt cache

The provider keeps the start of each request in a cache. The next request that starts
with the same text reads it for a small part of the price. efr keeps the history of a
conversation append-only, so each request starts with the request before it and its
answer.

The end-of-turn line shows the part of the turn's input that came from the cache,
such as `done in 42s, ctx 43% (89k/207k), 1.1k out, cache 91%`. A write to the cache
counts as a miss. The line shows it only when the turn's input is 2048 tokens or
more: below 512 tokens (Claude) or 1024 tokens (OpenAI), no cache entry forms.

OpenAI caches each request without any setting. Claude caches only where the request
puts a marker, and each marker keeps its entry for five minutes or for one hour.

### `cache_ttl`

A write for five minutes costs 1.25 times the input price. A write for one hour costs
2 times. A read costs 0.1 times or less. Each read starts the time to live again. A
pause longer than the time to live loses the entry, and the next request writes it
again.

| Value | What it does |
|---|---|
| `auto` (default) | The system prompt, the tools and the conversation up to the start of the turn stay for one hour. The calls of a tool loop write their part for five minutes. When a tool loop grows by about 20,000 tokens, the next call keeps the conversation up to that point for one hour. So a pause of any length up to one hour loses at most about 20,000 tokens: between two prompts, or while efr waits for your approval. |
| `5m` | Everything for five minutes. This costs less when you send the next prompt within five minutes. A longer pause writes the whole conversation again. |
| `1h` | Everything for one hour. Each write costs more, and a pause up to one hour loses nothing. |

A summary call of a compaction keeps its part for five minutes, because no later
request reads it.

### What makes the cache miss

| Event | Claude | OpenAI |
|---|---|---|
| A change of the model (`,model`) | The whole cache: each model has its own | The whole cache |
| A change of the effort (`,effort`) | The conversation; the system prompt and the tools stay | Can miss |
| A new `[model] system_prompt` | The system prompt and the conversation | The whole cache |
| A compaction | The conversation; the system prompt and the tools stay | The whole cache |
| A pause longer than the time to live | The whole cache | The whole cache (OpenAI keeps an entry for 30 minutes or more) |
| A restart of efrd | Nothing | Nothing |

### Measure the time to live

With `log = "info,efr_=debug"` in `config.toml`, the log of efrd shows two facts for
each model call:

- `gap_ms`: the time since the start of the conversation's call before. The first
  call after a start of efrd has none.
- For Claude, one line with the markers of the request, such as `cache_ttl=auto
  markers=S1h,A1h,P5m,T5m`: S is the system prompt and the tools, A the newest
  one-hour point in the conversation, P the end of the call before and T the end of
  this request. Under `auto`, `T1h` means that the call started a new one-hour point.

The `turn_completed` event of each turn holds its token counts: the cached input
(`cached_input_tokens`), the cache writes (`cache_write_tokens`) and the writes for
one hour (`cache_write_1h_tokens`). Together, these show which `cache_ttl` costs less
for the way you work.

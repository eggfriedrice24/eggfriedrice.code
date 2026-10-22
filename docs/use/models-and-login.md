# Models, providers and login

<!-- sources: README.md "Install", "API keys", "Use" (`,model`/`,effort` bullet), "First use"; docs/providers.md "Log in with an API key", "Keep the key out of the environment of efrd", "Change the provider", "Claude models", "The reviewer"; docs/config.md [model], [openai], [anthropic]; crates/efr-cli/src/cli.rs (login, logout); crates/efr-cli/src/commands/login.rs, login/key.rs, logout.rs, models.rs, status.rs, settings.rs; crates/efr-cli/src/error.rs (login_hint, MODEL_HINT); crates/efr-oauth-openai/src/lib.rs, login.rs, callback.rs; crates/efr-credentials/src/file_store.rs; crates/efr-daemon/src/providers.rs, catalog.rs -->

A provider is the service that runs the model. efrd talks to the model API directly.
It uses one provider for all new conversations, and `[model] provider` in
`config.toml` sets it:

| Provider | Models | Login |
|---|---|---|
| `openai-subscription` (the default) | OpenAI models on your ChatGPT plan | `efr login openai` in a browser |
| `openai-api` | OpenAI models, billed to an OpenAI API key | `efr login openai-api` with a key |
| `anthropic-api` | Anthropic's Claude models, billed to an Anthropic API key | `efr login anthropic` with a key |

## Log in to the ChatGPT plan

```sh
efr login openai
```

efr prints a URL and waits:

```text
Open this URL in a browser to log in to OpenAI:

  https://...

Waiting for the browser to finish the login...
```

Open the URL in a browser on the same machine. The daemon waits for the browser on
`127.0.0.1` only, so nothing off the machine can reach it. With `EFR_OPEN_BROWSER=1`,
efr opens the URL itself. Ctrl+C stops the login. efrd keeps the login and refreshes
its token before the token expires.

## Log in with an API key

Make a key in the Claude Console (Anthropic) or in the dashboard of the OpenAI
platform. Then give it to efr in one of three ways:

```sh
efr login anthropic                   # type the key at a prompt that does not show it
printenv ANTHROPIC_API_KEY | efr login anthropic
efr login anthropic --from-env        # read ANTHROPIC_API_KEY of this shell
```

`efr login openai-api` works the same, with `OPENAI_API_KEY` for `--from-env`.

- efr never takes a key as an argument. Other users can read the arguments of a
  process, and the shell keeps them in its history.
- efrd checks the key with one request that runs no model. When the API refuses the
  key, efr shows the message of the API and keeps nothing.
- `--no-check` keeps the key with no check, for a computer that is offline.
- efrd keeps the key in `secrets/` of its data root (`~/.local/share/efr/secrets/`
  by default), in a file that only you can read. The permission rules deny this
  directory to the tools of the agent.

When the login works, efr says so with the start and the last four characters of the
key, such as `logged in to anthropic-api with key sk-ant-...a1b2`. After
`efr login openai-api`, one more line says: `note: OpenAI bills the API key per token;
a ChatGPT plan does not cover it`.

### Keep keys out of the environment of efrd

Do not put an API key in the environment of the efrd service: not in
`~/.config/environment.d`, not with `systemctl --user set-environment` and not in the
unit file. efrd does not read a key there. But efrd gives its whole environment to
the hidden shells. In the `manual`, `cautious` and `full` modes, a command of the
agent can then read the key with `env`. Only the `auto` sandbox removes names such as
`*API_KEY*`. Use `efr login`.

## Change the provider

A login does not change the provider of new conversations. When the new login is not
for the active provider, efr tells you what to do:

```sh
efr config set model.provider anthropic-api
systemctl --user restart efrd
```

`[model] provider` needs a restart of efrd. Also change `[model] name` when it names a
model of the other company, or remove it. If not, every prompt that names no model
fails.
`efr config check` warns about such a name.

A terminal that chose a model with `,model` keeps that choice. Choose a model of the
new provider, or clear the choice with `,model default`.

## Check the login

`efr status` shows each provider with its login. `(active)` marks the provider of new
conversations:

```text
provider       openai-subscription: logged in, token expires 2026-10-10 18:00:00 UTC (in 6d 6h)
provider       openai-api: not logged in
provider       anthropic-api: logged in, key sk-ant-...a1b2 refused at 2026-10-04 11:55:00 UTC (5m 0s ago) (active)
```

When the provider refused a key, the line says when. The mark goes away when a request
with the key works again, or after a new login. When the active provider has no login,
`efr status` also says how to log in, such as `log in with: efr login anthropic`.

## Log out

```sh
efr logout anthropic
```

efrd deletes the stored login. An API key stays valid at its provider: revoke it in
the Claude Console or in the dashboard of the OpenAI platform. When you log out of the
active provider, efr warns that its turns fail until you log in again.

## Choose a model

`,model` and `,effort` work as `,mode` does. They set the model and the reasoning
effort for the prompts of this terminal only:

```text
,model gpt-5.4      # this terminal uses gpt-5.4
,effort high        # and the effort high
,model              # show the model, where it comes from, and the choices
,model default      # the config decides again
```

efr keeps a value only when the daemon accepts it. Otherwise efr lists the choices.
Tab completes the models and the efforts of the model. `efr send` and `efr new` take
`--model` and `--effort` for one prompt. `EFR_MODEL` and `EFR_EFFORT` give a new shell
its first values.

To set the default for every terminal, set `[model] name` and `[model] effort`:

```sh
efr config set model.name gpt-5.4
```

The next turn uses the new value, with no restart.

### List the models

`efr models` lists the models of the active provider. `*` marks the default. Each line
shows the context window and the efforts of the model:

```text
* gpt-5.5   272k (up to 872k)  efforts: low, medium, high, xhigh; default medium
  gpt-5.4   128k               efforts: low, medium, high
```

The last line names the provider and where the list came from, such as
`models: openai-subscription, from the backend, fetched 5m 0s ago`.
`efr models --names` prints only the ids, one on each line.

### Where the list comes from

- **OpenAI.** The subscription's list comes from its backend. efrd starts with the
  list of its last fetch. If there is none, it starts with a table built into efr. For `openai-api`, the
  `/v1/models` list of your key cuts that table down to the models that the key can
  use. The default is the model with the best priority in the list. `[openai] models`
  adds a model that the list does not hold yet.
- **Claude.** efr has no built-in list of Claude models. The models, their windows and
  their efforts come from the model list of the API. The default model is
  `claude-opus-5-5` when the API lists it, with the effort `medium`. Before the first
  list, a prompt waits for one fetch, for at most 60 seconds. `[anthropic] models` adds
  a model, or lowers the window of a listed model.

efrd fetches the list at start, after each login and then every hour. A failed fetch
keeps the list that efrd has and tries again sooner. A new list applies from the next
turn.

A change of the model during a conversation makes the whole prompt cache miss. See
[Providers](../providers.md#the-prompt-cache).

## The reviewer costs requests

In `auto`, a reviewer model decides most actions that leave the sandbox. It is the
small model of your provider: `gpt-6-luna` for OpenAI or `claude-haiku-5-5` for
Claude. `classifier.model` picks another one. Each review is one more request to your
provider. With an API key you pay for it per token. With a subscription it counts
against the limits of your plan. A review is small: it sends only the facts of one
action and your messages. [Providers](../providers.md#the-reviewer) tells more.

## When a prompt fails

- **`unauthorized`.** The provider of `[model]` has no login, or it refused the key.
  efr says how to log in, such as `efr login openai`. A key can expire, such as a
  Console key with an expiry date. Make a new key and log in again.
- **The backend refuses the model.** Which models the ChatGPT backend accepts from efr
  is known only after the first real request. efr names the fix: `,model <model>` for
  this terminal, or `name` under `[model]` in `config.toml`, which the daemon reloads
  with no restart.
- **The backend refuses the client.** The error shows the backend's message.
  `[openai] originator` changes the name that efr sends (`efr` by default). This key
  needs `systemctl --user restart efrd`.
- **A Claude key that is not scoped to one workspace.** The API refuses each request
  with a 400 that names `anthropic-workspace-id`. Make a key that is scoped to one
  workspace, or set `[anthropic] workspace_id` and restart efrd.
- **No daemon.** `efr` exits with 3 and says to run `systemctl --user start efrd`.

## See also

- [Providers](../providers.md): the Claude settings, the workspace id and the prompt
  cache.
- [config.toml reference](../config.md#model): the `[model]`, `[openai]` and
  `[anthropic]` keys.

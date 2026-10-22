<!-- sources: README.md "Settings" and the paragraph on roots; docs/config.md header; crates/efr-config/src/location.rs, reload.rs, writer.rs, theme_file.rs, keys.rs (RESTART_KEYS); crates/efr-cli/src/cli.rs (ConfigCommand, PathsArgs); crates/efr-cli/src/commands/config.rs, paths.rs; crates/efr-cli/src/format.rs (reloaded, status); crates/efr-daemon/src/main.rs; crates/efr-daemon/src/reload.rs; crates/efr-daemon/src/tools/settings_tool.rs; crates/efr-stdx/src/paths.rs; docs/permissions.md "The settings tool", "The config hold", "Config protection" -->

# Settings

All settings of efr are in one file, `config.toml`. This page tells where the file is,
how to change it and when a change applies. The
[config.toml reference](../config.md) lists every key with its default.

## The file

`config.toml` is in the config root, `~/.config/efr/config.toml` by default.

- The file is optional. A missing file means that every key has its default value.
- Every key is optional.
- An unknown key is an error. So efr tells you about a typo.
- The file holds no secrets. It can live in a dotfiles repository.

To keep the file in a dotfiles repository, make `~/.config/efr/config.toml` a symbolic
link to it. efr keeps the link:

- The daemon also watches the file behind the link.
- `efr config set` writes the file behind the link.
- `efr config edit` opens the file behind the link. So an editor that saves by
  replacing the file cannot turn the link into a plain file.

No tool of the agent can write the config directory or the files behind its links.
[Config protection](../permissions.md#config-protection) tells more.

## Change a setting

```sh
efr config edit                       # open the file in your editor
efr config set model.name gpt-5.4     # set one key
efr config unset model.name           # remove one key; its default applies again
```

- `efr config edit` opens the file in `$VISUAL`, else `$EDITOR`, else `vi`. A missing
  file starts as the commented example, which names every key. When you save, efr
  checks the file. If it has an error, efr shows the error and asks `edit it again?
  [Y/n]`.
- `efr config set KEY VALUE` changes one key. The value is text, a number, `true` or
  `false`, or a list as words separated by commas. efr keeps your comments and your
  layout.
- `efr config set` and `efr config unset` cannot change the arrays of tables:
  `[[permissions.rules]]`, `[[sandbox.allow]]`, `[[network.hosts]]` and
  `[[dbus.allow]]`. Edit these in the file.

After each change, efr asks the daemon to read the file again and prints the result:

```text
config.toml reloaded; new turns use it
```

When the daemon does not run, efr says `the daemon is not running; it reads the file
when it starts`.

## Check the file

- `efr config check` checks `config.toml`. `efr config check PATH` checks another
  file. An error names the line, the column and the key, and the command exits with 1:

  ```text
  $ efr config check config.toml
  config.toml: unknown field `idle_minuets`, ... (shell.idle_minuets, line 3, column 1)
  ```

- `efr config show` prints every key with its value and where the value comes from, as
  TOML with the source in a comment. Then it shows the file that the daemon reads, the
  last error of a reload and the keys that wait for a restart.
- `efrd --print-config` prints the settings of the daemon with their sources. The
  variables `EFR_LOG` and `EFR_SCREEN` and the `efrd` flags `--log` and `--screen` win
  over the file.

## When a change applies

The daemon reads the file again:

- when the file changes,
- on `efr config reload`,
- on `systemctl --user reload efrd`.

A running turn keeps its settings. The next turn uses the new ones. The
[config.toml reference](../config.md) has an Applies column for each key:

| Applies | When the change takes effect |
|---|---|
| `live` | From the next turn, prompt, tool call or new hidden shell. No restart. |
| `restart` | Only after `systemctl --user restart efrd`. |
| `client` | Only `efr` reads the key, each time it runs. |

The keys that need a restart are `screen`, `model.provider`, the base URLs, and a few
keys of `[openai]` and `[anthropic]`, such as `openai.originator`. For such a key,
`efr config reload` says `restart efrd to apply: <keys> (systemctl --user restart
efrd)`. `efr status` shows the keys in the `restart needed` line.

A file with an error changes nothing. The old settings stay. `efr status` shows the
error in the `config error` line, and the next prompt in each terminal with a recent
conversation shows one line, such as:

```text
efr: config.toml has an error; the old settings stay: <error> (<key>, line <n>)
```

While a full mode turn or one of its jobs runs, efrd does not load a change that
loosens permissions. The change stays held until you run `efr config reload` after the
full work ends. [The config hold](../permissions.md#the-config-hold) tells more.

## Ask efr to change a setting

You can ask the agent, as in `, make gpt-5.4 my default model` or
`, always allow cargo test in this project`. The agent uses its settings tool:

- It reads the settings with no question.
- It changes the file only after you approve the change. The question shows the diff
  of the file, in every mode, also in `auto`, and also when a rule of yours allows the
  write.
- A change that lets more run without a question is marked "loosens permissions".
- The tool never writes a rule about secrets. It never turns on full mode
  (`permissions.allow_full`) and never sets `permissions.mode = "full"`. You do that
  yourself.
- A turn from the phone cannot change settings.

The change applies from the next turn. [The settings
tool](../permissions.md#the-settings-tool) tells more.

## Settings for one terminal

The file sets the defaults of every terminal. To change the mode, the model or the
effort for one terminal only, type `,mode`, `,model` or `,effort`. `,mode default`
clears the choice of the terminal, so the file decides again.
[Permission modes](../use/modes.md) and
[Models, providers and login](../use/models-and-login.md) tell more.

## Colours

By default, efr uses the 16 colours of your terminal's theme. So it follows your
terminal's theme. The `[render]` table changes this:

- `[render.colors]` sets the colour of one role, such as `accent = "#f2c14e"` or
  `muted = 8`.
- `render.palette` names a theme file, such as `palette = "~/path/theme.toml"`. The
  path must be absolute or start with `~/`. The file has a `[colors]` table with the
  same roles, and an optional `code_theme`: the path of a `.tmTheme` file for code. A
  design system can make this file. `[render.colors]` wins over the theme file.
- `render.theme` names a theme for code blocks and diffs, such as `catppuccin-mocha`.
  `theme = "auto"` takes `theme_dark` or `theme_light` from the background of the
  terminal. The zsh plugin asks the terminal for its background once, when it loads.
  Set `EFR_TERMINAL_BG` to `dark` or `light` to skip the question.
- `NO_COLOR` turns colours off.

`efr config set render.theme NAME` refuses a theme that does not exist.

## Where efr keeps its files

efr uses four roots. efr finds each root in this order:

1. its own variable: `EFR_CONFIG_DIR`, `EFR_DATA_DIR`, `EFR_STATE_DIR` or
   `EFR_RUNTIME_DIR`,
2. a directory below `EFR_HOME`: `config/`, `data/`, `state/` or `runtime/`,
3. the XDG directory: `~/.config/efr`, `~/.local/share/efr`, `~/.local/state/efr` or
   `$XDG_RUNTIME_DIR/efr`. When `XDG_RUNTIME_DIR` is not set, the runtime root is
   `/run/user/<uid>/efr` if `/run/user/<uid>` is your own directory with mode 0700.

`efr paths` shows each root, where it came from and whether it exists. Then it shows
`config.toml`, the database `efr.sqlite`, the `secrets/` directory, the socket
`daemon.sock`, and the roots of the daemon. `efr paths --json` prints the same facts
as JSON.

```text
runtime          /run/user/1000/efr  (XDG, exists)
daemon.sock      /run/user/1000/efr/daemon.sock  (exists)
```

When the daemon uses other roots than your shell, `efr paths` warns. This occurs, for
example, when only your shell sets `EFR_HOME`. Give the daemon the same variable with
`systemctl --user edit efrd`.

Below `EFR_HOME`, the socket is on the disk. A socket path can be at most 107 bytes.
If the path is longer, the daemon does not start, and efr tells you to set
`EFR_RUNTIME_DIR` to a shorter directory.

[Storage](../storage.md) lists every file in the roots.

## The JSON schema

`efr config schema` prints the JSON schema of `config.toml`. The file
`docs/config.schema.json` in the repository holds the same schema. The first line of
the example file, `#:schema ...`, points editors to it, so an editor can complete keys
and check values.

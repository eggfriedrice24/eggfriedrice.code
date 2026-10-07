# efr-config

## Purpose

`config.toml` in the config root, for `efrd` and `efr` alike: one schema, so a key the
client reads never breaks the daemon and the reverse. The file is optional; a missing
file is an empty one, and every value is its default. It holds no secrets, so it can
live in a dotfiles repository behind a symbolic link.

Modules:

- `tables`: one struct per table (`ModelSettings`, `OpenAiSettings`,
  `PermissionSettings`, `ShellSettings`, `ConversationSettings`, `SandboxSettings`,
  `RenderSettings`) with `deny_unknown_fields` and the defaults in its `Default`, plus
  `ScreenChoice`, `SudoCache`, `WriteProjects` and the default constants
  (`DEFAULT_SYSTEM_PROMPT`, `DEFAULT_CACHES` and the rest). The doc comment of a field
  is its description in the JSON schema. A new key is one field here, its check in
  `validate` when it needs one, and its line in the example.
- `tables/render`: `[render]` (`RenderSettings`): `theme` (`auto`, `AUTO_THEME`,
  takes `theme_dark` or `theme_light` by the terminal's background), `theme_dark`
  (default `catppuccin-mocha`), `theme_light` (default `catppuccin-latte`), `palette`
  (the path of a theme file), `motion` (the spinner and the band over the state of the status row),
  `turn_summary` (the line at the end of each turn), `progress` (`Progress`: `auto`,
  `on` or `off`, for the progress bar of the terminal's tab) and `[render.colors]` (`RenderColors`, with `DiffColors` for the
  `diff.*` roles), one key per colour role of `efr` (`COLOR_ROLES`). A role colour
  (`ColorValue`) is `"#rrggbb"`, an ANSI slot from 0 to 15 (a number or one or two
  digits in a string), or a lowercase name of the 16 colours, also as
  `bright-<name>`; `ColorValue::color` reads it as a `RoleColor`.
  `RenderColors::over` lays one layer over another, and `RenderColors::colors` lists
  the roles that are set.
- `theme_file`: `ThemeFile`, the theme file that `render.palette` names: a `[colors]`
  table with the keys of `[render.colors]` and an optional `code_theme`, the path of a
  `.tmTheme` file (absolute, `~/...`, or relative to the theme file). `ThemeFile::load`
  reads and checks it like the config file, with its own errors (`ThemeRead`,
  `ThemeParse`, `ThemeInvalid`) that name the key and the place.
- `tables/sandbox`: `[sandbox]`, the keys of the `auto` sandbox (phase 1): the write
  roots, caches, masks, floors, environment and export lists, synced folders and the
  surface report. The daemon turns them into an `efr_sandbox::SandboxSpec` per call;
  this crate only reads and checks them. The proxy, classifier, bus and undo keys come
  with their phases.
- `settings`: `Settings`, the whole file. `Settings::parse` reads the text with unknown
  keys refused, reads `[[permissions.rules]]` one by one so an error names
  `permissions.rules[N]`, records which keys the file set, and runs the checks.
  `Settings::apply_override` lays an environment variable or a flag over one key, read
  as the file would read it, and records its source. `Settings::load` reads the file
  from the config root.
- `validate`: what the types do not check: the provider names, the form of an effort
  (one lowercase word; the daemon checks it against the model), numeric ranges,
  absolute paths, `~/` secret and sandbox paths (never `~` or `/` as an extra write
  root), variable names and patterns, relative name patterns, http and https URLs,
  non-empty names.
- `keys`: `keys()`, every dotted key in the order the tables declare them; `kind()`,
  what a key holds, and `description()`, its doc comment, both from the JSON schema;
  `json_schema()`; `RESTART_KEYS`, the keys a change applies to only after a restart;
  `Applies`, when a change of a key takes effect (live, restart, or in `efr` only).
- `reload`: `Settings::reloaded`, the file read again laid over the running settings:
  a restart key that changed keeps its running value and is listed in
  `Reloaded::restart_needed`, and a key an environment variable or a flag set keeps
  that value, because the override still wins over the file.
- `file`: `FileState`, what is at the file's path (nothing, a file, or a symbolic link
  and its resolved target) and the directories a watcher must watch for it.
- `reference`: `reference()` and `schema_text()`, the text of `docs/config.md` (every
  key, its default, when it applies and its description) and of
  `docs/config.schema.json`. `cargo xtask config-docs` writes both; CI checks that they
  are current.
- `effective`: `Source` (default, file, `env VAR`, `flag --x`), `Entry`,
  `Settings::entries` and `Settings::effective`, the dump of `efrd --print-config`.
  The values come from serializing the typed settings, so a new key is listed without
  more code.
- `location`: byte offsets to lines and columns, and the dotted key at an offset, so an
  error says `line 3, column 1` and `shell.idle_minuets`. `ConfigError::file_error`
  turns an error into the wire `ConfigFileError`.
- `example`: `EXAMPLE`, `examples/config.toml`: every key with a comment and its default
  or an example value commented out.
- `writer`: `ConfigFile` and `Edit`, the format-preserving writer of `efr config set`
  and the settings tool. Comments and layout stay (`toml_edit`). The file is read and
  written through `efr_stdx::fs::LinkedFile`, which the project registry shares: a
  symbolic link is followed and the file behind it is written: a temporary file in the
  target's directory, flushed, renamed (`efr_stdx::fs::write_atomic`, so the new file
  has mode 0600); the link stays. A missing file is created from the example, with its
  directory; a link to nothing is refused. The file as read before the change is
  compared with the file right before the write, so a change made meanwhile is never
  lost: the write fails with `ConfigError::Changed` and the caller plans the change
  again. The new
  text is checked like a load before anything is written. `Edit::add_rule` appends a
  rule after the rules of the file (`[[permissions.rules]]` right below
  `[permissions]`, or the inline array the file already uses) and
  `Edit::remove_rule` removes one by its place; only the settings tool calls them, and
  `efr config set` never changes rules.

`permissions.mode` is the mode of a turn whose prompt names none;
`PermissionSettings::policy(mode)` is that mode's built-in policy followed by the
user's rules. `model.name` and `model.effort` are checked here for their form only: the
model list belongs to the provider, so the daemon checks them against it, with a
warning at start and an `invalid` error for a turn that uses them.
`render.theme`, `render.theme_dark` and `render.theme_light` are checked for a
non-empty name only: the theme list lives in
`efr-render`, which this crate may not depend on, so `efr` checks the name. For the
same reason the role names of `[render.colors]` are listed here and in `efr-render`;
`efr` turns a `RoleColor` into an `efr_render::Colour`. A role colour that is not a
colour is an error that names its key, such as `render.colors.accent`. The colours
apply in this order: `[render.colors]` over the theme file over the 16-colour
defaults of `efr-render`; the theme file's `code_theme` wins over `render.theme`.

## Tier

Tier 2: below `efr-daemon` and `efr-cli`, which both depend on it.

## Allowed dependencies

`efr-permissions` (the rules), `efr-protocol` (the `Mode` and `CacheMode` wire types
and `ConfigFileError`) and `efr-stdx` (variables, atomic writes). `xtask/src/deps.rs` holds
the allowlist. `efr-tools` must never depend on this crate: it reaches
`efr-permissions`, and `efr-tools -> efr-permissions` is forbidden through any chain.

Third-party crates: `serde`, `toml` and `toml_edit` (reading and the writer),
`schemars` and `serde_json` (the JSON schema) and `thiserror`.

## Invariant

One schema serves both programs and denies unknown keys everywhere, so a typo is an
error and never silently does nothing. A value is valid in the file exactly when it is
valid as an override. The writer never writes a file that does not load, never writes
through a link to nothing, and never overwrites a change it did not read. Nothing here
blocks an async worker on its own: `load` and the writer block, and async callers run
them in `spawn_blocking`.

## Tests

Run the tests of this crate alone, without the rest of the workspace:

```sh
cargo nextest run -p efr-config
```

The writer's tests run in temporary directories. Tests keep the example, the JSON
schema, the key list and the dump equal: the example names every key once with a
comment, marks exactly the restart keys, holds only defaults as shipped and is valid
with every line uncommented; the schema covers every key with a description; the dump
lists every key. The colour tests read every form of a role colour, refuse the rest
with the key, lay the layers in order, and read a hex theme file with a relative
`code_theme`. No network, no zsh, no Zig.

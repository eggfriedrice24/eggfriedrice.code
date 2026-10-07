//! The efr config file, `config.toml` in the config root: one schema for `efrd` and
//! `efr`.
//!
//! - [`Settings`] and its tables ([`ModelSettings`], [`OpenAiSettings`],
//!   [`PermissionSettings`], [`ShellSettings`], [`ConversationSettings`],
//!   [`SandboxSettings`], [`RenderSettings`] with [`RenderColors`] and
//!   [`DiffColors`]): every key with its default, read with
//!   unknown keys refused,
//!   then checked (sets, ranges, paths, URLs, rules). [`Settings::apply_override`] lays
//!   an environment variable or a flag over a key.
//! - [`Source`] and [`Entry`]: the effective view, every value with where it came from.
//! - [`keys`], [`kind`], [`description`], [`Applies`] and [`json_schema`]: the keys,
//!   what each holds, what it means, when a change applies and the JSON schema, all
//!   derived from the typed tables, so a new key is written once.
//! - [`EXAMPLE`]: `examples/config.toml`, every key with a comment.
//! - [`reference()`] and [`schema_text`]: `docs/config.md` and `docs/config.schema.json`.
//! - [`Settings::reloaded`] and [`Reloaded`]: a file read again, laid over the running
//!   settings, with the keys that need a restart.
//! - [`RenderColors`], [`ColorValue`] and [`RoleColor`]: the colour of each role of
//!   `efr` ([`COLOR_ROLES`]), from `[render.colors]` and from a theme file.
//! - [`ThemeFile`]: the theme file that `render.palette` names: a `[colors]` table and
//!   an optional `code_theme`, read and checked like the config file.
//! - [`FileState`]: what is at the file's path, a file, nothing, or a symbolic link.
//! - [`ConfigFile`] and [`Edit`]: the format-preserving writer, which keeps comments and
//!   layout, writes the file behind a symlink, refuses a file that changed since it was
//!   read, and adds and removes rules for the settings tool.
//!
//! Allowed dependencies: `efr-permissions` (rules), `efr-protocol` (the `Mode` and
//! `CacheMode` wire types and `ConfigFileError`) and `efr-stdx` (variables, atomic
//! writes). What does not belong here: async code, the network, reading the
//! environment, and applying a setting; the daemon and the CLI do that. `efr-tools`
//! must never depend on this crate, because it reaches `efr-permissions`.

// NOTE: missing_docs is set here and not in Cargo.toml, because Cargo rejects a
// `[lints]` table that both inherits the workspace lints and adds its own.
#![warn(missing_docs)]

mod effective;
mod error;
mod example;
mod file;
mod keys;
mod location;
mod reference;
mod reload;
mod settings;
mod tables;
mod theme_file;
mod validate;
mod writer;

pub use effective::{Entry, Source};
pub use error::{ConfigError, Location};
pub use example::EXAMPLE;
pub use file::FileState;
pub use keys::{Applies, Kind, RESTART_KEYS, SCHEMA_URL, description, json_schema, keys, kind};
pub use reference::{reference, schema_text};
pub use reload::Reloaded;
pub use settings::{CONFIG_FILE, Settings};
pub use tables::render::{
    COLOR_ROLES, ColorValue, DiffColors, RenderColors, RenderSettings, RoleColor,
};
pub use tables::sandbox::{
    DEFAULT_CACHES, DEFAULT_MASK_GLOBS, DEFAULT_PROMOTE_ENV, DEFAULT_REBUILDABLE,
    DEFAULT_SURFACE_FILES, DEFAULT_SYNCED_DIRS, SandboxSettings, WriteProjects,
};
pub use tables::{
    ConversationSettings, DEFAULT_LOG, DEFAULT_ORIGINATOR, DEFAULT_PROVIDER, DEFAULT_SYSTEM_PROMPT,
    ModelSettings, OpenAiSettings, PROVIDERS, PermissionSettings, ScreenChoice, ShellSettings,
    SudoCache,
};
pub use theme_file::ThemeFile;
pub use writer::{ConfigFile, Edit};

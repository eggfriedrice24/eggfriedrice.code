//! The efr config file, `config.toml` in the config root: one schema for `efrd` and
//! `efr`.
//!
//! - [`Settings`] and its tables ([`ModelSettings`], [`OpenAiSettings`],
//!   [`AnthropicSettings`] with [`CacheTtlChoice`],
//!   [`PermissionSettings`], [`ShellSettings`], [`ConversationSettings`],
//!   [`ContextSettings`], [`CompactionSettings`],
//!   [`SandboxSettings`] with [`AskDestructive`] and the always-allow rules
//!   ([`AllowRule`], re-exported from `efr-permissions`), [`NetworkSettings`] with
//!   [`NetworkHost`], [`DbusSettings`],
//!   [`ClassifierSettings`],
//!   [`UndoSettings`], [`SnapshotSettings`] with [`IgnoredFiles`], [`PresenceSettings`], [`JobsSettings`],
//!   [`ResumeSettings`] with [`ResumePicker`],
//!   [`DiffSettings`] with [`DiffGroups`], [`RenderSettings`] with [`Progress`], [`RenderColors`] and
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
//! - [`ModelCompany`] and [`ForeignModel`]: the company of a model id and of a
//!   provider, and a `[model] name` of another company than `[model] provider`
//!   ([`Settings::foreign_model`]).
//! - [`ConfigFile`] and [`Edit`]: the format-preserving writer, which keeps comments and
//!   layout, writes the file behind a symlink, refuses a file that changed since it was
//!   read, adds and removes rules for the settings tool, and adds and removes the
//!   entries of an array of tables such as `sandbox.allow`.
//!
//! Allowed dependencies: `efr-permissions` (rules and the network host list),
//! `efr-protocol` (the `Mode`, `CacheMode` and `NetworkMode` wire types and
//! `ConfigFileError`) and `efr-stdx` (variables, atomic
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
mod model_company;
mod reference;
mod reload;
mod settings;
mod tables;
mod theme_file;
mod validate;
mod writer;

// NOTE: efr-cli names the always-allow rule through this crate (`efr sandbox allow`),
// so it needs no edge to efr-permissions.
pub use effective::{Entry, Source};
pub use efr_permissions::AllowRule;
pub use error::{ConfigError, Location};
pub use example::EXAMPLE;
pub use file::FileState;
pub use keys::{Applies, Kind, RESTART_KEYS, SCHEMA_URL, description, json_schema, keys, kind};
pub use model_company::{ForeignModel, ModelCompany};
pub use reference::{reference, schema_text};
pub use reload::Reloaded;
pub use settings::{CONFIG_FILE, Settings};
pub use tables::anthropic::{AnthropicSettings, CacheTtlChoice};
pub use tables::classifier::{ClassifierSettings, DEFAULT_CLASSIFIER_EFFORT};
pub use tables::compaction::{CompactionSettings, DEFAULT_AUTO_AT};
pub use tables::context::ContextSettings;
pub use tables::dbus::DbusSettings;
pub use tables::diff::{DiffGroups, DiffSettings};
pub use tables::jobs::JobsSettings;
pub use tables::network::{NetworkHost, NetworkSettings};
pub use tables::presence::PresenceSettings;
pub use tables::render::{
    AUTO_THEME, COLOR_ROLES, ColorValue, DiffColors, Progress, RenderColors, RenderSettings,
    RoleColor,
};
pub use tables::resume::{ResumePicker, ResumeSettings};
pub use tables::sandbox::{
    AskDestructive, DEFAULT_CACHES, DEFAULT_MASK_GLOBS, DEFAULT_PROMOTE_ENV, DEFAULT_REBUILDABLE,
    DEFAULT_SURFACE_FILES, DEFAULT_SYNCED_DIRS, DOWNLOAD_CACHES, SandboxSettings, WriteProjects,
};
pub use tables::snapshot::{IgnoredFiles, SnapshotSettings};
pub use tables::undo::UndoSettings;
pub use tables::{
    ConversationSettings, DEFAULT_LOG, DEFAULT_ORIGINATOR, DEFAULT_PROVIDER, DEFAULT_SYSTEM_PROMPT,
    ModelEntry, ModelLimits, ModelSettings, OpenAiSettings, PROVIDERS, PermissionSettings,
    ScreenChoice, ShellSettings, SudoCache, WebSocketChoice,
};
pub use theme_file::ThemeFile;
pub use writer::{ConfigFile, Edit};

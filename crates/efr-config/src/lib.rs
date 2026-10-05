//! The efr config file, `config.toml` in the config root: one schema for `efrd` and
//! `efr`.
//!
//! - [`Settings`] and its tables ([`ModelSettings`], [`OpenAiSettings`],
//!   [`PermissionSettings`], [`ShellSettings`], [`ConversationSettings`],
//!   [`RenderSettings`]): every key with its default, read with unknown keys refused,
//!   then checked (sets, ranges, paths, URLs, rules). [`Settings::apply_override`] lays
//!   an environment variable or a flag over a key.
//! - [`Source`] and [`Entry`]: the effective view, every value with where it came from.
//! - [`keys`], [`kind`] and [`json_schema`]: the keys, what each holds and the JSON
//!   schema, all derived from the typed tables, so a new key is written once.
//! - [`EXAMPLE`]: `examples/config.toml`, every key with a comment.
//! - [`ConfigFile`] and [`Edit`]: the format-preserving writer, which keeps comments and
//!   layout, writes the file behind a symlink and refuses a file that changed since it
//!   was read.
//!
//! Allowed dependencies: `efr-permissions` (rules), `efr-protocol` (the `Mode` wire
//! type and `ConfigFileError`) and `efr-stdx` (variables, atomic writes). What does not
//! belong here: async code, the network, reading the environment, and applying a
//! setting; the daemon and the CLI do that. `efr-tools` must never depend on this crate,
//! because it reaches `efr-permissions`.

// NOTE: missing_docs is set here and not in Cargo.toml, because Cargo rejects a
// `[lints]` table that both inherits the workspace lints and adds its own.
#![warn(missing_docs)]

mod effective;
mod error;
mod example;
mod keys;
mod location;
mod settings;
mod tables;
mod validate;
mod writer;

pub use effective::{Entry, Source};
pub use error::{ConfigError, Location};
pub use example::EXAMPLE;
pub use keys::{Kind, RESTART_KEYS, SCHEMA_URL, json_schema, keys, kind};
pub use settings::{CONFIG_FILE, Settings};
pub use tables::{
    ConversationSettings, DEFAULT_LOG, DEFAULT_ORIGINATOR, DEFAULT_PROVIDER, DEFAULT_SYSTEM_PROMPT,
    ModelSettings, OpenAiSettings, PROVIDERS, PermissionSettings, RenderSettings, ScreenChoice,
    ShellSettings, SudoCache,
};
pub use writer::{ConfigFile, Edit};

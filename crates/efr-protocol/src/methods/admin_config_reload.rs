//! `admin.config_reload`: read `config.toml` again now, for `efr config reload`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::ConfigFileError;

/// The params of `admin.config_reload`, an admin method (Unix socket only). It takes
/// none.
///
/// The daemon also reloads on its own when the file changes and on `SIGHUP`; this
/// method reloads at once and answers with the outcome.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AdminConfigReload {}

/// The result of `admin.config_reload`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AdminConfigReloadResult {
    /// True when the file was valid and its settings now apply to new turns. False when
    /// it has an error; the old settings stay.
    pub applied: bool,
    /// What is wrong with the file, when `applied` is false.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ConfigFileError>,
    /// The dotted keys whose new values apply only after efrd restarts, such as
    /// `screen`. The daemon keeps their old values until then.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub restart_needed: Vec<String>,
    /// What the reload changed that the user should know, one sentence each, such as a
    /// `[model] name` of another company than `[model] provider`, an always rule that a
    /// floor disables, or a bus filter that is now off. Each warning comes once: a
    /// reload that changes nothing of it warns no more. A change that the config hold
    /// keeps back is in `error`, not here. Absent when there is none, and from an
    /// earlier daemon.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

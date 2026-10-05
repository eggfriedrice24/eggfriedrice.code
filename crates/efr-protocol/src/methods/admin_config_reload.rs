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
}

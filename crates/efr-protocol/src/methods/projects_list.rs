//! `projects.list`: the registered projects, for `efr project list`.

use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::ProjectId;

/// The params of `projects.list`. It takes none.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProjectsList {}

/// The result of `projects.list`: the daemon's project registry as the file holds it
/// now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProjectsListResult {
    /// The registry file, `projects.toml` in the config root.
    pub file: PathBuf,
    /// Every registered project, in the order of the file.
    pub projects: Vec<ProjectInfo>,
}

/// One registered project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProjectInfo {
    /// The id that `Scope::Project` carries.
    pub id: ProjectId,
    /// The project's root directory: absolute, in lexical normal form.
    pub root: PathBuf,
    /// A name for people, when the registry gives one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

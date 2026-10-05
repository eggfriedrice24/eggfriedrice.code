//! `admin.project_remove`: take a project out of the registry, for `efr project remove`.

use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{AdminConfigReloadResult, ProjectInfo};

/// The params of `admin.project_remove`, an admin method (Unix socket only).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AdminProjectRemove {
    /// The root of the project, absolute. It matches a registered root as given, and
    /// with symbolic links resolved when the directory still exists.
    pub path: PathBuf,
}

/// The result of `admin.project_remove`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AdminProjectRemoveResult {
    /// The project that was removed.
    pub project: ProjectInfo,
    /// The file that was written: `projects.toml`, or the file behind it when it is a
    /// symbolic link.
    pub file: PathBuf,
    /// The reload after the write, as for `admin.project_add`.
    pub reload: AdminConfigReloadResult,
}

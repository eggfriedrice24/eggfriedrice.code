//! `admin.project_add`: register a project, for `efr project add`.

use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{AdminConfigReloadResult, ProjectInfo};

/// The params of `admin.project_add`, an admin method (Unix socket only).
///
/// The daemon adds the project to `projects.toml` in its config root, keeping the
/// file's comments, and reloads, so the permission engine trusts the project from the
/// next tool call on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AdminProjectAdd {
    /// An absolute directory. The daemon registers it with symbolic links resolved.
    pub path: PathBuf,
    /// A name for people; absent means the last component of the root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// True to register the root of the git work tree that holds `path` instead, or
    /// `path` itself when none does. The daemon then refuses a root that is the home
    /// directory, `/` or a directory above the home directory, which only an explicit
    /// `path` may register. False when absent.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub git_root: bool,
}

/// The result of `admin.project_add`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AdminProjectAddResult {
    /// The project as registered.
    pub project: ProjectInfo,
    /// The file that was written: `projects.toml`, or the file behind it when it is a
    /// symbolic link.
    pub file: PathBuf,
    /// The reload after the write. When `config.toml` has an error, the daemon keeps its
    /// old settings and its old engine, and the project counts from the next reload
    /// that succeeds; a turn's scope already sees it.
    pub reload: AdminConfigReloadResult,
}

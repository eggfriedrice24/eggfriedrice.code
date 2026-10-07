//! `sandbox.explain`: what the `auto` sandbox does with one path, for
//! `efr sandbox explain`.

use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{ExitKind, Mode};

/// The params of `sandbox.explain`, a read method.
///
/// It is not an `admin.*` method on purpose: it changes nothing and reveals no more
/// than the user's own config does, so `read` is enough.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SandboxExplain {
    /// The path to explain: absolute, or relative to `cwd`.
    pub path: PathBuf,
    /// The directory the question is asked from, which picks the project. Absent: the
    /// daemon's view of the machine scope.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
}

/// The result of `sandbox.explain`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SandboxExplainResult {
    /// The path, absolute.
    pub path: PathBuf,
    /// The root of the project the answer is for; absent outside a project.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<PathBuf>,
    /// The mode the answer is for.
    pub mode: Mode,
    /// What the sandbox does with the path.
    pub role: SandboxPathRole,
    /// True when a contained call can read the path's content.
    pub read: bool,
    /// True when a contained call can write the path.
    pub write: bool,
    /// Why, in one sentence, such as `a shell startup file (floor)`.
    pub reason: String,
    /// The exit that a write of the path would be, such as `persistence`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub write_exit: Option<ExitKind>,
}

/// What the sandbox does with a path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum SandboxPathRole {
    /// It lies in a write root: read and write.
    WriteRoot,
    /// It lies in a cache overlay: writes go to a private copy.
    CacheOverlay,
    /// It lies in the private `/tmp`.
    PrivateTmp,
    /// Read only, like the rest of the system.
    ReadOnly,
    /// Masked: it reads as empty.
    Masked,
    /// A floor: read only even inside a write root.
    Floor,
    /// A socket, bus or device that an approval binds for one call: read and write.
    Granted,
}

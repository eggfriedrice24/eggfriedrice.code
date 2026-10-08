//! `[snapshot]`: efr's own snapshots of the files that the agent can change, which show
//! what a call and a turn changed (efr's auto spec, section 10, phase 4 keys of this
//! round; undo comes later).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Which ignored files a snapshot takes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum IgnoredFiles {
    /// No ignored file.
    None,
    /// Ignored files up to 1 MiB each, outside `target`, `node_modules`, `.venv`,
    /// `venv`, `__pycache__`, `dist`, `build`, `.next` and `.cache`, such as `.env`.
    #[default]
    Small,
}

impl IgnoredFiles {
    /// The name in the file, such as `small`.
    pub const fn as_str(self) -> &'static str {
        match self {
            IgnoredFiles::None => "none",
            IgnoredFiles::Small => "small",
        }
    }
}

/// `[snapshot]`: the snapshots before and after each call that can write, in the turn's
/// registered project, `$SCRATCH` and, in `auto`, the registered projects that a line
/// names. Every key applies from the next call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
pub struct SnapshotSettings {
    /// `false` takes no snapshot: a `shell` call and the end of a turn then list no
    /// changed files, and `efr diff` has nothing new. A file tool still shows its diff.
    pub enabled: bool,
    /// Files above this size in MiB are left out of a snapshot, from 1 to 1024: a new
    /// untracked file, and a file that grows past it, which then shows as changed with
    /// no line counts.
    pub max_file_mib: u32,
    /// Which ignored files a snapshot takes: `none` or `small` (up to 1 MiB each,
    /// outside build and dependency directories, such as `.env`).
    pub ignored: IgnoredFiles,
    /// A project or `$SCRATCH` with more files than this is not snapshotted, from 100 to
    /// 1000000; the debug log says so, and efr counts its files again after 10
    /// minutes.
    pub max_files: u32,
    /// The turns of each conversation whose snapshots stay, newest first, from 1 to
    /// 10000; older ones are deleted.
    pub keep_turns: u32,
    /// Days without a snapshot after which the store of a project or `$SCRATCH` is
    /// deleted, from 1 to 3650.
    pub max_age_days: u32,
}

impl Default for SnapshotSettings {
    fn default() -> Self {
        SnapshotSettings {
            enabled: true,
            max_file_mib: 10,
            ignored: IgnoredFiles::Small,
            max_files: 20_000,
            keep_turns: 50,
            max_age_days: 30,
        }
    }
}

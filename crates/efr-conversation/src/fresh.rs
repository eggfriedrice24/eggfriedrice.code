//! The fresh context block: what efr reads from disk when a compaction with a summary
//! ends, so the model does not work from a stale picture of the machine.
//!
//! The block is one user message before the summary: the user's directory and the
//! hidden shell's directory, the running jobs of the hidden shell, the git status of
//! the project, and the `AGENTS.md` files from the project root down to the user's
//! directory. The compaction stores it (`Compaction::fresh`), so every request until
//! the next compaction sends the same bytes and the prompt cache hits, also after a
//! daemon restart. Only for a compaction from an efrd before the stored block is the
//! block read from disk again; the actor then keeps it in memory with the compaction's
//! id.

use std::fmt::Write as _;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use efr_protocol::{Compaction, CompactionId, ConversationId, Scope};

use crate::ConversationDeps;

/// What the fresh block starts with.
pub(crate) const FRESH_OPEN: &str = "<fresh-context>";

/// What the fresh block ends with.
pub(crate) const FRESH_CLOSE: &str = "</fresh-context>";

/// The name of the instruction files that the block carries.
const AGENTS_FILE: &str = "AGENTS.md";

/// The most bytes of one `AGENTS.md` that the block carries.
const AGENTS_MAX_BYTES: usize = 32 * 1024;

/// A fresh block, with the compaction it belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Fresh {
    pub(crate) compaction_id: CompactionId,
    pub(crate) text: String,
}

/// What the fresh block says.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct FreshFacts {
    /// The user's directory.
    pub(crate) cwd: PathBuf,
    /// The hidden shell's directory; `None` when no shell runs.
    pub(crate) shell_cwd: Option<PathBuf>,
    /// The jobs in the background of the hidden shell; `None` when the toolbox cannot
    /// tell.
    pub(crate) jobs: Option<Vec<String>>,
    /// The project root, and its git status when git can tell.
    pub(crate) root: Option<PathBuf>,
    pub(crate) git_status: Option<String>,
    /// Each `AGENTS.md` from the project root down to the user's directory, with its
    /// text.
    pub(crate) agents: Vec<(PathBuf, String)>,
}

impl FreshFacts {
    /// The block as the model reads it.
    pub(crate) fn render(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "{FRESH_OPEN}");
        let _ = writeln!(
            out,
            "efr read this from disk when it compacted the conversation. It is newer than the \
             summary after it."
        );
        let _ = writeln!(out, "\n## Directories");
        let _ = writeln!(out, "- The user's directory: {}", self.cwd.display());
        match &self.shell_cwd {
            Some(dir) => {
                let _ = writeln!(out, "- The hidden shell's directory: {}", dir.display());
            }
            None => {
                let _ = writeln!(out, "- No hidden shell runs.");
            }
        }
        if let Some(jobs) = &self.jobs {
            let _ = writeln!(out, "\n## Running jobs of the hidden shell");
            if jobs.is_empty() {
                let _ = writeln!(out, "None.");
            }
            for job in jobs {
                let _ = writeln!(out, "- {job}");
            }
        }
        if let (Some(root), Some(status)) = (&self.root, &self.git_status) {
            let _ = writeln!(out, "\n## Git status of {}", root.display());
            let _ = writeln!(out, "```\n{}\n```", status.trim_end());
        }
        for (path, text) in &self.agents {
            let _ = writeln!(out, "\n## {}", path.display());
            let _ = writeln!(out, "{}", text.trim_end());
        }
        out.push_str(FRESH_CLOSE);
        out
    }
}

/// The fresh block of `compaction` without a read of the disk: the block that the
/// compaction stored, else `kept`, the block that the actor holds, when it belongs to
/// that compaction. `None` for a compaction from an efrd before the stored block whose
/// block the actor does not hold.
pub(crate) fn stored(compaction: &Compaction, kept: Option<&Fresh>) -> Option<String> {
    compaction.fresh.clone().or_else(|| {
        kept.filter(|fresh| fresh.compaction_id == compaction.compaction_id)
            .map(|fresh| fresh.text.clone())
    })
}

/// Reads the facts of the block now: the scope resolver gives the project root and its
/// git status, the toolbox the hidden shell's directory and jobs (else `agent_cwd`,
/// from the log), the disk the `AGENTS.md` files.
pub(crate) async fn read(
    deps: &ConversationDeps,
    conversation_id: ConversationId,
    cwd: &Path,
    agent_cwd: Option<PathBuf>,
) -> FreshFacts {
    let derivation = deps.scope.resolve(cwd).await;
    let registered = match &derivation.scope {
        Scope::Project(id) => {
            let engine = std::sync::Arc::clone(&deps.engine.borrow());
            engine.locations().project_root(id).map(Path::to_path_buf)
        }
        _ => None,
    };
    let root = registered.or_else(|| derivation.repo.as_ref().map(|repo| repo.root.clone()));
    let git_status = match &root {
        Some(root) => deps.scope.status(root).await,
        None => None,
    };
    let shell_cwd = match deps.toolbox.shell_cwd(conversation_id).await {
        Some(dir) => Some(dir),
        None => agent_cwd,
    };
    let jobs = deps.toolbox.jobs(conversation_id).await;
    let dirs = chain(root.as_deref(), cwd);
    let agents = tokio::task::spawn_blocking(move || read_agents(&dirs)).await.unwrap_or_default();
    FreshFacts { cwd: cwd.to_path_buf(), shell_cwd, jobs, root, git_status, agents }
}

/// The directories from `root` down to `cwd`, or `cwd` alone when it is not inside
/// `root`.
pub(crate) fn chain(root: Option<&Path>, cwd: &Path) -> Vec<PathBuf> {
    let Some(root) = root else {
        return vec![cwd.to_path_buf()];
    };
    let Ok(rest) = cwd.strip_prefix(root) else {
        return vec![cwd.to_path_buf()];
    };
    let mut dirs = vec![root.to_path_buf()];
    let mut dir = root.to_path_buf();
    for component in rest.components() {
        dir.push(component);
        dirs.push(dir.clone());
    }
    dirs
}

/// The `AGENTS.md` file of each of `dirs` that can be read, each cut at
/// [`AGENTS_MAX_BYTES`].
fn read_agents(dirs: &[PathBuf]) -> Vec<(PathBuf, String)> {
    dirs.iter()
        .filter_map(|dir| {
            let path = dir.join(AGENTS_FILE);
            let file = std::fs::File::open(&path).ok()?;
            if !file.metadata().ok()?.is_file() {
                return None;
            }
            let mut bytes = Vec::new();
            let limit = u64::try_from(AGENTS_MAX_BYTES).unwrap_or(u64::MAX) + 1;
            file.take(limit).read_to_end(&mut bytes).ok()?;
            let cut = bytes.len() > AGENTS_MAX_BYTES;
            bytes.truncate(AGENTS_MAX_BYTES);
            let mut text = String::from_utf8_lossy(&bytes).into_owned();
            if cut {
                text.push_str("\n[efr cut this file at 32 KiB]");
            }
            Some((path, text))
        })
        .collect()
}

#[cfg(test)]
mod tests;

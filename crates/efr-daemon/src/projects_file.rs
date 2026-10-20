//! `<runtime root>/projects.tsv`: the registered projects for the zsh plugin's scope
//! marker (DV52).
//!
//! The plugin shows the scope of the next prompt before the user sends it, and it may
//! start no program for that: no `efr`, no git. So efrd writes the registry in a form
//! that zsh reads with no fork: one line per project, `<root>\t<name>`, the deepest root
//! first, mode 0600, by an atomic rename. efrd writes it at start, after
//! `admin.project_add` and `admin.project_remove`, and at a prompt whose registry read
//! differs from the file (a hand edit of `projects.toml`). The plugin reads it again
//! only when its time or its size changed.
//!
//! A root with a tab, a newline or a carriage return cannot be one field of a line, so
//! it is left out: the marker then says "no project" there, and efrd's head line on the
//! reply stays the truth. Such characters in a name become blanks.

use std::path::{Path, PathBuf};

use efr_protocol::RouteScope;
use efr_scope::Registry;

use crate::routing;

/// The file's name in the runtime root.
pub(crate) const FILE: &str = "projects.tsv";

/// Where the file of the runtime root `runtime` is.
pub(crate) fn path(runtime: &Path) -> PathBuf {
    runtime.join(FILE)
}

/// The file's text for `registry`: one line per project, `<root>\t<name>\n`, the
/// deepest root first, so the first root that holds a directory is its project.
pub(crate) fn text(registry: &Registry) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt as _;
    let mut projects: Vec<(&Path, String)> = registry
        .projects()
        .iter()
        .filter(|project| {
            !project.root().as_os_str().as_bytes().iter().any(|byte| b"\t\n\r".contains(byte))
        })
        .map(|project| {
            let label =
                routing::label(registry, &RouteScope::Project(project.id()), project.root());
            let name = label.name.unwrap_or_default().replace(['\t', '\n', '\r'], " ");
            (project.root(), name)
        })
        .collect();
    projects.sort_by(|(left, _), (right, _)| {
        right.components().count().cmp(&left.components().count()).then_with(|| left.cmp(right))
    });
    let mut text = Vec::new();
    for (root, name) in projects {
        text.extend_from_slice(root.as_os_str().as_bytes());
        text.push(b'\t');
        text.extend_from_slice(name.as_bytes());
        text.push(b'\n');
    }
    text
}

/// Writes `text` to the file at `path` unless it holds it already. Returns whether it
/// wrote. It blocks; async callers run it on the blocking pool.
pub(crate) fn sync(path: &Path, text: &[u8]) -> Result<bool, efr_stdx::StdxError> {
    if std::fs::read(path).is_ok_and(|held| held == text) {
        return Ok(false);
    }
    efr_stdx::fs::write_atomic(path, text)?;
    Ok(true)
}

/// Writes the file of the runtime root `runtime` for the registry file `registry`, as
/// it holds it now. A registry that cannot be read leaves the file as it is; a failed
/// write is logged, because the marker only shows what efrd routes by.
pub(crate) async fn refresh(registry: PathBuf, runtime: &Path) {
    let file = path(runtime);
    let written = tokio::task::spawn_blocking(move || {
        let registry = Registry::load(&registry).ok()?;
        Some(sync(&file, &text(&registry)))
    })
    .await;
    log(written.ok().flatten());
}

/// Logs the outcome of a [`sync`]: `None` when nothing was tried.
pub(crate) fn log(written: Option<Result<bool, efr_stdx::StdxError>>) {
    match written {
        Some(Ok(true)) => tracing::debug!("wrote the registered projects for the zsh plugin"),
        Some(Err(error)) => {
            tracing::warn!(error = %error, "the registered projects could not be written for the zsh plugin");
        }
        Some(Ok(false)) | None => {}
    }
}

#[cfg(test)]
mod tests;

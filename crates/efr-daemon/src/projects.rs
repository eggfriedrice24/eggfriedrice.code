//! The project registry as clients change it: `projects.list`, `admin.project_add` and
//! `admin.project_remove`, for `efr project list`, `add` and `remove`.
//!
//! The registry file belongs to `efr-scope`; the daemon is the one place that changes
//! it for a client, because the CLI may not depend on `efr-scope`. A change keeps the
//! file's comments and its link into a dotfiles repository (`efr_scope::RegistryEdit`),
//! runs one at a time, and is followed by a reload, so the permission engine trusts a
//! new project from the next tool call on. The model reaches them only by running
//! `efr` in its shell, which no built-in rule of any mode allows, so that asks unless
//! a rule of the user's allows it.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use efr_protocol::{AdminProjectAdd, AdminProjectRemove, ProjectId, ProjectInfo};
use efr_scope::{Discovery, Project, Registry, RegistryEdit, ScopeError};
use efr_stdx::id::uuid_v7;

use crate::DaemonError;
use crate::state::State;

/// How often a change is planned again when the file changed between its read and its
/// write.
const ATTEMPTS: usize = 3;

/// `project` as the protocol carries it.
pub(crate) fn info(project: &Project) -> ProjectInfo {
    ProjectInfo {
        id: project.id(),
        root: project.root().to_path_buf(),
        name: project.name().map(str::to_owned),
    }
}

/// Every registered project, as the file holds it now.
pub(crate) async fn list(state: &State) -> Result<Vec<ProjectInfo>, DaemonError> {
    let file = state.engine_parts.registry.clone();
    let registry = blocking("projects.list", move || {
        Registry::load(&file).map_err(|source| DaemonError::Registry { source })
    })
    .await?;
    Ok(registry.projects().iter().map(info).collect())
}

/// Registers the project that `params` names, and returns it with the file that was
/// written.
pub(crate) async fn add(
    state: &State,
    params: AdminProjectAdd,
) -> Result<(ProjectInfo, PathBuf), DaemonError> {
    let root = root_to_add(state, &params).await?;
    let name = params
        .name
        .filter(|name| !name.trim().is_empty())
        .or_else(|| root.file_name().and_then(|name| name.to_str()).map(str::to_owned));
    let id = ProjectId::from_uuid(uuid_v7(&*state.clock, &*state.rng));
    let file = state.engine_parts.registry.clone();
    let lock = std::sync::Arc::clone(&state.registry_writes);
    blocking("admin.project_add", move || {
        change(&file, &lock, |edit| {
            edit.register(id, root.clone(), name.clone())
                .map(|project| info(&project))
                .map_err(|source| DaemonError::Registry { source })
        })
    })
    .await
}

/// Takes the project whose root is `params.path` out of the registry, and returns it
/// with the file that was written.
pub(crate) async fn remove(
    state: &State,
    params: AdminProjectRemove,
) -> Result<(ProjectInfo, PathBuf), DaemonError> {
    let path = params.path;
    if !path.is_absolute() {
        return Err(DaemonError::InvalidParams { reason: "the path must be absolute" });
    }
    let file = state.engine_parts.registry.clone();
    let lock = std::sync::Arc::clone(&state.registry_writes);
    blocking("admin.project_remove", move || {
        // NOTE: the root is registered with links resolved, and the user may name it
        // through a link, so a path that matches no root is tried in its real form too.
        let real = std::fs::canonicalize(&path).ok();
        change(&file, &lock, |edit| {
            let registry = |source| DaemonError::Registry { source };
            let mut removed = edit.remove_root(&path).map_err(registry)?;
            if removed.is_none()
                && let Some(real) = &real
            {
                removed = edit.remove_root(real).map_err(registry)?;
            }
            match removed {
                Some(project) => Ok(info(&project)),
                None => Err(DaemonError::ProjectNotRegistered { path: path.clone() }),
            }
        })
    })
    .await
}

/// The root that `params` registers: the directory with links resolved, or with
/// `git_root` the root of the git work tree that holds it, else the directory.
async fn root_to_add(state: &State, params: &AdminProjectAdd) -> Result<PathBuf, DaemonError> {
    if !params.path.is_absolute() {
        return Err(DaemonError::InvalidParams { reason: "the path must be absolute" });
    }
    let path = params.path.clone();
    let dir = blocking("admin.project_add", move || {
        Ok(std::fs::canonicalize(&path).ok().filter(|real| real.is_dir()))
    })
    .await?;
    let Some(dir) = dir else {
        return Err(DaemonError::ProjectRootMissing { path: params.path.clone() });
    };
    if !params.git_root {
        return Ok(dir);
    }
    let home = &state.engine_parts.home;
    let root = match state.git.discover(&dir, home).await {
        Ok(Discovery::WorkTree(repo)) => repo.root,
        Ok(Discovery::NotARepository | Discovery::Guarded { .. }) => dir,
        Err(source) => return Err(DaemonError::Registry { source }),
    };
    // NOTE: a project lets the auto mode write freely below its root, so the home
    // directory or `/` is a project only when the user names it, never because a
    // command ran there.
    if home.is_at_or_above(&root) {
        return Err(DaemonError::ProjectRootTooWide { root });
    }
    Ok(root)
}

/// Applies `plan` to the registry file at `file` and writes it, one change at a time,
/// planning again when the file changed in between. Returns what `plan` returned and
/// the file that was written. It blocks; async callers run it in `spawn_blocking`.
fn change<T>(
    file: &Path,
    lock: &Mutex<()>,
    mut plan: impl FnMut(&mut RegistryEdit) -> Result<T, DaemonError>,
) -> Result<(T, PathBuf), DaemonError> {
    // The lock guards no data, so a poisoned one still serializes the writes.
    let _writing = lock.lock().unwrap_or_else(PoisonError::into_inner);
    let mut attempt = 0;
    loop {
        attempt += 1;
        let mut edit =
            RegistryEdit::open(file).map_err(|source| DaemonError::Registry { source })?;
        let value = plan(&mut edit)?;
        match edit.save() {
            Ok(()) => return Ok((value, edit.target().to_path_buf())),
            Err(ScopeError::RegistryChanged { .. }) if attempt < ATTEMPTS => {}
            Err(source) => return Err(DaemonError::Registry { source }),
        }
    }
}

/// Runs `work` on the blocking pool.
async fn blocking<T: Send + 'static>(
    task: &'static str,
    work: impl FnOnce() -> Result<T, DaemonError> + Send + 'static,
) -> Result<T, DaemonError> {
    tokio::task::spawn_blocking(work).await.map_err(|_| DaemonError::TaskPanicked { task })?
}

#[cfg(test)]
mod tests;

//! `sandbox.explain`: what a contained call can do with one path, and why, from the
//! same plan that the launcher builds (efr's auto spec, section 13.6).

use std::path::{Path, PathBuf};

use efr_config::Settings;
use efr_permissions::Engine;
use efr_protocol::{
    CallId, ConversationId, ExitKind, Launch, Mode, SandboxExplainResult, SandboxPathRole,
};
use efr_sandbox::{
    FloorKind, MaskKind, MountOrigin, MountPlan, WriteRootKind, is_within, resolve, too_wide,
};

use crate::DaemonError;
use crate::sandbox::fs::{DaemonFs, WithAssets};
use crate::sandbox::{SandboxService, plan};

/// The bubblewrap program a plan names when the probe found none; the plan never runs.
const DEFAULT_BWRAP: &str = "/usr/bin/bwrap";

impl SandboxService {
    /// What a contained call of a turn in `cwd` can do with `path`.
    pub(crate) async fn explain(
        &self,
        path: &Path,
        cwd: Option<&Path>,
        settings: &Settings,
        engine: &Engine,
    ) -> Result<SandboxExplainResult, DaemonError> {
        let path = match (path.is_absolute(), cwd) {
            (true, _) => path.to_path_buf(),
            (false, Some(cwd)) if cwd.is_absolute() => cwd.join(path),
            _ => {
                return Err(DaemonError::InvalidParams {
                    reason: "the path must be absolute, or cwd must be given",
                });
            }
        };
        let home = self.home().path().to_path_buf();
        let projects = self.projects().await;
        let turn_project = cwd.and_then(|cwd| {
            projects
                .iter()
                .filter(|root| cwd.starts_with(root.as_path()) && !too_wide(root, &home))
                .max_by_key(|root| root.as_os_str().len())
                .cloned()
        });
        let protected_config = {
            let dir = self.inner.dirs.config().to_path_buf();
            tokio::task::spawn_blocking(move || crate::engine::protected_config(&dir))
                .await
                .unwrap_or_default()
        };
        let secrets = engine.locations().secret_paths();
        let status = self.current();
        let bwrap = status.bwrap.clone().unwrap_or_else(|| PathBuf::from(DEFAULT_BWRAP));
        let id = efr_stdx::id::uuid_v7(&*self.inner.clock, &*self.inner.rng);
        let conversation = ConversationId::from_uuid(id);
        let call = CallId::from_uuid(id);
        let scratch = self.inner.dirs.data().join(crate::state::SCRATCH_DIR).join("explain");
        let launch = Launch::contained();
        let named_paths: Vec<PathBuf> =
            std::iter::once(path.clone()).chain(cwd.map(Path::to_path_buf)).collect();
        let input = plan::PlanInput {
            conversation,
            call,
            launch: &launch,
            turn_project: turn_project.as_deref(),
            projects: &projects,
            named_paths: &named_paths,
            scratch: &scratch,
            settings: &settings.sandbox,
            secrets: &secrets,
            protected_config: &protected_config,
            host: &self.inner.host,
            dirs: &self.inner.dirs,
            home: &home,
            bwrap: &bwrap,
            cache_mode: status.cache_mode,
            launcher: &self.inner.copy,
        };
        let spec = plan::build(&input).spec;
        let runtime = spec.runtime.clone();
        let target = path.clone();
        let explained = tokio::task::spawn_blocking(move || {
            // NOTE: the launcher's files exist per call or from the first shell start;
            // the answer must not depend on that.
            let view = WithAssets {
                inner: &DaemonFs,
                assets: vec![
                    runtime.launcher.clone(),
                    runtime.child_script.clone(),
                    runtime.call_dir.join(efr_sandbox::LINE_FILE),
                ],
                dirs: std::iter::once(runtime.private_tmp())
                    .chain(
                        spec.caches
                            .iter()
                            .flat_map(|cache| [cache.upper.clone(), cache.work.clone()]),
                    )
                    .collect(),
            };
            let plan = MountPlan::build(&spec, &view)?;
            let real = resolve(&DaemonFs, &target)?.path;
            let mut explanation = plan.explain(&real);
            // NOTE: the plan mounts a floor only inside a writable place; elsewhere the
            // path is read-only anyway, but the answer names why a grant cannot open it.
            if explanation.origin.is_none() {
                for floor in &spec.floors {
                    let floor_real = resolve(&DaemonFs, &floor.path)?.path;
                    if is_within(&real, &floor.path) || is_within(&real, &floor_real) {
                        explanation.role = SandboxPathRole::Floor;
                        explanation.origin = Some(MountOrigin::Floor(floor.kind));
                        break;
                    }
                }
            }
            Ok::<_, efr_sandbox::SandboxError>((real, explanation))
        })
        .await
        .map_err(|_| DaemonError::TaskPanicked { task: "sandbox.explain" })?
        .map_err(|source| DaemonError::SandboxSpec { source })?;
        let (real, explanation) = explained;
        let (reason, write_exit) = why(explanation.role, explanation.origin);
        Ok(SandboxExplainResult {
            path: real,
            project: turn_project,
            mode: Mode::Auto,
            role: explanation.role,
            read: explanation.read,
            write: explanation.write,
            reason: reason.to_owned(),
            write_exit,
        })
    }
}

/// The one-sentence reason of a path's `role` from the mount `origin` that decides it,
/// and the exit that a write of it would be.
pub(crate) fn why(
    role: SandboxPathRole,
    origin: Option<MountOrigin>,
) -> (&'static str, Option<ExitKind>) {
    let floor = |reason| (reason, Some(ExitKind::Persistence));
    match origin {
        None => ("outside every write root, like the rest of the system", Some(ExitKind::Write)),
        Some(MountOrigin::Runtime) => ("a runtime dir of your session or of efr (hidden)", None),
        Some(MountOrigin::EfrState) => ("efr's own data or state (hidden)", None),
        Some(MountOrigin::Mask(MaskKind::EngineSecret)) => {
            ("a secret (hidden; no approval opens it)", Some(ExitKind::Secret))
        }
        Some(MountOrigin::Mask(MaskKind::ProjectEnv)) => {
            ("a project .env file (hidden; a read is a masked_read exit)", Some(ExitKind::Write))
        }
        Some(MountOrigin::Mask(_)) => (
            "a credential store, profile or history (hidden; a read is a masked_read exit)",
            Some(ExitKind::Write),
        ),
        Some(MountOrigin::Cache) => ("a tool cache (writes go to a private copy)", None),
        Some(MountOrigin::WriteRoot(kind)) => (root_reason(kind), None),
        Some(MountOrigin::PrivateTmp | MountOrigin::SharedMemory) => {
            ("the private /tmp of the conversation", None)
        }
        Some(MountOrigin::Pin) => ("the git dir of a project (write root)", None),
        Some(MountOrigin::Floor(FloorKind::Config)) => {
            ("efr's config (floor; no approval writes it)", Some(ExitKind::Config))
        }
        Some(MountOrigin::Floor(kind)) => floor(floor_reason(kind)),
        Some(MountOrigin::Asset) => ("a file of efr's sandbox (read only)", None),
        Some(_) if role == SandboxPathRole::ReadOnly => {
            ("a socket or device of an approval (read only)", None)
        }
        Some(_) => ("a part of the sandbox", None),
    }
}

fn root_reason(kind: WriteRootKind) -> &'static str {
    match kind {
        WriteRootKind::TurnProject => "in the turn's project (write root)",
        WriteRootKind::NamedProject => {
            "in a registered project that the command names (write root)"
        }
        WriteRootKind::GitDir | WriteRootKind::GitCommonDir => {
            "in the git dir of a worktree project (write root)"
        }
        WriteRootKind::Scratch => "in $SCRATCH (write root)",
        WriteRootKind::UserConfigured => "in a root of sandbox.write_roots",
        WriteRootKind::Grant => "in an approved write of this call",
        _ => "in a write root",
    }
}

fn floor_reason(kind: FloorKind) -> &'static str {
    match kind {
        FloorKind::Config => "efr's config (floor)",
        FloorKind::ShellStartup => "a shell startup file (floor)",
        FloorKind::Autostart => "an autostart or service dir (floor)",
        FloorKind::PathDir => "a directory on the hidden shell's PATH (floor)",
        FloorKind::ToolConfig => "a tool config that runs code (floor)",
        FloorKind::LinkTarget => "the target of a link in your home directory (floor)",
        FloorKind::EfrBinary => "an efr program (floor)",
        FloorKind::GitConfig | FloorKind::GitHooks | FloorKind::GitFile => {
            "a git setting that runs programs (floor)"
        }
        FloorKind::ProtectedName => "an agent or editor config in a write root (floor)",
        FloorKind::User => "a path of sandbox.protect (floor)",
        _ => "read only even in a write root (floor)",
    }
}

#[cfg(test)]
mod tests;

//! What comes back from a call: the records filtered for the trusted shell (the spec's
//! section 6), the sandbox state for later contained calls, and the summary.
//!
//! The records are untrusted: the model's code can write fd 3 too. Only a `cd` into a
//! directory the trusted shell can enter, and exports that pass every check of
//! `ExportFilter`, reach `$CALL/apply`. A cwd in the private tmp stays in the sandbox
//! state as `sandbox_cwd`. Functions and aliases stay in the state and never return.

use std::path::{Path, PathBuf};

use efr_protocol::{SandboxPathRole, SandboxSummary};
use efr_sandbox::{
    ExportFilter, ExportVerdict, FsView, MountPlan, Records, SandboxCwd, SandboxSpec, SandboxState,
    is_within, resolve,
};

/// What the trusted shell gets from one call.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Promotion {
    /// The directory to `cd` to.
    pub(crate) cd: Option<PathBuf>,
    /// The exports that pass every check.
    pub(crate) exports: Vec<(String, String)>,
    /// The unsets of promotable names.
    pub(crate) unsets: Vec<String>,
}

/// Where a call ended, as the launcher sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FinalCwd {
    /// A directory the trusted shell can enter.
    Host(PathBuf),
    /// A directory of the private tmp, which only later contained calls have.
    Private {
        /// The path inside the sandbox.
        inside: PathBuf,
        /// The same directory outside.
        host: PathBuf,
    },
    /// No usable cwd: the shell stays where it is.
    Stay,
}

/// True when `path` lies below `/tmp` or `/var/tmp`, which only the sandbox has.
fn in_tmp(path: &Path) -> bool {
    is_within(path, Path::new("/tmp")) || is_within(path, Path::new("/var/tmp"))
}

/// Judges the cwd record of a contained call.
pub(crate) fn contained_cwd(cwd: &Path, plan: &MountPlan, fs: &dyn FsView) -> FinalCwd {
    if in_tmp(cwd) {
        return match plan.private_host_path(cwd) {
            Some(host) if fs.lstat(&host).ok() == Some(efr_sandbox::FileKind::Dir) => {
                FinalCwd::Private { inside: cwd.to_path_buf(), host }
            }
            _ => FinalCwd::Stay,
        };
    }
    let Ok(resolved) = resolve(fs, cwd) else { return FinalCwd::Stay };
    let masked = plan.explain(&resolved.path).role == SandboxPathRole::Masked;
    if masked || resolved.kind != Some(efr_sandbox::FileKind::Dir) {
        return FinalCwd::Stay;
    }
    FinalCwd::Host(cwd.to_path_buf())
}

/// Judges the cwd record of the exit child, which ran with the user's rights: any
/// directory outside efr's own roots and the masks.
pub(crate) fn exit_child_cwd(cwd: &Path, spec: &SandboxSpec, fs: &dyn FsView) -> FinalCwd {
    let runtime = &spec.runtime;
    let hidden = [&runtime.user_runtime, &runtime.runtime, &runtime.data, &runtime.state]
        .into_iter()
        .chain(spec.masks.iter().map(|mask| &mask.path))
        .any(|root| is_within(cwd, root));
    let is_dir =
        resolve(fs, cwd).is_ok_and(|resolved| resolved.kind == Some(efr_sandbox::FileKind::Dir));
    if hidden || in_tmp(cwd) || !is_dir {
        FinalCwd::Stay
    } else {
        FinalCwd::Host(cwd.to_path_buf())
    }
}

/// The export filter of an exit child: every place a contained call of this
/// conversation can write counts as a root, without a plan.
pub(crate) fn exit_child_filter(spec: &SandboxSpec, cwd: &Path) -> ExportFilter {
    let mut roots: Vec<PathBuf> = spec.write_roots.iter().map(|root| root.path.clone()).collect();
    roots.extend(spec.caches.iter().map(|cache| cache.target.clone()));
    roots.extend(spec.grants.iter().filter_map(|grant| grant.path()).map(Path::to_path_buf));
    roots.push(spec.runtime.scratch.clone());
    roots.push(spec.runtime.sandbox_dir.clone());
    ExportFilter::new(
        spec.env.promote.clone(),
        spec.env.export_deny.clone(),
        roots,
        spec.runtime.home.clone(),
        cwd.to_path_buf(),
        spec.limits.max_value,
    )
}

/// Filters the exports and unsets of `records`, and notes each name in `summary`.
pub(crate) fn promote(
    records: &Records,
    cd: Option<PathBuf>,
    filter: &ExportFilter,
    fs: &dyn FsView,
    summary: &mut SandboxSummary,
) -> Promotion {
    let mut promotion = Promotion { cd, ..Promotion::default() };
    for (name, value) in &records.exports {
        match filter.check_resolving(name, value, fs) {
            ExportVerdict::Promote => {
                promotion.exports.push((name.clone(), value.clone()));
                summary.promoted.push(name.clone());
            }
            ExportVerdict::Drop => summary.dropped.push(name.clone()),
            _ => summary.kept_out.push(name.clone()),
        }
    }
    for name in &records.unsets {
        if filter.check(name, "") == ExportVerdict::Promote {
            promotion.unsets.push(name.clone());
        }
    }
    promotion
}

/// Lays a contained call's records over the state and records where it ended.
pub(crate) fn update_state(
    state: &mut SandboxState,
    records: &Records,
    cwd: &FinalCwd,
    shell_pwd: &Path,
) {
    state.apply(records);
    state.sandbox_cwd = match cwd {
        FinalCwd::Private { inside, .. } => {
            Some(SandboxCwd { path: inside.clone(), shell_pwd: shell_pwd.to_path_buf() })
        }
        FinalCwd::Host(_) | FinalCwd::Stay => None,
    };
}

#[cfg(test)]
mod tests;

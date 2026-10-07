//! [`ExportFilter`]: which exports of a contained call return to the trusted shell
//! (the spec's section 6.3).
//!
//! An export returns only when every check passes: a shell name, on the promote list,
//! not on the never list, not secret-like, a value without control characters and of
//! bounded length, and no part of the value that resolves into a place that the
//! sandbox can write. Everything else stays in the sandbox's state for later contained
//! calls, except the names of [`OVERLAY_DENY`], which are dropped everywhere.

use std::path::{Path, PathBuf};

use crate::fs_view::{FsView, resolve};
use crate::names::{any_matches, is_variable_name, secret_like};
use crate::paths::{expand_home, is_within, normalize};
use crate::plan::MountPlan;
use crate::spec::SandboxSpec;

/// Names that never return to the trusted shell, whatever the promote list says.
pub const NEVER_PROMOTE: &[&str] = &[
    "PATH",
    "MANPATH",
    "FPATH",
    "CDPATH",
    "HOME",
    "SHELL",
    "ZDOTDIR",
    "ENV",
    "BASH_ENV",
    "IFS",
    "PS0",
    "PS1",
    "PS2",
    "PS3",
    "PS4",
    "PS5",
    "PS6",
    "PS7",
    "PS8",
    "PS9",
    "PROMPT*",
    "RPROMPT*",
    "HISTFILE",
    "TMPDIR",
    "LD_*",
    "DYLD_*",
    "*_PRELOAD",
    "*_COMMAND",
    "GIT_*",
    "SSH_*",
    "GPG_*",
    "GNUPGHOME",
    "DBUS_*",
    "XDG_*",
    "*_proxy",
    "*_PROXY",
    "PYTHON*",
    "PERL5*",
    "RUBY*",
    "JAVA_TOOL_OPTIONS",
    "_JAVA_OPTIONS",
    "CLASSPATH",
    "GCONV_PATH",
    "LOCPATH",
    "EDITOR",
    "VISUAL",
    "*_EDITOR",
    "*PAGER",
    "*ASKPASS*",
    "CARGO_*",
    "RUSTUP_*",
    "RUSTC*",
    "RUSTFLAGS",
    "RUSTDOCFLAGS",
    "CC",
    "CXX",
    "LD",
    "AR",
    "MAKEFLAGS",
    "GO*",
    "npm_config_*",
    "NPM_CONFIG_*",
    "PIP_*",
    "UV_*",
    "GEM_*",
    "BUNDLE_*",
    "KUBECONFIG",
    "DOCKER_*",
    "SUDO_*",
    "EFR_*",
    "_EFR_*",
];

/// `NODE_*` never returns, except `NODE_ENV`.
const NODE_PREFIX: &str = "NODE_";

/// Names dropped everywhere: they never return and never stay in the sandbox's state,
/// so a rejected `LD_PRELOAD` cannot reach later contained calls.
pub const OVERLAY_DENY: &[&str] = &[
    "LD_*",
    "BASH_ENV",
    "ENV",
    "ZDOTDIR",
    "HOME",
    "TMPDIR",
    "XDG_RUNTIME_DIR",
    "EFR_*",
    "_EFR_*",
    "*_proxy",
    "*_PROXY",
    "NO_PROXY",
    "no_proxy",
];

/// True when `name` is dropped everywhere ([`OVERLAY_DENY`]).
pub fn overlay_denied(name: &str) -> bool {
    any_matches(OVERLAY_DENY, name)
}

/// What happens to one export.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ExportVerdict {
    /// It returns to the trusted shell.
    Promote,
    /// It stays in the sandbox's state; the tool result names it.
    KeepInSandbox(KeepReason),
    /// It is dropped everywhere ([`OVERLAY_DENY`]).
    Drop,
}

/// Why an export stays in the sandbox.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum KeepReason {
    /// The name is not a shell variable name.
    BadName,
    /// The name is on the never list.
    NeverList,
    /// The name is not on the promote list.
    NotListed,
    /// The name looks like a secret.
    SecretName,
    /// The value has a control character or is too long.
    BadValue,
    /// A part of the value points into a place that the sandbox can write.
    ValueInRoot,
}

/// The export filter of one call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportFilter {
    promote: Vec<String>,
    deny: Vec<String>,
    roots: Vec<PathBuf>,
    home: PathBuf,
    cwd: PathBuf,
    max_value: usize,
}

impl ExportFilter {
    /// A filter with these lists and places: `roots` are every place the sandbox can
    /// write, as the host names them; `cwd` is the call's final directory, against
    /// which a value part that starts with `.` is read.
    pub fn new(
        promote: Vec<String>,
        export_deny: Vec<String>,
        roots: Vec<PathBuf>,
        home: PathBuf,
        cwd: PathBuf,
        max_value: usize,
    ) -> ExportFilter {
        ExportFilter { promote, deny: export_deny, roots, home, cwd, max_value }
    }

    /// The filter of `spec`'s call under `plan`: the write roots, the private tmp
    /// (outside and inside), scratch and the cache overlays.
    pub fn from_spec(spec: &SandboxSpec, plan: &MountPlan, cwd: &Path) -> ExportFilter {
        let mut roots: Vec<PathBuf> = plan.write_dirs().to_vec();
        roots.push(plan.private_tmp().to_path_buf());
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

    /// The verdict on `name=value`, reading the value's paths lexically.
    pub fn check(&self, name: &str, value: &str) -> ExportVerdict {
        self.verdict(name, value, None)
    }

    /// The verdict on `name=value`, also following the links of each path in the
    /// value through `fs`.
    pub fn check_resolving(&self, name: &str, value: &str, fs: &dyn FsView) -> ExportVerdict {
        self.verdict(name, value, Some(fs))
    }

    fn verdict(&self, name: &str, value: &str, fs: Option<&dyn FsView>) -> ExportVerdict {
        if overlay_denied(name) {
            return ExportVerdict::Drop;
        }
        let keep = ExportVerdict::KeepInSandbox;
        if !is_variable_name(name) {
            return keep(KeepReason::BadName);
        }
        let never = any_matches(NEVER_PROMOTE, name)
            || (name.starts_with(NODE_PREFIX) && name != "NODE_ENV")
            || any_matches(&self.deny, name);
        if never {
            return keep(KeepReason::NeverList);
        }
        if !any_matches(&self.promote, name) {
            return keep(KeepReason::NotListed);
        }
        if secret_like(name) {
            return keep(KeepReason::SecretName);
        }
        if value.len() > self.max_value || value.chars().any(|c| c.is_control() && c != '\t') {
            return keep(KeepReason::BadValue);
        }
        if self.points_into_roots(value, fs) {
            return keep(KeepReason::ValueInRoot);
        }
        ExportVerdict::Promote
    }

    fn points_into_roots(&self, value: &str, fs: Option<&dyn FsView>) -> bool {
        let parts = value.split(|c: char| c == ':' || c == '=' || c.is_whitespace());
        for part in parts.filter(|part| part.starts_with(['/', '~', '.'])) {
            let path = expand_home(Path::new(part), &self.home);
            let Some(path) = normalize(&self.cwd.join(path)) else { continue };
            if self.in_roots(&path) {
                return true;
            }
            if let Some(fs) = fs
                && let Ok(resolved) = resolve(fs, &path)
                && self.in_roots(&resolved.path)
            {
                return true;
            }
        }
        false
    }

    fn in_roots(&self, path: &Path) -> bool {
        is_within(path, Path::new("/tmp"))
            || is_within(path, Path::new("/var/tmp"))
            || self.roots.iter().any(|root| is_within(path, root))
    }
}

#[cfg(test)]
mod tests;

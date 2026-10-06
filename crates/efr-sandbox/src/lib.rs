//! The pure logic of the `auto` sandbox: the spec of one call, the mount plan and its
//! bwrap arguments, Landlock and seccomp as data, the environment and export filters,
//! the records of a call, the sandbox state, the result file, the surface guard, the
//! worktree record and the probe's result types.
//!
//! efrd builds a [`SandboxSpec`] per call from plain paths; `efr-sbx` checks it, builds
//! the [`MountPlan`] and launches bwrap with it. Every rule lives here, so efrd's
//! early check and the launcher agree, and every rule is tested without a kernel.
//!
//! Allowed dependencies: `efr-protocol` (the wire types: grants, the summary, the
//! status), `serde`, `serde_json` and `thiserror`. What does not belong here: IO
//! outside [`FsView`], processes, tokio, code outside safe Rust, and the secret
//! tables (efrd passes their paths).

// NOTE: missing_docs is set here and not in Cargo.toml, because Cargo rejects a
// `[lints]` table that both inherits the workspace lints and adds its own.
#![warn(missing_docs)]

mod args;
mod env_filter;
mod error;
mod export_filter;
mod fs_view;
mod git_config;
mod inner;
mod landlock;
mod names;
mod paths;
mod plan;
mod probe;
mod records;
mod result;
mod seccomp;
mod spec;
mod state;
mod surface;
#[cfg(test)]
mod testing;
mod worktree;

pub use args::{FdTable, LaunchFds, encode_args};
pub use env_filter::{EnvFilter, HIDDEN_SHELL_SCRUB, MOVED_CACHES, OFFLINE_HINTS};
pub use error::SandboxError;
pub use export_filter::{
    ExportFilter, ExportVerdict, KeepReason, NEVER_PROMOTE, OVERLAY_DENY, overlay_denied,
};
pub use fs_view::{FileKind, FsView, MAX_LINKS, Resolved, resolve};
pub use git_config::{cargo_code_keys, code_keys, is_code_key, parse_config};
pub use inner::{InnerPolicy, MAX_POLICY_BYTES, RECORDS_FD};
pub use landlock::{
    ERRATUM_DISCONNECTED_DIRS, FsAccess, LandlockPolicy, LandlockRule, LandlockScope,
    MIN_LANDLOCK_ABI,
};
pub use names::{
    EFR_NAMES, PROXY_NAMES, SECRET_NAMES, SOCKET_NAMES, is_variable_name, matches as name_matches,
    secret_like,
};
pub use paths::{depth, expand_home, is_normal, is_within, normalize, too_wide};
pub use plan::{Explanation, Mount, MountOp, MountOrigin, MountPlan, OpKind, PlanNote, StartDir};
pub use probe::{
    BWRAP_REQUIRED_FLAGS, BwrapFacts, HOME_PROJECT_FIX, HOME_PROJECT_REASON, ProbeFailure,
    ProbeReport, SELF_TEST_TCP_ADDR, bwrap_missing_flags, check_bwrap, check_landlock,
    parse_bwrap_version,
};
pub use records::{RECORDS_HEADER, Records, encode_apply, encode_records, parse_records};
pub use result::{
    APPLY_FILE, LINE_FILE, MAX_RESULT_BYTES, NONCE_FILE, RESULT_FILE, SETUP_FAILURE_STATUS,
    SNAPSHOT_FILE, SPEC_FILE, STARTED_FILE, STATE_JSON_FILE, STATE_ZSH_FILE, SandboxResult,
    nonce_hex, parse_nonce_hex,
};
pub use seccomp::{
    AF_INET, AF_INET6, AF_NETLINK, AF_UNIX, CLONE_NEW_MASK, EAFNOSUPPORT, ENOSYS, EPERM,
    SeccompAction, SeccompArch, SeccompProfile, SyscallRule, TIOCLINUX, TIOCSTI,
};
pub use spec::{
    CacheOverlay, EnvPlan, Floor, FloorKind, MAX_SPEC_BYTES, Mask, MaskKind, NetworkPlan,
    RecordLimits, RuntimePaths, SPEC_VERSION, SandboxSpec, SpecLaunch, WriteRoot, WriteRootKind,
    layer_name,
};
pub use state::{MAX_STATE_BYTES, STATE_NAME_VAR, SandboxCwd, SandboxState, quote};
pub use surface::{
    ConfigLister, GitDirTarget, MAX_SURFACE_FILE, SCAN_SKIP, ScanLimits, SurfaceManifest,
    SurfaceRule, check_surface, report_file, scan_git_dirs,
};
pub use worktree::{
    WorktreeCheck, WorktreeRecord, check_worktree, parse_git_file, read_worktree, record_file_name,
};

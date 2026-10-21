//! The pure logic of the `auto` sandbox: the spec of one call, the mount plan and its
//! bwrap arguments, Landlock and seccomp as data, the environment and export filters,
//! the records of a call, the sandbox state, the result file, the cache deletes, the
//! surface guard, the worktree record and the probe's result types.
//!
//! efrd builds a [`SandboxSpec`] per call from plain paths; `efr-sbx` checks it, builds
//! the [`MountPlan`] and launches bwrap with it. Every rule lives here, so efrd's
//! early check and the launcher agree, and every rule is tested without a kernel.
//!
//! Allowed dependencies: `efr-protocol` (the wire types: grants, the summary, the
//! status), `serde`, `serde_json` and `thiserror`. What does not belong here: IO
//! outside [`FsView`] and [`LayerView`], processes, tokio, code outside safe Rust, and
//! the secret tables (efrd passes their paths).

// NOTE: missing_docs is set here and not in Cargo.toml, because Cargo rejects a
// `[lints]` table that both inherits the workspace lints and adds its own.
#![warn(missing_docs)]

mod args;
mod desktop;
mod env_filter;
mod error;
mod export_filter;
mod forward;
mod fs_view;
mod git_config;
mod inner;
mod landlock;
mod layers;
mod modules;
mod mounts;
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
pub use desktop::{HYPRLAND_SOCKET, X11_SOCKET_DIR, desktop_socket_path, is_desktop_socket};
pub use env_filter::{EnvFilter, HIDDEN_SHELL_SCRUB, MOVED_CACHES, OFFLINE_HINTS};
pub use error::SandboxError;
pub use export_filter::{
    ExportFilter, ExportVerdict, KeepReason, NEVER_PROMOTE, OVERLAY_DENY, overlay_denied,
};
pub use forward::{
    BRIDGE_PORT, Change, Debounce, FORWARD_VERSION, FRAME_LEN, Frame, FrameKind, Listen,
    MAX_DAEMON_FORWARDS, MAX_FORWARD_CONNECTIONS, MAX_JOB_FORWARDS, MAX_REPORTED_PORTS, MIN_PORT,
    PortRefusal, SERVICE_PORTS, TCP_LISTEN, TCP_TIME_WAIT, TcpRow, loopback_listens,
    parse_tcp_table, peer_uid, refusal, v6_holds,
};
pub use fs_view::{FileKind, FsView, MAX_LINKS, Resolved, resolve};
pub use git_config::{cargo_code_keys, code_keys, is_code_key, parse_config};
pub use inner::{InnerForward, InnerPolicy, LayersSync, MAX_POLICY_BYTES, RECORDS_FD};
pub use landlock::{
    BUS_PROXY_FILES, BUS_PROXY_READS, ERRATUM_DISCONNECTED_DIRS, FsAccess, LandlockPolicy,
    LandlockRule, LandlockScope, MIN_LANDLOCK_ABI,
};
pub use layers::{
    CacheDeletes, CacheLayer, CacheLayers, DeleteLimits, LayerEntry, LayerView, MAX_DELETE_ENTRIES,
    MAX_LAYERS_BYTES, STAGING_DIR,
};
pub use modules::{MAX_MODULE_DEPTH, module_git_dirs};
pub use mounts::{
    MAX_MOUNTS_BYTES, MAX_STATVFS_PATH_BYTES, MAX_STATVFS_PATHS, MOUNTS_FILE, MountEntry,
    MountSizes, MountSnapshot, SIZES_FILE, StatvfsLine, StatvfsValues, is_dummy, is_masked,
    is_other_users, parse_mountinfo, render_df, render_findmnt_df,
};
pub use names::{
    EFR_NAMES, GIT_CONFIG_NAMES, PROXY_NAMES, SECRET_NAMES, SOCKET_NAMES, has_url_password,
    is_variable_name, matches as name_matches, secret_like,
};
pub use paths::{depth, expand_home, is_normal, is_within, normalize, too_wide};
pub use plan::{
    Explanation, Mount, MountOp, MountOrigin, MountPlan, NO_PROXY, NO_PROXY_VARIABLES, OpKind,
    PROXY_PORT, PROXY_URL, PROXY_VARIABLES, PlanNote, SERVICE_SOCKETS, SYSTEM_BUS_SOCKET, StartDir,
};
pub use probe::{
    BWRAP_REQUIRED_FLAGS, BwrapFacts, HOME_PROJECT_FIX, ProbeFailure, ProbeReport,
    SELF_TEST_TCP_ADDR, bwrap_missing_flags, check_bwrap, check_landlock, parse_bwrap_version,
};
pub use records::{RECORDS_HEADER, Records, encode_apply, encode_records, parse_records};
pub use result::{
    APPLY_FILE, ENV_FILE, GUARD_FILE, JobStop, LAUNCHER_LOG_FILE, LINE_FILE, LaunchTiming,
    MAX_ENV_BYTES, MAX_RESULT_BYTES, NONCE_FILE, RESULT_FILE, SETUP_FAILURE_STATUS, SNAPSHOT_FILE,
    SPEC_FILE, STARTED_FILE, STATE_JSON_FILE, STATE_ZSH_FILE, SURFACE_BEFORE_FILE, SandboxResult,
    TIMES_FILE, nonce_hex, parse_nonce_hex,
};
pub use seccomp::{
    AF_INET, AF_INET6, AF_NETLINK, AF_PACKET, AF_UNIX, CLONE_NEW_MASK, EAFNOSUPPORT, ENOSYS, EPERM,
    EPROTONOSUPPORT, NETLINK_ROUTE, NETLINK_SOCK_DIAG, SeccompAction, SeccompArch, SeccompProfile,
    SyscallRule, TIOCLINUX, TIOCSTI,
};
pub use spec::{
    BUS_FILTER_DIR, CacheOverlay, DesktopSocket, EnvPlan, FORWARD_SOCKET_FILE, Floor, FloorKind,
    ForwardPlan, JOB_BACKSTOP_SECS, JOB_DIR, JOB_FSIZE_BYTES, JOB_OOM_SCORE_ADJ, JobPlan,
    MAX_SPEC_BYTES, Mask, MaskKind, NetworkPlan, PROXY_SOCKET_FILE, RecordLimits, RuntimePaths,
    SPEC_VERSION, SandboxSpec, SpecLaunch, WriteRoot, WriteRootKind, bus_filter_file, layer_name,
};
pub use state::{MAX_STATE_BYTES, STATE_NAME_VAR, STOP_TRAPS, SandboxCwd, SandboxState, quote};
pub use surface::{
    CONFIG_LISTINGS_FILE, ConfigLister, ConfigListings, GitDirTarget, MAX_CONFIG_LISTINGS_BYTES,
    MAX_SURFACE_FILE, SCAN_SKIP, ScanLimits, SurfaceManifest, SurfaceRule, check_surface,
    report_file, scan_git_dirs,
};
pub use worktree::{
    WorktreeCheck, WorktreeRecord, check_worktree, parse_git_file, read_worktree, record_file_name,
};

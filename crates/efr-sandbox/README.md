# efr-sandbox

## Purpose

The pure logic of the `auto` sandbox. efrd builds a `SandboxSpec` for each contained
call from plain paths (the turn's project, the engine's secret tables, efr's roots, the
grants of an approved exit). `efr-sbx run` reads it, checks it and builds the
`MountPlan`, the bwrap arguments and the policy of the inner stage. Every rule of the
launch lives here, so efrd's early check and the launcher agree, and every rule is
tested without a kernel.

| File | Holds |
|---|---|
| `src/spec.rs` | `SandboxSpec` and its parts: `WriteRoot`, `CacheOverlay`, `Mask`, `Floor`, `NetworkPlan`, `EnvPlan`, `RecordLimits`, `RuntimePaths` with its `DesktopSocket`s, and `JobPlan`, the limits of a job (`efr-sbx run --job`): its call dir lies in the shell dir's `jobs` dir, and a call's never does |
| `src/fs_view.rs` | `FsView`, the only way this crate reads the file system (with the kinds of a listing, so a scan needs no `lstat` per entry), `resolve`, which follows every link, and `Memo`, which a plan uses so it looks up each path once |
| `src/desktop.rs` | `desktop_socket_path`: the one control socket of a desktop from the variable that names it (`$XDG_RUNTIME_DIR/hypr/<signature>/.socket.sock`, `$XDG_RUNTIME_DIR/$WAYLAND_DISPLAY`, `$SWAYSOCK`, `$I3SOCK`, `$NIRI_SOCKET`, `/tmp/.X11-unix/X<n>` for a local `$DISPLAY`), which efrd and the plan share; `is_desktop_socket`: the place and the file name that the socket of each desktop must have (`wayland-*`, `sway-ipc.*.sock`, `i3/ipc-socket.*`, `niri.*.sock`), so a variable cannot name another socket of the user, such as the session bus |
| `src/plan.rs`, `src/plan/` | `MountPlan`: the ordered mounts, the plan rules (a socket grant below an engine secret or a mask, or in efr's own roots, is refused; a desktop grant binds only the socket that `desktop_socket_path` names for the value in the spec, still with the place and the name of `is_desktop_socket` after every link, sets only its variable, and without a socket in the spec is `SandboxError::NoDesktop`), `explain` for `sandbox.explain` and the start dir of a call. `$SCRATCH/jobs` is a floor of every call once efrd made it (`FloorKind::JobOutput`), and a job gets each git dir that it finds at its start as a floor whole (`FloorKind::JobGitDir`). One rule decides the network namespace and the profile: a call keeps the host's network namespace only with an open network, or with `Grant::NetView` and no other network (`net_view()`), and then its inner stage gets the network view's seccomp profile. A `Grant::Unknown` (a grant of a newer efr) fails the plan, and so does `Grant::NetView` beside any grant other than `Write` and `Unmask` (`SandboxError::NetViewMixed`), the rule that efrd keeps too |
| `src/args.rs` | the bwrap argument list, `FdTable` (one descriptor per bind, never reused) and `LaunchFds` |
| `src/layers.rs` | `CacheLayers`: the cache overlays that `efr-sbx layers` mounts after bwrap's setup, and the mounts inside each cache that move onto it |
| `src/layers/deletes.rs` | `CacheDeletes`: the paths that a call deleted in its private cache layers (the whiteouts of each upper layer, read through `LayerView`), for `SandboxSummary::cache_deletes`, with the limits that keep `result.json` small |
| `src/landlock.rs` | `LandlockPolicy` and `FsAccess` as data; no `RESOLVE_UNIX` rule without a socket grant; a network view call also handles the TCP bind and connect rights with no rule (`deny_tcp`) |
| `src/seccomp.rs` | `SeccompProfile`: the deny list as data. `phase1()` for every contained call; `net_view()` for a call with `Grant::NetView`: only `AF_UNIX` and `AF_NETLINK` sockets, only the `ROUTE` and `SOCK_DIAG` netlink protocols, and `listen` and `pidfd_getfd` refused |
| `src/inner.rs` | `InnerPolicy`, what the launcher sends to `efr-sbx inner`, and `LayersSync`, the handshake while the overlays mount |
| `src/env_filter.rs` | `EnvFilter`: the environment of a contained call |
| `src/export_filter.rs` | `ExportFilter`: which exports return to the trusted shell; `OVERLAY_DENY` |
| `src/records.rs` | the records of fd 3 and the `apply` file of the trusted shell |
| `src/state.rs` | `SandboxState` and `state.zsh`: what later contained calls inherit |
| `src/result.rs` | the files of a call dir and `SandboxResult`, the launcher's `result.json`, with the time of each launcher step (`LaunchTiming`), and for a job the stop reason (`JobStop`) and its nice value; the job files: `env`, `launcher.log`, `surface-before.json`, `guard.json` |
| `src/surface.rs` | the surface guard: `SurfaceManifest`, `check_surface`, the scan for nested git dirs, the turn-end report entry; a manifest reads back from JSON, so a job keeps the one of its start on disk |
| `src/surface/listings.rs` | `ConfigListings`: git's listing of each config content, which the captures of a call and the next calls of the shell use again |
| `src/git_config.rs` | the git and cargo config keys that run a program, and a small git config reader |
| `src/worktree.rs` | `WorktreeRecord`: the git dirs of a worktree project, read once at registration |
| `src/probe.rs` | `ProbeReport`, `ProbeFailure` with the reason and fix texts, the Landlock and bwrap checks |
| `src/mounts.rs`, `src/mounts/` | the mount view: `parse_mountinfo` (with the kernel's octal escapes), `MountSnapshot` (the sizes that efrd measured, `mounts.json`), `StatvfsLine` (one answer of `efr-sbx statvfs`), the rules of the sizes (`is_dummy`, `is_other_users`, `is_masked`), and the `df` and `findmnt --df` tables of a snapshot in GNU's and util-linux's format (`render_df`, `render_findmnt_df`); the plan binds `mounts` and `mounts.json` of the call dir next to the launcher when efrd wrote them |
| `src/names.rs`, `src/paths.rs` | name patterns with `*`, secret-like names, and lexical path helpers |

## Tier

Tier 1. `efr-sbx`, `efr-shell` and `efr-daemon` depend on it.

## Allowed dependencies

`efr-protocol`, for the wire types that the spec and the result carry (`Grant`,
`CacheMode`, `SandboxSummary`, `SurfaceChange`, `SandboxStatus`). Third-party crates:
`serde`, `serde_json` and `thiserror`.

The crate must never reach tokio (`xtask/src/deps.rs` forbids `efr-sandbox -> tokio`),
so it does not depend on `efr-stdx`; the few paths it needs come in the spec as plain
paths (`RuntimePaths`). The secret tables stay in `efr-permissions`: efrd passes their
paths, so this crate needs no `efr-permissions` edge.

## Invariant

Nothing here decides alone what a sandboxed call may do with more rights than the spec
gives. A mask always wins over a floor, a floor always wins over a widening, the home
directory is never a write root, a link that a call could change never moves a floor or
a pin, and the records that a call reports are untrusted data. A cache delete counts
only when the lower entry exists, never at or below a pin, a mask or a moved mount.
No IO except through `FsView` and `LayerView`, no unsafe code, no async.

## Tests

Run the tests of this crate alone, without the rest of the workspace:

```sh
cargo nextest run -p efr-sandbox
```

The tests use `testing::FakeFs`, a file system in memory with links and sockets, and
upper layers in memory for the cache deletes. No
kernel sandbox, no network and no zsh: the real-bwrap tests belong to `efr-sbx` and run
under `just test-sandbox`.

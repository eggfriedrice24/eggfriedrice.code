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
| `src/spec.rs` | `SandboxSpec` and its parts: `WriteRoot`, `CacheOverlay`, `Mask`, `Floor`, `NetworkPlan`, `EnvPlan`, `RecordLimits`, `RuntimePaths` |
| `src/fs_view.rs` | `FsView`, the only way this crate reads the file system, and `resolve`, which follows every link |
| `src/plan.rs`, `src/plan/` | `MountPlan`: the ordered mounts, the plan rules, `explain` for `sandbox.explain` and the start dir of a call |
| `src/args.rs` | the bwrap argument list, `FdTable` (one descriptor per bind, never reused) and `LaunchFds` |
| `src/layers.rs` | `CacheLayers`: the cache overlays that `efr-sbx layers` mounts after bwrap's setup, and the mounts inside each cache that move onto it |
| `src/landlock.rs` | `LandlockPolicy` and `FsAccess` as data; no `RESOLVE_UNIX` rule without a socket grant |
| `src/seccomp.rs` | `SeccompProfile`: the deny list as data |
| `src/inner.rs` | `InnerPolicy`, what the launcher sends to `efr-sbx inner`, and `LayersSync`, the handshake while the overlays mount |
| `src/env_filter.rs` | `EnvFilter`: the environment of a contained call |
| `src/export_filter.rs` | `ExportFilter`: which exports return to the trusted shell; `OVERLAY_DENY` |
| `src/records.rs` | the records of fd 3 and the `apply` file of the trusted shell |
| `src/state.rs` | `SandboxState` and `state.zsh`: what later contained calls inherit |
| `src/result.rs` | the files of a call dir and `SandboxResult`, the launcher's `result.json` |
| `src/surface.rs` | the surface guard: `SurfaceManifest`, `check_surface`, the scan for nested git dirs, the turn-end report entry |
| `src/git_config.rs` | the git and cargo config keys that run a program, and a small git config reader |
| `src/worktree.rs` | `WorktreeRecord`: the git dirs of a worktree project, read once at registration |
| `src/probe.rs` | `ProbeReport`, `ProbeFailure` with the reason and fix texts, the Landlock and bwrap checks |
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
a pin, and the records that a call reports are untrusted data. No IO except through
`FsView`, no unsafe code, no async.

## Tests

Run the tests of this crate alone, without the rest of the workspace:

```sh
cargo nextest run -p efr-sandbox
```

The tests use `testing::FakeFs`, a file system in memory with links and sockets. No
kernel sandbox, no network and no zsh: the real-bwrap tests belong to `efr-sbx` and run
under `just test-sandbox`.

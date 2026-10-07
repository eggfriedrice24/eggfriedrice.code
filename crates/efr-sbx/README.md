# efr-sbx

## Purpose

The launcher of the `auto` sandbox. The hidden zsh's wrapper runs
`efr-sbx run --call-dir $CALL` as a foreground job for every model call in `auto`. The
launcher checks the call dir, builds the mount plan of `efr-sandbox` from the spec that
efrd wrote, and starts bwrap. bwrap mounts no cache overlay: when bwrap's setup is
done, the inner stage waits, and `efr-sbx layers` mounts the overlays with `index=off`
and `xino=off` in the call's namespaces. Then `efr-sbx inner` applies Landlock (ABI 9,
hard requirement) and the seccomp deny list, and runs the child shell. After the call
the launcher filters the records, runs the surface guard, and writes `$CALL/apply`, the
sandbox state and `$CALL/result.json`, in that order. An approved exit runs in the exit
child instead: no sandbox, the launcher is a child subreaper, and the exit child's
process tree ends with the call: SIGTERM, then SIGKILL after 2 s. A process that is
still there 2 s later (one that runs as another user, such as a root process that
`sudo` left) is named in the summary's `survivors`, the call still ends, and efrd
starts a new hidden shell. efrd runs `efr-sbx probe --json` to learn whether
`auto` can run here.

| Subcommand | What it does |
|---|---|
| `run --call-dir DIR` | one call, contained or in the exit child |
| `inner --policy-fd N` (hidden) | inside bwrap: waits for the cache overlays, then Landlock, seccomp and the child shell |
| `layers --pid N` (hidden) | enters the namespaces of the inner stage `N` and mounts the cache overlays of the plan on stdin |
| `probe --json --dir DIR` | the checks of the spec's section 12.1 and one real launch with the self-test |
| `self-test` (hidden) | the probe's checks from inside a sandbox; the escape suite uses it for system calls a shell cannot make |
| `bridge` (hidden) | the seam of phase 2; it refuses in phase 1 |
| `landlock-abi` (hidden) | prints the kernel's Landlock ABI from a rule set that restricts nothing |

| File | Holds |
|---|---|
| `src/call.rs` | `run`: the call dir, the contained launch, apply, state and result |
| `src/call_dir.rs` | the call dir and shell dir checks; atomic writes |
| `src/launch.rs`, `src/launch/status.rs` | one bwrap launch: argument and policy memfds, the pipes, the status stream, the handshake with the layer helper |
| `src/layers.rs` | the layer helper: the cache overlays, and the plan's mounts inside each cache moved onto it |
| `src/inner.rs` | the inner stage |
| `src/landlock.rs`, `src/seccomp.rs` | the policy of `efr-sandbox` applied with the `landlock` and `seccompiler` crates |
| `src/exit_child.rs` | the exit child as a subreaper |
| `src/finish.rs` | the records filtered for the trusted shell and the sandbox state |
| `src/guard.rs` | the surface guard: manifests, git as the config lister, quarantine |
| `src/probe.rs`, `src/probe/fixture.rs`, `src/self_test.rs` | the probe and its self-test |
| `src/fds.rs` | the only `unsafe` code: descriptor numbers the process does not own yet (ADR 0007) |
| `src/real_fs.rs` | `FsView` over `openat2(RESOLVE_NO_SYMLINKS)` |
| `src/os.rs` | the process, environment and time calls that `efr-stdx` routes elsewhere |
| `src/signals.rs` | SIGINT, SIGQUIT and SIGTSTP caught with a flag, never ignored |
| `src/bridge.rs` | the phase 2 seam |

What the launcher writes, in order:

| File | When |
|---|---|
| `$CALL/started` | once every bind source is open and the git surface is recorded, before bwrap or the exit child starts; efrd releases the plan lock then |
| `$CALL/exit-child/` | the exit child only: copies of `snapshot.zsh` (and `.zwc`) and `line` |
| `$SBX/quarantine/<call>/<n>-<name>` and `entries.json` | each entry the surface guard moved away; `entries.json` lists `{ "from", "to" }` so efrd can move an entry back when the user keeps it. Across file systems the launcher copies at most 64 MiB per call and removes the original all the same; a file cut short is listed in the entry's `truncated`, and efrd does not move that entry back |
| `$CALL/apply` | the filtered `cd`, exports and unsets for `_efr_hs_sbx_apply` |
| `$R/sbx/<conversation>/state.json`, `state.zsh` | contained calls only, when the records were valid |
| `$CALL/result.json` | last, by a rename, after every process of the call is gone |

The child shell's contract (efr-shell's `assets/zsh/efr-child.zsh` keeps it):
`zsh -f efr-child.zsh DIR`, where `DIR` holds `snapshot.zsh`, `state.zsh` (contained
calls only) and `line`; the records go to fd 3 in the format of the spec's section 6.2.
Contained calls get `DIR = $XDG_RUNTIME_DIR/efr-sbx` inside the sandbox; the exit child
gets `$CALL/exit-child` with copies of the snapshot and the line.

## Tier

Tier 2. A binary; nothing depends on it.

## Allowed dependencies

`efr-sandbox` (every rule of the launch) and `efr-protocol` (the wire types in the spec
and the result). Third-party crates: `landlock` 0.4.7, `seccompiler` 0.5.0 (with its
json frontend, which maps system call names to numbers), `rustix` (with `mount` for the
layer helper), `signal-hook`,
`clap`, `libc` (only in `src/fds.rs`), `serde`, `serde_json` and `thiserror`.

No `efr-stdx`: it reaches tokio, and `xtask/src/deps.rs` forbids `efr-sbx -> tokio`.
The launcher is a small process in the hidden shell's foreground job, so it has no
async runtime; it reads its own environment, which is the trusted shell's.

## Invariant

- Nothing runs unsandboxed that efrd did not mark `unsandboxed` in the spec. A failure
  of any step before the child starts is a setup failure (exit status 125 and
  `setup_error`); the line never runs elsewhere.
- Every bind source is opened with `openat2(RESOLVE_NO_SYMLINKS)`, one descriptor per
  bind, and the launcher clears `FD_CLOEXEC` on exactly the descriptors bwrap and the
  inner stage take; every other descriptor it inherited is close-on-exec.
- bwrap's stderr is a pipe to the launcher, so bwrap's messages never reach the tool
  output; the inner stage writes its own failures there before the child exists.
- `result.json` is written last, by a rename, after bwrap or the exit child and every
  process of the call are gone.
- A cache overlay is mounted only by `efr-sbx layers`, before any code of the call
  runs, with every mask, pin and floor inside the cache moved onto it. When the helper
  fails, the inner stage stops, and the call does not run. The launcher holds the
  call's mount namespace until bwrap ends, so the kernel tears the overlays down before
  the next call mounts them.
- The launcher never runs a program from a place a call can write: git for the surface
  guard comes from a `PATH` dir outside every write root.
- `unsafe` code exists only in `src/fds.rs` (`xtask/src/tidy.rs` holds the allowlist).

## Tests

Run the tests of this crate alone, without the rest of the workspace:

```sh
cargo nextest run -p efr-sbx
```

The unit tests need no sandbox. The integration tests (`tests/it/`: the escape suite,
the behaviour tests, the probe, the launch cost and the corpus run) drive the real
launcher with real bwrap, Landlock and seccomp. They read the launcher from
`EFR_TEST_SBX_BIN` and run `efr-sbx probe` first; without the variable, or when the
probe says the sandbox cannot run here, each prints `skipped: <reason>` and passes.
`just test-sandbox` builds the launcher, probes, sets `EFR_TEST_SBX_BIN` and
`EFR_TEST_SBX_REQUIRE=1`, and then a skip fails. It also runs efr-shell's behaviour
tests on this launcher (`e2e_zsh::launcher`) and fails when they skip on a ready
machine. Every test call runs efr-shell's own `assets/zsh/efr-child.zsh` and
`assets/efr-editor`, the files that efrd installs. Every fixture lives in the target
dir's temp dir with a fake home; no test reads or writes the user's real efr dirs, and
no test uses the network.

CI's `sandbox` job runs the same tests in a VM, because the kernel of GitHub's runner is
older than 7.1. `.github/sandbox-vm.sh` boots a pinned 7.1 kernel with virtme-ng and
runs the tests of the build job's archive inside; `just test-sandbox-vm` does the same
in a container that is set up like the runner.

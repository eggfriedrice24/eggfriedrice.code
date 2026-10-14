# efr-sbx

## Purpose

The launcher of the `auto` sandbox. The hidden zsh's wrapper runs
`efr-sbx run --call-dir $CALL` as a foreground job for every model call in `auto`. The
launcher checks the call dir, builds the mount plan of `efr-sandbox` from the spec that
efrd wrote, and starts bwrap. bwrap mounts no cache overlay: when bwrap's setup is
done, the inner stage waits, and `efr-sbx layers` mounts the overlays with `index=off`
and `xino=off` in the call's namespaces. Then `efr-sbx inner` applies Landlock (ABI 9,
hard requirement) and the seccomp deny list, and runs the child shell. After the call
the launcher lists the cache deletes (the whiteouts in each upper layer, which efrd
offers to apply to the real cache), filters the records, runs the surface guard, and
writes `$CALL/apply`, the sandbox state and `$CALL/result.json`, in that order. An approved exit runs in the exit
child instead: no sandbox, the launcher is a child subreaper, and the exit child's
process tree ends with the call: SIGTERM, then SIGKILL after 2 s. A process that is
still there 2 s later (one that runs as another user, such as a root process that
`sudo` left) is named in the summary's `survivors`, the call still ends, and efrd
starts a new hidden shell. efrd runs `efr-sbx probe --json` to learn whether
`auto` can run here.

A background job (efr's auto spec, section 3.18) is not a call of the hidden shell:
efrd starts `efr-sbx run --job` itself, with its own small environment, the job's
start dir as the working directory, a pipe as stdin (the lifeline) and the output pipe
as stdout. The launcher calls `setsid`, catches SIGTERM and SIGHUP, and reads one start
byte from the lifeline before it starts anything. On its main thread it then sets
nice to the higher of its own value and the spec's, the idle IO class, a hard file
size limit (soft and hard), SIGXFSZ ignored and an `oom_score_adj` of 1000, which
every child inherits. The job runs contained or in the exit child (with
`no_new_privs`), with the environment of `$CALL/env`, `/dev/null` as stdin and the
output pipe as stdout and stderr. A stop thread ends the job on SIGTERM or SIGHUP, at
the end of the lifeline, or at efrd's limit plus the backstop: SIGTERM to every process
of the job but bwrap's own, SIGKILL from 2 s, and it gives up at 4 s. The bridge of a
proxy job (named `efr-bridge`, with the namespace's init as its parent) is spared like
bwrap's own and is not named in `background_stopped`; it ends with the namespace. This
is a rule of the list only: a process that renames itself so still ends with the job. `result.json`
names the stop and the nice value. A job writes no apply and no state, lists no cache
deletes, and keeps the guard's record of its start in `$SBX/jobs/<call>`, where
`efr-sbx guard-check` reads it while the job runs.

| Subcommand | What it does |
|---|---|
| `run --call-dir DIR` | one call, contained or in the exit child |
| `run --call-dir DIR --job` | one background job that efrd started; the spec must have a job plan, and without `--job` it must not |
| `guard-check --call-dir DIR --state-dir SBXJOB` | compares a contained job's git surface with the record of its start, moves what breaks a rule to its quarantine, writes `guard.json`; exit status 0 with no change, 3 with a change, 1 on an error |
| `inner --policy-fd N` (hidden) | inside bwrap: waits for the cache overlays, starts the bridge of a proxy call, then Landlock, seccomp and the child shell |
| `layers --pid N` (hidden) | enters the namespaces of the inner stage `N`, mounts the cache overlays of the plan on stdin, and sends the descriptors of their upper dirs back over its stdout, a Unix socket |
| `probe --json --dir DIR` | the checks of the spec's section 12.1 and one real launch with the self-test; when it passes, a second launch with `Grant::NetView` runs the network view's self-test, whose result is the report's `netview` (a failure there is a warning, never a failed probe); a third launch with a proxy plan runs the proxy's self-test against a socket of the probe that answers `CONNECT` with `200` and echoes, whose result is the report's `proxy` (a warning too) |
| `self-test` (hidden) | the probe's checks from inside a sandbox; the escape suite uses it for system calls a shell cannot make. With `--net-view` it runs the network view's checks: `tcp4` first, and when it does not get `EAFNOSUPPORT` no other check runs; then the other families, the netlink protocols, `listen`, `pidfd_getfd`, the host's live abstract names and a Unix socket, each with its exact errno. With `--proxy` it runs the checks of a proxy call: a tunnel through the bridge (`proxy_tcp`), the proxy socket (`EACCES`), another address (`ENETUNREACH`), a signal to the bridge (`EPERM`), the bridge's `/proc` files (root's, `EACCES`), its port (`EADDRINUSE`) and UDP (`ENETUNREACH`); the probe runs the first four |
| `bridge --listen-fd 0 --socket P` (hidden) | the bridge of a proxy call: the starter runs the server (`--serve --ready-fd 1`) and exits 0 on its ready byte, else 1; the server copies TCP from `127.0.0.1:3128` inside to the call's proxy socket. `--check-dumpable` runs the server's setup and checks it |
| `landlock-abi` (hidden) | prints the kernel's Landlock ABI from a rule set that restricts nothing |
| `statvfs` (hidden) | efrd's helper of the mount view, outside the sandbox: the paths on stdin (NUL-separated, at most 4096), one thread each, one JSON line per path as each answers (`{"path", "ok"}` or `{"path", "errno"}`, with the device of `stat`); efrd kills it at its deadline |
| `mounts df ARGS`, `mounts findmnt-df ARGS` (hidden) | inside a call: the `df` or `findmnt --df` table of the snapshot next to the launcher (`mounts`, `mounts.json`), for the child shell's `df` and `findmnt`; the arguments are raw, so `-h` is `df`'s; exit status 2 with no output means "run the real command" |

| File | Holds |
|---|---|
| `src/call.rs` | `run`: the call dir, the contained launch, apply, state and result |
| `src/job.rs` | a job: the session, the start byte, the priority and limits, the `env` file, the lifeline thread and the stop thread |
| `src/guard_check.rs` | `guard-check`: a contained job's surface guard while it runs and after a restart |
| `src/call_dir.rs` | the call dir and shell dir checks; atomic writes |
| `src/launch.rs`, `src/launch/status.rs` | one bwrap launch: argument and policy memfds, the pipes, the status stream, the handshake with the layer helper |
| `src/layers.rs` | the layer helper: the cache overlays, the plan's mounts inside each cache moved onto it, and the upper dirs sent back with `SCM_RIGHTS` |
| `src/deletes.rs` | the cache deletes: `efr_sandbox::CacheDeletes` over the upper dirs that the helper sent, read with `RESOLVE_BENEATH` and `RESOLVE_NO_SYMLINKS`; a whiteout is a character device 0/0, an opaque dir has `user.overlay.opaque` |
| `src/timings.rs` | the time of each step of a contained launch, for `result.json` and efrd's `phase` debug lines |
| `src/layer_lock.rs` | in the `overlay` mode, the exclusive `flock` on the conversation's layer dir that each launch holds until its overlays are gone |
| `src/inner.rs` | the inner stage |
| `src/landlock.rs`, `src/seccomp.rs` | the policy of `efr-sandbox` applied with the `landlock` and `seccompiler` crates |
| `src/exit_child.rs` | the exit child as a subreaper |
| `src/finish.rs` | the records filtered for the trusted shell and the sandbox state |
| `src/guard.rs` | the surface guard: manifests, git as the config lister, quarantine |
| `src/probe.rs`, `src/probe/fixture.rs`, `src/self_test.rs` | the probe and its self-test |
| `src/fds.rs` | the only `unsafe` code: descriptor numbers the process does not own yet (ADR 0007) |
| `src/real_fs.rs` | `FsView` over `openat2(RESOLVE_NO_SYMLINKS)` |
| `src/os.rs` | the process, environment and time calls that `efr-stdx` routes elsewhere |
| `src/signals.rs` | SIGINT, SIGQUIT and SIGTSTP caught with a flag, never ignored; whether SIGINT or SIGQUIT stopped the call; for a job, SIGTERM and SIGHUP caught through a pipe for the stop thread |
| `src/bridge.rs` | the bridge of a proxy call: the starter, the server with its own session, name (`efr-bridge`), no dump, Landlock domain (`LandlockPolicy::bridge`) and seccomp profile (`SeccompProfile::bridge`), and the copies, two threads per connection, at most 64 connections |
| `src/statvfs.rs` | `statvfs`: the sizes of the host's mounts for efrd |
| `src/mounts.rs` | `mounts`: the tables of the mount view; a variable that changes `df`'s units (`DF_BLOCK_SIZE` and the like) leaves them to the real `df` |

What the launcher writes, in order:

| File | When |
|---|---|
| `$CALL/started` | once every bind source is open and the git surface is recorded, before bwrap or the exit child starts; efrd releases the plan lock then |
| `$CALL/exit-child/` | the exit child only: copies of `snapshot.zsh` (and `.zwc`) and `line` |
| `$SBX/quarantine/<call>/<n>-<name>` and `entries.json` | each entry the surface guard moved away; `entries.json` lists `{ "from", "to" }` so efrd can move an entry back when the user keeps it. Across file systems the launcher copies at most 64 MiB per call and removes the original all the same; a file cut short is listed in the entry's `truncated`, and efrd does not move that entry back |
| `$CALL/apply` | the filtered `cd`, exports and unsets for `_efr_hs_sbx_apply` |
| `$R/sbx/<conversation>/state.json`, `state.zsh` | contained calls only, when the records were valid |
| `$R/sbx/<conversation>/config-listings.json` | contained calls only, after the surface guard, when its listings changed: git's listing of each config content that the call read. git reads the content on stdin, so a listing depends on the content alone, and the next call runs git only for a new content. A listing of another git (path, inode, size or times) is never used |
| `$CALL/result.json` | last, by a rename, after every process of the call is gone; a contained call lists its cache deletes in `summary.cache_deletes` (at most 4096 paths and 256 KiB) and counts the rest in `cache_deletes_more` |
| `$SBX/jobs/<call>/surface-before.json` | a contained job only, before its child starts: the guard's record of the job's start, on disk, for `guard-check` |
| `$SBX/jobs/<call>/guard.json` | `guard-check` only: `{ "changes", "quarantined" }` |

A job writes `started`, its quarantine and `result.json` as above, and never `apply`,
`state.json`, `state.zsh` or `config-listings.json`. It writes no `launcher.pid`:
efrd keeps the launcher's pid and start time itself. Its call dir is
`$R/sbx/<conversation>/jobs/<call>`, so nothing that reads the direct entries of the
shell dir takes it for a call that still runs. The job's end guard and `guard-check`
hold a `flock` on `$SBX/jobs/<call>` while they compare and move entries; each one adds
its entries to the quarantine's `entries.json` and keeps those already there.

The child shell's contract (efr-shell's `assets/zsh/efr-child.zsh` keeps it):
`zsh -f efr-child.zsh DIR`, where `DIR` holds `snapshot.zsh`, `state.zsh` (contained
calls only) and `line`; the records go to fd 3 in the format of the spec's section 6.2.
SIGINT or SIGQUIT at any time before the line ends the child with 128 plus the signal,
and the line does not run; during the line, either signal ends the child.
Contained calls get `DIR = $XDG_RUNTIME_DIR/efr-sbx` inside the sandbox; the exit child
gets `$CALL/exit-child` with copies of the snapshot and the line. For a line with `df` or
`findmnt`, a contained call's `DIR` also holds `mounts` and `mounts.json`, the snapshot
of the host's mounts, next to the launcher itself (`DIR/efr-sbx`).

## Tier

Tier 2. A binary; nothing depends on it.

## Allowed dependencies

`efr-sandbox` (every rule of the launch) and `efr-protocol` (the wire types in the spec
and the result). Third-party crates: `landlock` 0.4.7, `seccompiler` 0.5.0 (with its
json frontend, which maps system call names to numbers), `rustix` (with `mount` for the
layer helper, and `net` for the socket pair that brings its upper dirs back),
`signal-hook`,
`clap`, `libc` (only in `src/fds.rs`), `serde`, `serde_json` and `thiserror`.

No `efr-stdx`: it reaches tokio, and `xtask/src/deps.rs` forbids `efr-sbx -> tokio`.
The launcher is a small process in the hidden shell's foreground job, so it has no
async runtime; it reads its own environment, which is the trusted shell's.

## Invariant

- Nothing runs unsandboxed that efrd did not mark `unsandboxed` in the spec. A failure
  of any step before the child starts is a setup failure (exit status 125 and
  `setup_error`); the line never runs elsewhere.
- Ctrl+C (SIGINT or SIGQUIT) during the setup is not a failure. The launcher catches
  the signal first. When the signal comes before bwrap or the exit child starts, the
  launcher does not start it. When the signal breaks the setup, the result has the
  signal and no `setup_error`, and the status is 128 plus the signal (130 for
  SIGINT). A signal that comes while bwrap or the exit child starts goes to the
  call's process group again.
- During a contained launch the launcher is a child subreaper. When a signal ends
  bwrap before bwrap set the parent-death signal of the namespace's init, the init
  becomes the launcher's child, and the launcher ends it with SIGKILL. Without this,
  the init holds the pipes of the launch open, and the launcher waits forever.
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
- The launcher reads an upper layer only after the call, through the descriptor that
  the helper opened, under the conversation's layer lock. It lists a whiteout only when
  the lower entry exists, never at or below a pin, a mask or a moved mount, and it never
  deletes anything itself: efrd decides and deletes.
- The launcher never runs a program from a place a call can write: git for the surface
  guard comes from a `PATH` dir outside every write root.
- A job never runs on a terminal: it has no controlling terminal, its stdin is
  `/dev/null`, and no descriptor of the launcher above 2 reaches it. It starts only
  after efrd's start byte, and it ends with efrd: the end of the lifeline stops it.
- `unsafe` code exists only in `src/fds.rs` (`xtask/src/tidy.rs` holds the allowlist).
- The bridge of a proxy call serves only in its own sandbox. Two `Command` spawns start
  it (no `fork`): the starter waits at most 1 s for the server's ready byte, which the
  server writes only after its own Landlock domain and seccomp profile are in place, and
  the inner stage waits at most 2 s for the starter. Any other end is a setup failure,
  and the call does not run. The child's domain has no rule for the proxy socket, and
  its signal scope keeps it from the bridge.

## Tests

Run the tests of this crate alone, without the rest of the workspace:

```sh
cargo nextest run -p efr-sbx
```

The unit tests need no sandbox; the seccomp tests run the compiled filters on a small
BPF interpreter for both architectures, and the Landlock TCP test restricts one thread
of its own. The integration tests (`tests/it/`: the escape suite, the network view's
escape tests in `netview.rs`, the behaviour tests, the jobs, the probe, the launch cost
with the cost of a job's start and of a network view call, and the corpus run) drive
the real launcher with real bwrap, Landlock and seccomp. A network view test passes a
refusal only on its exact errno, and targets only the test's own listeners and
abstract names; its write checks change nothing even if they work. They read the launcher from
`EFR_TEST_SBX_BIN` and run `efr-sbx probe` first; without the variable, or when the
probe says the sandbox cannot run here, each prints `skipped: <reason>` and passes.
`just test-sandbox` builds the launcher, copies it to a private dir in the target dir,
probes, sets `EFR_TEST_SBX_BIN` to the copy and `EFR_TEST_SBX_REQUIRE=1`, and then a
skip fails. A test that needs a tool skips without it (`need!`), except a tool that
the gates and CI install: `require!` fails a test with `EFR_TEST_SBX_REQUIRE=1` when
that tool is missing. `ss` and `ip` (iproute2), for the network view, are such tools.
The tests run the copy, because another cargo command in the same target
dir can link a new `target/debug/efr-sbx` while a test runs it. The probe tests take
`EFR_TEST_SBX_BIN` too; without it they run the launcher that cargo built with them
(`CARGO_BIN_EXE_efr-sbx`), and then no other cargo command may build in that target
dir during the run. It also runs efr-shell's behaviour
tests on this launcher (`e2e_zsh::launcher`) and fails when they skip on a ready
machine. Every test call runs efr-shell's own `assets/zsh/efr-child.zsh` and
`assets/efr-editor`, the files that efrd installs. Every fixture lives in the target
dir's temp dir with a fake home; no test reads or writes the user's real efr dirs, and
no test uses the network.

CI's `sandbox` job runs the same tests in a VM, because the kernel of GitHub's runner is
older than 7.1. `.github/sandbox-vm.sh` boots a pinned 7.1 kernel with virtme-ng and
runs the tests of the build job's archive inside; `just test-sandbox-vm` does the same
in a container that is set up like the runner.

# efr-pty

## Purpose

The in-process PTY holder. `LocalPtyHolder` implements `efr_holder::PtyHolder`: it
opens a pseudo-terminal, starts the spec's program on it as a session leader with the
PTY as its controlling terminal, sets and changes the window size, delivers signals,
and reaps every child. At milestone 1 the daemon uses it through its `local-pty`
feature; from milestone 5 `efr-ptyd` and the PTY proxy use it, and the daemon reaches
it only over the holder socket.

Modules:

- `local_holder`: `LocalPtyHolder` and the spawn. The holder opens the master with
  `posix_openpt` (`O_NOCTTY | O_CLOEXEC`), runs `grantpt` and `unlockpt`, opens the
  slave by its `ptsname` (also `O_NOCTTY`, so the PTY never becomes the daemon's own
  controlling terminal), sets the window size and `IUTF8`, and moves the slave off
  descriptors 0 to 2. The child is started with `efr_stdx::process::command`; its
  `pre_exec` closure runs `setsid`, `TIOCSCTTY`, `dup2` of the slave onto 0, 1 and 2,
  `PR_SET_CHILD_SUBREAPER` when the spec asks for it (`SpawnSpec::child_subreaper`; the
  flag survives `execve`), resets every standard signal to its default action with none
  blocked, and marks every other descriptor close-on-exec (`close_range`, with an
  `fcntl` walk on kernels before 5.11). The holder keeps a copy of each master for
  resizes and the foreground-group query until `release`, and hands the original to
  the caller. `foreground` answers with `tcgetpgrp` on that copy: the child's pid while
  the child holds its terminal, another group while one of its jobs does, and `None`
  once the child has exited or the terminal has no foreground group.
- `child`: one child process. A task on the caller's tokio runtime owns the tokio child
  and reaps it the moment it exits; the result goes into a `watch` channel that `list`
  reads and `wait` subscribes to. Signals to the child go through a pidfd opened before
  anything could reap it, so a signal never reaches a process that reused the pid.
- `termios`: the window size, `IUTF8` and the foreground process group, through
  rustix.
- `error`: `PtyError`, the step of a spawn that failed (opening the master, the slave,
  setting up the terminal, no runtime). The trait returns `HolderError`; a failed spawn
  is `HolderError::Spawn`, whose `io::Error` source keeps the system error's kind and
  wraps the `PtyError`, so a caller can downcast it.

Behaviour a caller relies on:

- The master in `PtyHandle` is blocking. A reader that wants `AsyncFd` sets
  `O_NONBLOCK` itself; the flag is shared with the holder's copy, which only does
  ioctls, so that is safe.
- A read of the master ends with `EIO` once the child and everything it started have
  closed the slave; the holder keeps no slave open.
- `wait` and `list` agree: the status starts as running and changes once, when the
  reaper has reaped the child, so after `wait` returns, `list` never reports the child
  as running. If waiting for a child fails (something else reaped it), the status is
  `Exited { code: -1 }`, a code no process can exit with, and the failure is logged.
- `release` closes the holder's copy of the master. The child keeps running while the
  caller holds its copy and gets `SIGHUP` when that closes too; it is still reaped.
- The holder needs a tokio runtime with IO enabled, as tokio's process support does.
  A spawn outside any runtime fails with `PtyError::NoRuntime` before it opens
  anything.
- Linux only: the crate uses `pidfd_open`, `close_range` and the Linux PTY ioctls on
  the master side.

## Tier

Tier 2.

## Allowed dependencies

`efr-holder` (the trait, its types and the re-exported `PtyId` and `Size`) and
`efr-stdx` (`process::command`, the one constructor of child processes).
`xtask/src/deps.rs` holds the allowlist.

Third-party crates: `rustix` (`pty`, `termios`, `process`, `fs`, plus `stdio` for
`dup2` onto the standard streams), `libc` (only the `signal`, `sigprocmask`,
`close_range` and `fcntl` calls in the child), `tokio` (the child process and the
reaper task), `async-trait`, `thiserror` and `tracing`.

## Invariant

- `crates/efr-pty/src/local_holder.rs` is the only file in the workspace that may
  contain `unsafe` code at milestone 1 (the allowlist is in `xtask/src/tidy.rs`). Every
  block carries a `// SAFETY:` comment naming what it relies on; the `pre_exec` closure
  calls only async-signal-safe functions, allocates nothing and takes no lock.
- The master is an `OwnedFd` from the first line, and no raw descriptor number leaves
  the crate.
- The child gets exactly the spec's environment, its working directory and the PTY on
  descriptors 0, 1 and 2, and nothing else from the holder process: no other
  descriptor, no ignored signal, no blocked signal. The same spec starts the same shell
  in the daemon and in `efr-ptyd`.
- Every child is reaped, released or not, so the holder leaves no zombies.

## Tests

Run the tests of this crate alone, without the rest of the workspace:

```sh
cargo nextest run -p efr-pty
```

The tests open real PTYs and run `/bin/sh` children with an environment of only
`PATH`, in `/` or a temp directory. They read `echo ok` back from the master, check the
starting size and a resize with `stty size`, signal the child and the foreground
group, check the exit status from `wait` against `list`, check the session and the
controlling terminal, the foreground group, that a child subreaper adopts the orphan of
an intermediate shell and a plain child does not, that no other descriptor reaches the
child, that the environment is exactly the spec's, and every release rule. They need `/bin/sh`, `stty` and `tr`,
use no network and no zsh, and never wait on a clock: a test reads the master until the
output it expects appears or the slave closes.

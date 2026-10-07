# 0007: An unsafe exception for efr-sbx's fds module

Status: accepted, 2026-10-06.

## Context

`unsafe_code` is denied in the whole workspace. `cargo xtask tidy` allows the token
`unsafe` only in a short list of files, and every block needs a `// SAFETY:` comment.

`efr-sbx` is the launcher of the `auto` sandbox (the spec's sections 3 and 15.1). It
works with descriptors that arrive by number:

- The trusted shell starts the launcher. Every descriptor from 3 up that the launcher
  inherits must be close-on-exec before it starts bwrap or the exit child, so that
  nothing of the trusted shell reaches the sandbox. The launcher does not own these
  descriptors, and it does not know their numbers.
- bwrap takes the bind sources, the status pipe, the argument list and the policy by
  number. `efr_sandbox::FdTable` owns the bind sources and hands out numbers only, and
  the launcher must clear `FD_CLOEXEC` on exactly those numbers.
- The inner stage inside bwrap receives the policy, the records pipe and a copy of the
  terminal by number. It must own them, move the records pipe to exactly fd 3 for the
  child shell, and close every other descriptor before it applies Landlock, so that no
  descriptor opened outside keeps its rights inside.
- The exit child needs the records pipe at exactly fd 3 too.
- The layer helper mounts the cache overlays of a call. It must enter the user
  namespace that owns the call's mounts, which is the parent of the namespace that
  `/proc/<pid>/ns/user` names, because bwrap nests the call in a second one.
- The probe's self-test proves that seccomp refuses `io_uring_setup`, `TIOCSTI` and
  `unshare(CLONE_NEWUSER)` by making these calls.

Safe Rust and rustix cover most of the launcher: `openat2`, `memfd_create`, pipes,
`fcntl_setfd` on an owned descriptor, `dup2` onto an owned descriptor, `prctl` for the
child subreaper, signal flags through `signal_hook::flag::register`, and Landlock and
seccomp through their crates. They do not cover these steps: rustix has no
`close_range`, no way to turn a number into an `OwnedFd`, no `dup3` onto a number the
process does not own yet, no `NS_GET_PARENT`, and it offers `io_uring_setup` and
`unshare` only as `unsafe` functions and `TIOCSTI` not at all. `setns`, `fsopen`,
`fsconfig`, `fsmount`, `open_tree` and `move_mount` of the layer helper are safe in
rustix. `efr-pty` has the same gap for its
`pre_exec` code and calls `libc` in its one allowlisted module.

## Decision

`crates/efr-sbx/src/fds.rs` is the one module of `efr-sbx` that may contain `unsafe`
code. `main.rs` opts it out with `#[allow(unsafe_code)]` on its module item, and
`xtask/src/tidy.rs` lists the file. It holds exactly these functions:

| Function | Call | Why it is sound |
|---|---|---|
| `mark_inherited_cloexec(first)` | `close_range(first, ~0, CLOSE_RANGE_CLOEXEC)` | integers only; it sets a flag |
| `close_from(first)` | `close_range(first, ~0, 0)` | the caller dropped every `OwnedFd` in the range first |
| `inherit_number(fd)` | `fcntl(fd, F_SETFD, 0)` | `FdTable` keeps the descriptor open for the call |
| `adopt(fd)` | `fcntl(F_GETFD)`, then `OwnedFd::from_raw_fd` | the number came from an argument, is open, and has no other owner |
| `dup_to(src, target, cloexec)` | `dup3(src, target, flags)`, then `from_raw_fd(target)` | `target` has no owner in the process |
| `ns_parent(fd)` | `ioctl(fd, NS_GET_PARENT)`, then `from_raw_fd` | no memory is passed; the result is a new descriptor with no other owner |
| `io_uring_blocked()` | `io_uring_setup(1, &params)` | a 120-byte zeroed buffer; a ring that is made is closed at once |
| `tiocsti_blocked(fd)` | `ioctl(fd, TIOCSTI, &byte)` | one byte through a live pointer, on a terminal the self-test opened |
| `userns_blocked()` | `unshare(CLONE_NEWUSER)` | no descriptor table is unshared; the self-test has one thread |

Each call happens while the process has one thread: at the start of the launcher,
before its reader threads exist, in the inner stage, which starts no thread, in the
layer helper (`efr-sbx layers`) and in the self-test.

## Consequences

- The rest of `efr-sbx` stays safe Rust; a reviewer reads one file of about 150 lines.
- A new raw call in the launcher goes into this module, with its row in this table and a
  `// SAFETY:` comment, or it does not go in.
- `libc` is a dependency of `efr-sbx` for this module alone; the README says so.
- If rustix gains `close_range` and a safe way to adopt a descriptor by number, the
  first five functions move to it and this exception shrinks to the self-test.

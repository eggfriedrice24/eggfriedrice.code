# 0006: The auto mode runs commands in a kernel sandbox

Status: accepted, 2026-10-06. Phase 1 (containment) is the part that this record
covers as built.

## Context

`auto` was a static allow list: the writer programs, the build, test, format and lint
tools of a project, and local git, with no question. The user rejected it. An allow
list judges the command line, but a build or a test runs whatever the project's code,
its build scripts and its git hooks do. The model writes that code freely in the
project, so in `auto` it could write a `Makefile` or a `build.rs` and run it, with the
user's rights and full network access. The engine's limits, config protection and the
approval of the settings tool did not hold against injected instructions.

The goal for `auto`: a command whose effects stay inside a known envelope runs with no
question and no model review, and the kernel, not text analysis, enforces the envelope
for the command and for every process that it starts.

## Decision

- Each shell command of the model in `auto` runs in a per-call sandbox: bubblewrap
  (user, pid, net, ipc, uts and cgroup namespaces, read-only root, binds by file
  descriptor), Landlock ABI 9 or higher (file rights and `RESOLVE_UNIX`), and a seccomp
  deny list. A trusted launcher, `efr-sbx`, starts it from a fixed wrapper line in the
  hidden zsh; the model's line is never typed into that shell.
- The envelope: writes only in the turn's project, the registered projects that the
  line names, `$SCRATCH`, a private `/tmp` and private overlays of the tool caches; no
  network; no sockets, D-Bus or new privileges; secrets and efr's own state masked;
  shell startup files, autostart, efr's config, the git surface and agent configs
  read-only (the floor). A mask always wins over a floor. The home directory is never
  a write root.
- The engine gets the effect `contain` between `allow` and `ask`, and `auto` gets its
  own base table. No rule lifts the sandbox.
- An action outside the envelope is an exit. efr finds exits before the run, and the
  model can ask for one with `needs` after a failure. In phase 1 the user answers every
  exit. A "yes" widens the sandbox by exactly one path, socket, device, bus or the
  network for one call. Privilege, upload, persistence, writes at or above a root or to
  synced folders, and `outside` run in a separate exit child with full rights; such a
  line must be one command plus read-only helpers, and the question shows the whole
  line, every program word and an untrusted mark for programs that the sandbox wrote.
- Only `cd` and listed exports return to the trusted shell. Functions and aliases stay
  in the sandbox's own state.
- A surface guard checks the git dirs after each call. It quarantines planted settings
  that run programs and asks the user, with its own question, before the next call.
- A probe runs at start and again after failures and config changes. When any part is
  missing, `auto` runs as `cautious` and says why. There is no partial sandbox.
- The design is in five phases. Phase 1 (this one) is containment and ships alone. A
  network proxy, an exit classifier, snapshot and undo, and a filtered bus with
  always-allow rules are later phases and are not part of this decision as built.

## Consequences

- `auto` needs Linux with Landlock ABI 9 (Linux 7.1) and an unprivileged bubblewrap.
  Ubuntu 24.04, Debian 13 and the GitHub runners fall back to `cautious`. CI tests the
  fallback and the pure parts; the escape suite runs only on a ready machine with
  `just test-sandbox`.
- Phase 1 has no network in the sandbox. A command that downloads is an exit, and
  `sandbox.offline_hints` asks package managers to use their caches.
- Each call costs a few milliseconds for bubblewrap and Landlock, plus the replay of
  the user's shell snapshot.
- New code: the pure crate `efr-sandbox`, the launcher `efr-sbx` (with one module of
  `unsafe`, ADR 0007), new wire types and events in `efr-protocol`, and the
  `[sandbox]` table in `config.toml`. The curated `auto` table of `efr-permissions` is
  deleted.
- The hidden zsh runs no user hooks or prompt code in any mode, because a prompt theme
  would run planted git settings outside the sandbox. direnv and venv auto-activation
  do not work in the hidden shell.
- Risks that stay are listed in `docs/sandbox.md` ("Known limits"): kernel or
  bubblewrap bugs, code that the model writes and the user runs later, older hard
  links, readable command lines of other processes, resource exhaustion, and the full
  rights of an approved exit outside the sandbox.

## Alternatives

1. **Keep the allow list and make it stricter.** Rejected: any list that allows a build
   runs code that the model can write.
2. **A classifier for every command.** Rejected as the main mechanism: a model judging
   text cannot hold what a build script does. A classifier for exits only is a later
   phase.
3. **A container or a VM per conversation.** Rejected: it hides the user's machine,
   which efr exists to work on, and costs much more per call.
4. **Approved exits typed into the trusted shell.** Rejected: a sourced script could
   then redefine the wrapper for every later call. Exits run in a separate child.

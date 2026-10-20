# 0015: Full mode, a mode with no sandbox and no questions

Status: accepted, 2026-10-20.

## Context

efr had three permission modes. `manual` and `cautious` ask for each action that is
not routine. `auto` runs each command at once in a kernel sandbox, and an action that
leaves the sandbox goes to a reviewer model or to the user (ADR 0006). Other agents
have a mode that runs every command with no question, such as `bypassPermissions` of
Claude Code and "Full Access" of the Codex CLI. Users asked for the same in efr: for a
machine where the sandbox cannot run (Ubuntu 24.04, Debian 13, WSL1), and for work
where each question costs more than the risk.

Such a mode has no sandbox, so nothing holds a program that a command starts. A floor
that holds for every process in `auto` holds only for what the command line shows. A
process that leaves the hidden shell's tree (`setsid -f`, `nohup ... &`, a fork in a
script, `systemd-run --user`) counts as the user on efr's socket: efrd walks `/proc`
from a peer, and such a walk ends at pid 1. So it can answer efr's own questions, send
prompts and reload the config. These choices are expensive to reverse: the words that
efr uses for the mode set what users expect of it.

## Decision

1. **The `auto` decision without the sandbox, then a settle pass.** `Engine::decide`
   decides a full call as an `auto` call on a machine with no sandbox
   (`AutoSupport::for_full`: no egress, no bus filter, no hidden host). Then a pure
   pass (`efr-permissions/src/full.rs`, `settle`) changes the reasons: what the
   sandbox would hold, and what a built-in row asks about, runs with `Cause::Full`;
   each exit runs and keeps its record; the floors stay. So full mode reuses the exits,
   the home copies, the undo cover and the records of phases A to E, and `auto` stays
   byte for byte as before. A full machine with a table of its own was rejected: it
   needs a third case in each place that `auto` contains a call.
2. **What still holds.** A floor deny (a secret, efr's config, its own programs and
   dirs), a privileged program and a `privilege` exit (local only, one question for the
   whole line), a `persistence` exit, a delete that undo cannot revert, a change of the
   settings, and the user's own `ask` and `deny` rules. Every other exit runs with no
   review and is recorded with the judge `full`.
3. **The delete guard.** A new pure detector (`exits::critical_deletes`) reads the
   line for deletes of `~`, `/`, a system dir, a project root, a dir that holds efr's
   files, a secret, a floor or more than one project, and of a target that it cannot
   read. Each such delete asks at the user's own machine, at most 120 seconds. When
   nobody answers, the delete does not run and the model reads how to go on. After 3
   expired questions in a turn, each later guarded delete is refused with no
   question. A job refuses it. The guard is no exit kind: `auto` never reads it, and
   the exit records stay as they were.
4. **The gate.** `permissions.allow_full = true` in `config.toml` allows full mode. The
   user sets it by hand or with `efr config set`; the settings tool of the model never
   changes it and never sets `permissions.mode = "full"`. `permissions.mode = "full"`
   is valid only next to the gate key; without it, the file does not load. A terminal
   chooses full mode with `,mode full`, a prompt with `--mode=full`.
5. **Where it never runs.** A phone turn runs with at most `cautious`, and a turn
   that a job notice starts with at most `auto`. efrd refuses full mode while it runs
   as root, while `snapshot.enabled` or `undo.enabled` is false, and when the hidden
   shell is no zsh (efr clears sudo's cached password before and after each full call
   through its zsh integration). The turn reads the gate again before each call, so a
   change of the gate stops a running full turn at its next call.
6. **The config hold.** While a full turn or a job that it started runs, efrd does not
   load a change of `config.toml` that loosens permissions. The hold stays after the
   full work ends, for every automatic reload, until the user runs `efr config reload`
   while no full work runs, or the change leaves the file. `efr status` names the keys
   that loosen.
7. **The words.** Every text of full mode (the `,mode full` lines, the turn-start line,
   the docs, the changelog) says the same: full mode guards against mistakes, not
   against a hostile model or a prompt injection; a script or a program that a line
   starts is not read; undo and the record cover what efr saw.

## Consequences

- The floors of a shell line are text. A script, `python3 -c`, a build step or a
  program that the model wrote can read a secret, send an API key from the
  environment to a host, write `config.toml`, delete `~` or efr's snapshots, and none
  of it is recorded or undone. `docs/permissions.md`, "Full mode", marks each floor as
  hard or text.
- The guard is against mistakes, not a hostile model. It reads one level into
  `sh -c` and nothing inside a script. Some safe deletes ask, such as
  `find . -name '*.o' -delete` in a project root and `rm -rf *`.
- Any process that runs as the user can answer efr's questions, send full prompts
  while the gate is on, reload the config after a full turn, and turn the gate on
  again after the user turned it off. "Only your own machine" means a local process,
  not a person. A floor sandbox for full mode (a sandbox that holds only the floors)
  can come later as a setting; it is the only way to a stronger promise.
- The record and undo cover what efr saw. Undo covers the project, `$SCRATCH` and the
  named projects fully, and outside them only the files that a predicted exit named.
- The environment of the hidden shell is exposed as in `cautious`.
- Full mode works where `auto` cannot run, so users may pick it as their daily mode.
  The docs name `auto` as the safe choice where it runs.
- An older `efr` cannot read a `turn_started` with `mode: full`. efr and efrd ship
  together.

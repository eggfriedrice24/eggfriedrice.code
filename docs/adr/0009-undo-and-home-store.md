# 0009: Undo plans in `efr-snapshot`, writes in efrd, and a home store

Status: accepted, 2026-10-11.

## Context

Phase B of the auto spec (sections 10.5 to 10.8) adds `efr undo`. Undo writes the
user's files and moves the refs of the user's repositories. A wrong write loses work
that efr cannot bring back. These choices are expensive to reverse: they set where the
code of undo lives, what is on disk, and what git reads.

## Decision

1. **The planner lives in `efr-snapshot`; the writes live in efrd.** The planner is
   pure on trees and refs: it reads the store and writes nothing, so a dry run takes
   no snapshot and moves no ref. Only efrd writes the user's files, through its own
   file access (`DaemonFs`), which opens each parent with `RESOLVE_BENEATH` and
   `RESOLVE_NO_SYMLINKS`. Just before each write, efrd reads the file again and
   compares its blob id with what the plan expects. So a link that appears after the
   plan, or a change of the user, never makes undo write the wrong file.

2. **Undo acts only on the paths that the turn's own calls changed (`touched`).** The
   store records them after each call. A path that differs between the turn's first
   and last snapshot but that no call changed is a change of the user between two
   calls, and undo leaves it. A diff of the two snapshots alone would revert it.

3. **The undo trash is `$D/undo-trash/<undo-id>/`, not `$D/trash`.** The model can
   write `$D/trash` (it is efr's trash for the model's own deletes, and the engine
   counts it as rebuildable). `$D/undo-trash` falls under efr's own floor and the
   sandbox mask of the data root. Every file that undo overwrites or removes goes
   there first, written from a store blob and never moved with `rename`, so a root on
   another file system needs no special case.

4. **The project's refs come from `git for-each-ref`, not from ref files.** A
   reftable repository (git 2.45 and later) has no plain ref files. A second hardened
   mode of the store's runner (`ProjectGit`) runs a short list of commands on the
   project's git dir, which efrd takes from its worktree record and pins by device and
   inode. A capture never writes the project's `.git`. Only an applied undo writes its
   refs and objects, by compare and swap, under the plan lock.

5. **Exact bytes come from the store's own `info/attributes`.** It holds `* -text -eol
   -ident -filter -working-tree-encoding`, which has the highest precedence of all
   attribute files on every git version. Every store command also turns off the user's
   and the system's attribute files. So a project's `.gitattributes` never changes the
   bytes that undo writes back, and efr needs no git version floor.

6. **The home store is `git fast-import` from bytes that efrd streams.** git never
   reads a home path, runs no filter and follows no link. Each copy is a "before"
   commit and an "after" child commit with a manifest of each path's state, so undo
   compares the current file with the state after the call, and a later change of the
   user is a conflict, not an overwrite.

7. **The stack and the journal live in the store.** A turn is undone when its store
   has an `undone` ref. An apply writes a `started` ref first, so an undo that stops
   half way resumes at the next `efr undo`, and `--redo` takes back what it did.

## Consequences

- The store holds the project's `.env` files and other small ignored files for up to
  `snapshot.max_age_days` days. `docs/storage.md` says so. There is no command that
  drops the copies of one turn at once in phase B.
- An approved exit outside the sandbox can change the store. efr checks each `pre` ref
  at the end of the turn and says so, but it cannot stop the change.
- A write of another conversation or of the user inside a call's window counts as the
  call's change. Its old bytes go to the undo trash, so nothing is lost.
- Background jobs (phase C, ADR 0010) write after their call ended. Undo refuses while
  a job of the conversation runs, or a job of another conversation writes in a root of
  the undo. At the job's end, its changes join the `touched` paths of the turn that
  started it, except the paths that another call changed meanwhile, which stay with
  that call's turn. The start turn's roots are marked not complete, so its undo needs
  `--force`. A job's home copies get the "before" commit and no "after": undo lists
  them as `not covered (a job wrote it)`, and `--force` brings back the state before
  the job.

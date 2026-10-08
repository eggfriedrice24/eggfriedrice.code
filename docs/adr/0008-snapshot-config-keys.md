# 0008: The snapshot keys live in `[snapshot]`, not `[undo]`

Status: accepted, 2026-10-08.

## Context

The auto spec (sections 10.2 and 13.1) names the keys of efr's own snapshot store
`undo.max_file_mib`, `undo.ignored` and `undo.keep_turns`, next to `undo.enabled`. It
puts only `snapshot.mirror_max_mib` in `[snapshot]`.

The snapshot store came before undo. It shows what a call and a turn changed: the row
under a `shell` call, the line at the end of a turn and `efr diff`. These keys set what
that store takes and keeps, so they change the change lists today, with no undo at all.
The same round added three keys that the spec does not name: `snapshot.enabled`,
`snapshot.max_files` and `snapshot.max_age_days`.

Config keys are user-facing. A rename after a release needs aliases or breaks the
user's file.

## Decision

Every key of the snapshot store lives in `[snapshot]`:

| Spec key | Key |
|---|---|
| `undo.max_file_mib` | `snapshot.max_file_mib` |
| `undo.ignored` | `snapshot.ignored` |
| `undo.keep_turns` | `snapshot.keep_turns` |
| (none) | `snapshot.enabled`, `snapshot.max_files`, `snapshot.max_age_days` |
| `snapshot.mirror_max_mib` | `snapshot.mirror_max_mib`, when the ref mirror comes |

`[undo]` keeps the keys of undo itself. `undo.enabled` comes with `efr undo` in phase 4
and switches undo on or off. It does not switch the snapshots off:
`snapshot.enabled = false` does that, and then undo has nothing to restore.

The spec's sections 10.2, 10.4 and 13.1 now use these names.

## Consequences

- One table holds what the snapshot store takes, what it keeps and how long. A user
  who wants smaller stores or fewer change rows edits `[snapshot]` only.
- Undo reads the same keys. Phase 4 adds `undo.enabled` and no copy of a size limit.
- No key changes its name after this release.

## Alternatives

- Move the three keys to `[undo]`, as the spec says. Rejected: the keys would set the
  change lists of a feature with no undo yet, under a table whose name says undo, and
  `undo.enabled = false` would read as if it switched off the snapshots too.
- Keep both names with aliases. Rejected: two names for one key in an unreleased
  config only add a source of mistakes.

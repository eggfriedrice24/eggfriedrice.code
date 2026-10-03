# 0002: Shell marks come from a scanner in efr-screen

Status: accepted, 2026-10-03.

## Context

The shell tool needs the exact output and exit status of each command the model runs
in a hidden zsh. Shells report this with OSC 133 (prompt, input, output start, command
end with exit status) and OSC 7 (cwd). Neither libghostty-vt pin exposes an OSC 133
event: the pinned master has only per-row semantic prompt state with no exit code, and
the newer ghostty effect (commit 7bb45ba) fires after the screen update with no stream
offset. vt100 handles only OSC 0, 1, 2 and 52.

## Decision

`efr-screen/src/shell_marks/` holds a backend-independent `ShellMarkScanner` that
`ScreenActor` runs on every chunk before it feeds the backend. It frames `ESC ]`
sequences (BEL and `ESC \` terminators, split chunks, a 4096-byte body cap, DCS, APC, PM
and SOS skipped), parses OSC 133 `A`, `P`, `B`, `C`, `D` with their options, and parses
OSC 7 in both URL forms. Each `ShellMark` carries the recording byte offsets of its
start and end, so a command's output is the recording between the end of `C` and the
start of `D`.

The hidden shell's integration script is written from scratch to emit the same
sequences as ghostty's zsh integration, which is GPLv3 and is never copied.

## Consequences

- Both backends produce identical marks, and one conformance suite tests them.
- Output slices come from the recording, not from rendered rows.
- When a libghostty-rs rev wraps the 7bb45ba effect, it becomes a cross-check in
  `efr-screen-ghostty/src/semantic_prompt.rs`; the scanner stays the source of truth.
- Nested shells and raw `ESC ]` fragments in binary output are open risks, covered by
  proptest chunk-split tests and, if needed later, fuzzing.

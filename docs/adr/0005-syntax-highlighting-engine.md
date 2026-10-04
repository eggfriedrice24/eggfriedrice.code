# 0005: Syntax highlighting engine for efr-render

Status: open, 2026-10-04. syntect is in use for now; the choice is to be revisited.

## Context

`efr-render` colours code blocks and diffs in agent replies. It uses syntect 5.3.0 with
the pure-Rust regex backend and two-face 0.5.2 for bat's syntaxes and themes; the
default theme is bat's Ansi theme, so code colours follow the terminal palette.

On 2026-10-04 `cargo deny` reported RUSTSEC-2025-0141: bincode 1.3.3 is unmaintained.
It is not a vulnerability. The bincode team stopped development and calls 1.3.3
complete. syntect and two-face load their embedded grammar and theme dumps with it, and
no upgrade exists, because syntect itself depends on bincode 1.

## Decision for now

Keep syntect. `deny.toml` ignores exactly RUSTSEC-2025-0141, with a reason. Any other
advisory for bincode or for syntect still fails the deny gate.

## Options to revisit

1. **Stay on syntect** if upstream moves its dumps off bincode 1, or if bincode 1.3.3
   simply stays unmaintained without a vulnerability.
2. **syntect without binary dumps**: parse grammars from source at runtime. No bincode,
   but slower first-code-block latency in a short-lived `efr` process, and the grammar
   sources must be bundled.
3. **tree-sitter-highlight**: more accurate highlighting and an active ecosystem, but a C
   grammar per language, possible link conflicts between grammar crates, a larger
   binary, and a C toolchain in the build. Rejected for milestone 1 for these reasons.

## What would close this record

A decision between the options above, with a measurement of startup latency and binary
size for the candidate, and the removal of the advisory ignore from `deny.toml` if
syntect goes.

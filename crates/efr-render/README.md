# efr-render

## Purpose

Markdown in, ANSI out, for the `efr` CLI (and later the PTY proxy). The daemon never
renders: the event log keeps raw markdown and the phone app renders natively.

- `Renderer`: the streaming renderer. A reply is pushed in pieces as it arrives. Each
  push returns the newly committed output (complete blocks, written once to scrollback
  and never rewritten) and the live zone (the block still arriving, which the caller
  redraws in place inside synchronized output). `finish` commits the rest.
- `render`: a whole document at once. It equals what a `Renderer` commits for the same
  text pushed in any pieces.
- `render_trace`: one dim line, cut to the width, for a tool call trace or reasoning.
- `RenderOptions`: the width, the `ColourMode` (none, 16 colours, truecolor), the
  `Theme` for code, OSC 8 hyperlinks on or off, and whether the output is a terminal.

What commits when:

- A paragraph commits at its end (a blank line or the next block). When it grows past
  three complete lines, its earlier lines commit and the last stays live, so a table
  delimiter row or a setext underline that arrives next still finds its line.
- A fenced or indented code block commits line by line, with syntax colours.
- A list commits each item when the next item starts.
- A table commits when it is complete, because column widths need every row.
- A heading or a rule commits with its own line; a quote at a blank line.
- A partial line is never committed.

Because committed output stays on the screen, a few things differ from rendering the
whole reply at once with a single parse: a reference-style link resolves only against
a definition in its own block, and emphasis or a setext heading cannot reach back over
paragraph lines that already committed. Each slice is rendered with a fresh parse,
which is what makes the result independent of how the reply was chunked.

Elements: headings bold and coloured without `#`; bold, italic and strikethrough as SGR;
inline code coloured (with its backticks when there is no colour); code blocks with a
dim language label, never wrapped; unified diffs with green and red lines and the
diffed file's syntax colours inside (tinted backgrounds with a truecolor theme); lists
and quotes indented, quotes with a dim bar; task items as `[ ]` and `[x]`; tables as a
grid, or one `header: value` record per row when wider than the terminal; links, bare
URLs, absolute paths in links and inline code as OSC 8 hyperlinks (`file://` for
paths); images as `[image: alt]`.

Soft line breaks keep the author's lines. Top-level prose is not hard-wrapped, so the
terminal reflows it on resize; text inside a list item or a quote is wrapped to keep
its indent.

Grammars and themes come from `two-face` (bat's set) on `syntect` with the pure-Rust
`fancy-regex` backend, and decompress on the first code block that needs them. The
default theme is `ansi`, which uses the terminal's 16-colour palette and so follows the
Ghostty theme.

## Tier

Tier 1.

## Allowed dependencies

No workspace crate. `xtask/src/deps.rs` holds the allowlist.

Third-party crates: `pulldown-cmark` (tables, strikethrough, task lists), `syntect`
(`parsing`, `dump-load`, `regex-fancy`; no oniguruma), `two-face` (`syntect-fancy`),
`unicode-width`, `thiserror`.

## Invariant

- No IO and no environment: the crate never writes to a terminal and never reads
  `NO_COLOR`, `TERM` or anything else. The CLI decides and passes `RenderOptions`.
- Streaming is exact: what is committed is decided once per complete source line, from
  the text up to that line only, so any chunking of a reply commits byte for byte what
  `render` produces for the whole reply.
- Committed output is never taken back: a block commits only when no later text can
  change it, or (a long paragraph, earlier list items) when the rest is rendered as its
  continuation.
- Every output line opens and closes its own SGR state and hyperlink, so a caller can
  clip the live zone to whole lines.
- Model output cannot drive the terminal: control characters in the markdown are shown
  as visible stand-ins, hyperlink targets are percent-encoded, and only `http`,
  `https`, `mailto` and `file` targets become hyperlinks.
- When the output is not a terminal, the markdown passes through unchanged.

## Tests

Run the tests of this crate alone, without the rest of the workspace:

```sh
cargo nextest run -p efr-render
```

Snapshots (`insta`) cover every element at 80 and 40 columns with the `ansi` theme,
plus a document without colour, a diff in a truecolor theme and links without
hyperlinks; escape bytes appear as `\e`. Property tests check that any chunking of a
document, byte-by-byte streaming included, commits what `render` does, and that early
commits render the elements exactly as one parse of the whole document does. Nothing
touches the network, the terminal or the home directory.

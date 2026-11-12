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
- `render_trace`: one muted line, cut to the width, for a tool call trace or
  reasoning.
- `Preload`: loads the syntax data of code blocks ahead, for a caller that runs it
  on a thread of its own (the CLI does, from the start of the follow), so a frame
  does not wait for it. `load` loads the grammars and the theme with its highlighter
  (1 to 2 ms). `text` reads the reply's text as it comes and readies the grammar of
  each fenced block whose opening line ends in it, which the block's first line
  otherwise pays (1 to 125 ms the first time, by the language: about 14 ms for Rust,
  125 ms for TypeScript).
- `diff_rows`: a unified diff as painted rows, for a caller that lays a diff out
  itself (the CLI under a file write): the colours of a diff in a code block, with the
  tint of a changed line behind its text, each line cut at the width into rows that
  read back as the line (never at a space, because a diff shows code), the spaces
  before a cut kept. Without a terminal, each line is one plain row.
- `diff_lines`: a unified diff as one `DiffLine` for each line: its kind (added,
  removed, context, hunk, other), its painted sign apart from the content, the content
  cut into rows of the width, and the fill of its rows (the SGR parameters of
  `diff.add.bg` or `diff.remove.bg`, or of the blended tint). The sign and each span
  of the rows are on the fill already; the caller writes the fill before its indent and
  again before the `ESC [ K` that ends a row. The seam of the terminal look: this crate
  paints the lines in the colours of each depth (TM-Diffs), the CLI lays them out.
  `diff_rows` stays for the callers of today.
- `RenderOptions`: the width, the `ColourMode` (none, 16 colours, truecolor), the
  `Palette`, the `Theme` for code or a `CodeTheme` from a `.tmTheme` file (and one in
  bat's ANSI encoding for 16 colours, `with_code_theme_ansi`), OSC 8 hyperlinks on or
  off, the `WidthMethod`, the `Glyphs`, and whether the output is a terminal.
  `with_width` makes the same options for a new width after a resize.
- `Role` and `Palette`: every colour goes through a role. `RenderOptions::paint` and
  `RenderOptions::sgr` give the CLI the same roles for its own lines.
  `RenderOptions::paint_with` adds `Attrs` (bold, italic, underline, reverse, dim) to
  a role, such as the reverse and bold badge of a question; the attributes stay
  without colour. `RenderOptions::tint` gives the colour of a role without its
  attributes, for the CLI's context gauge, whose colour changes with its level.
  `RenderOptions::fill` gives the SGR parameters of a background role, for a row that
  the caller fills to the edge with `ESC [ K` and resets before its newline.
- `Glyphs`: every glyph of the CLI's own lines (the marks of the design's legend and
  the mark of the thought row), with `Glyphs::UNICODE` and `Glyphs::ASCII`. The CLI
  picks the table from the locale; `RenderOptions::glyphs` gives it. No other file of
  this crate or of the CLI writes one of these glyphs as a literal (a test of the CLI
  checks it; the files of today are listed there until the slices move them).
- `text_width` and `display_width`: the columns of text (painted text without its
  escape sequences) by code point or by grapheme cluster. `display_rows`: the rows of
  one painted line as the terminal wraps it (a wide character that does not fit at the
  end of a row starts the next row). `last_rows`: the last rows of one painted line,
  with the colours and the hyperlink that are open where they start, for a caller that
  clips the live zone by rows. `Update::live_tail` clips the live zone the same way.

Colour roles and their defaults in 16 colours:

| Role | Used for | Default | Without colour |
|---|---|---|---|
| `text` | prose | the terminal's foreground | plain |
| `muted` | the marks of Markdown, labels, rules, notes, traces | dim | dim |
| `accent` | the colour of headings, the spinner | palette entry 3 (yellow) | bold |
| `heading` | headings of level 1 and 2 | bold, in the accent's colour | bold |
| `link` | links | entry 5 (magenta), underlined | underlined |
| `code` | inline code, a command to run | entry 2 (green) | plain, with backticks |
| `success` | done task boxes | entry 2 (green) | plain |
| `warning` | questions, refusals | entry 3 (yellow) | bold |
| `error` | failures | entry 1 (red) | bold |
| `quote` | quote text | italic | italic |
| `diff.add`, `diff.remove` | diff lines | entries 2, 1 | plain (the sign stays bold) |
| `diff.hunk` | hunk headers | entry 6 (cyan) | dim |
| `code.bg` | the background of code rows | none | none |
| `diff.add.bg`, `diff.remove.bg` | the background of added and removed rows | none (a truecolor code theme gives a tint) | none |

The last three roles are backgrounds. A background role has no default colour and no
attribute: it paints only in truecolor and only when the palette sets an RGB colour,
and it is never reduced to one of the 16 colours. When the palette sets `diff.add.bg`
and `diff.remove.bg`, the rows of a diff take them; else a truecolor code theme tints
them with a blend of its background, as before.

A `Palette` sets any role to a `Colour`: a palette entry or RGB. A role keeps its
attributes (bold, underline, italic) with a new colour. `muted` is dim only when it has
no colour. An RGB colour is written as RGB in truecolor and not at all without colour.
In 16-colour mode each role is one of the 16 slots (TM-Depths 5 to 8): a role whose
palette colour is RGB, or an entry above 15, keeps its 16-colour default, so `muted`
stays dim and `text` the terminal's own colour; a slot from 0 to 15 that the palette
names stays. The colours of a code theme still become the nearest of the 16 entries: a
colour with a clear hue keeps its hue (red, yellow, green, cyan, blue or magenta,
normal or bright), and only a colour with little hue becomes a grey. When `text` has a colour,
every span without a colour of its own gets it. A heading without a colour of its own
takes the accent's colour.

Widths: a terminal that counts by code point (most terminals, tmux) gives a ZWJ emoji
the width of every emoji in it. Ghostty counts by grapheme cluster (mode 2027): a
cluster takes the width of its first code point, a variation selector 16 makes a narrow
emoji wide, a variation selector 15 makes a wide one narrow, and a flag takes two
columns. `WidthMethod::Grapheme` counts the same way. The row count of the live zone and
the cut of a trace line and the word wrap of prose use the method of the options; the
layout of tables counts by code point.

What commits when:

- A paragraph commits at its end (a blank line or the next block). When it grows past
  three complete lines, its earlier lines commit and the last stays live, so a table
  delimiter row or a setext underline that arrives next still finds its line.
- A fenced or indented code block commits line by line, with syntax colours. A
  fenced block in a list item or a quote does too: its first line commits with the
  text of the item or quote before it. A blank line in it commits only with the next
  line of code that is not blank, because at the end of a list item it is not code.
- A list commits each item when the next item starts.
- A table commits when it is complete, because column widths need every row.
- A heading or a rule commits with its own line; a quote at a blank line.
- A partial line is never committed.

What the live zone costs: the text goes in one line at a time, and the work for a
line does not grow with the reply. A fenced code block in a list item or a quote is
followed line by line: each new line is checked against the text up to the block's
opening line only (the head), is painted alone and commits. The committed lines
leave the pending text, so the pending text keeps the head and the lines that are
not committed, and the live zone is the partial line alone. A later rendering of the
item or quote renders the head again (it is short) and writes only what comes after
the committed lines (`Continuation::Nested`). Any other live zone is rendered again
at each push, its partial last line included: for prose, tables, quotes and lists
that costs about 40 us for each KB of pending text. Only from 4 KB of pending text
that holds a code block in a list item or a quote that is not followed
(`LIVE_CACHE_BYTES`) is the live zone rendered again only when a line is complete,
because each rendering paints the syntax colours of the whole block again (about
1.5 ms for each KB).

Because committed output stays on the screen, a few things differ from rendering the
whole reply at once with a single parse: a reference-style link resolves only against
a definition in its own block (and not against one after a committed code line in
the same item or quote), and emphasis or a setext heading cannot reach back over
paragraph lines that already committed. Each slice is rendered with a fresh parse,
which is what makes the result independent of how the reply was chunked.

Elements: the marks of Markdown stay where they carry structure, in `muted`, so the
level of a heading or a list reads without colour and a copy pastes as Markdown
(TM-Markdown). A heading starts with `#` once for each level and a space, then its
text in one colour with the level by weight (level 1 bold and underlined in the
heading colour, level 2 bold in the heading colour, level 3 bold, lower levels bold
italic). An item of a bullet list starts with `-` at every level, and each level is two
columns further in; an ordered item starts with its number and a dot; a task item
starts with its box, `[x]` in `success` and `[ ]` in `muted`, and no `-`. A quote
line starts with `>` and a space, and its text is in the `quote` role (italic).
Bold, italic and strikethrough are SGR; inline code is in the `code` role (with its
backticks when there is no colour). Code blocks are never wrapped, and have no frame
and no indent, so a copy is the code (TM-Code). A muted label line names the
language and the file when the fence names both (`rust · src/main.rs` from
`rust src/main.rs`, `title="x"` or `file=x`), else the one it names; a short table
gives the language of a short fence word (`ts` gives `typescript`); plain text
(`text`, `txt`, `plain`, `plaintext` or no info) and diffs (`diff`, `patch`, `udiff`)
show no label. In truecolor each row of code is on `code.bg` to the end of the row:
each span on the background, then `ESC [ K` with the background set, then a reset
before the newline; the label and the blank line after the block are not. A row as
wide as a whole number of terminal rows gets no `ESC [ K`, which would erase its last
character there. In 16 colours and without colour a code block has no background.
Without colour the fences come back, dim: the opening fence with the fence word as
typed and no file (` ```rust `), and the closing fence, also while the block streams,
under its lines in the live zone. A comment stays italic without colour (a theme of
one rule; the grammars load for it). Unified diffs have the `diff.*` roles: the sign
of a changed line bold in its role, a hunk header whole in `diff.hunk`, file headers
bold. In truecolor a changed line has the diffed file's syntax colours inside and its
row on `diff.add.bg` or `diff.remove.bg` (else, with a truecolor theme, a tint that
efr blends from the theme's background); in 16 colours a changed line is whole in its
role's colour, with no syntax and no background; without colour a removed line is
dim. A table that fits is a grid: the columns apart by three spaces, the
header in bold, one `muted` rule under the header as wide as the table, and no row
ends in padding. A table wider than the terminal is one record for each row: one line
for each column with the header in `muted` in a column of its own, two spaces and the
value, the first value of a record in bold, and a blank line between records. Rules
are muted and at most 40 columns wide, so they do not wrap when the terminal gets
narrower. Links, bare URLs, absolute paths in links and inline code are OSC 8
hyperlinks (`file://` for paths); without a hyperlink, the target follows in muted
parentheses. Images show as `[image: alt]`. The line of a rule and of the table's rule
is the `rule` glyph of the options (`─`, or `-` in ASCII).

Soft line breaks keep the author's lines. A line of prose or a heading wider than the
terminal is wrapped at words into rows of the width, as the boards of the design draw
it, so the terminal does not cut a word in two; text inside a list item or a quote is
wrapped to keep its indent. The cost: the rows are hard, so a terminal no longer
reflows a reply when its width changes, and a copy of a paragraph has a line break at
each wrap. Code blocks are never wrapped, so a copy of code is the code.

Grammars and themes come from `two-face` (bat's set) on `syntect` with the pure-Rust
`fancy-regex` backend, and decompress on the first code block that needs them, or
before when the caller asks for them with `Preload`. A grammar unpacks its contexts
and builds its patterns at the first line of its first block, not at the label
line, so a `Preload` has the time of the label line to do it first. The
default theme is `ansi`, which uses the terminal's 16-colour palette and so follows the
Ghostty theme. `CodeTheme::from_tmtheme` reads the bytes of a `.tmTheme` file (the CLI
reads the file); while the options have one, it wins over the `Theme`. All code blocks
of one theme share one highlighter. An embedded theme makes it at its first block; a
`.tmTheme` file's theme and highlighter are made when the file is parsed and stay in
memory until the process ends.

The grammars give some tokens no scope, or the scope of another kind, where the
syntax table of the design (TM-Code, after the tree-sitter captures of the editor)
has a kind. For Rust, TypeScript, JavaScript and shell, the highlighter adds the
scope of that kind on top of such a token, and the code theme colours it by its own
rule: a plain Rust variable (`variable.other`), a field after `.`, a macro name with
its `!`, the name of an attribute, a type or a constant by its case, a module after
`::`, `self` as a parameter, a primitive type such as `usize`; an upper-case
TypeScript constant; a shell number, a test operator such as `-ne` inside `[[ ]]`,
and the shebang as a directive (not a comment). It adds no colour: a theme with no
rule for the scope shows the token as before. In Markdown it leaves out the scope of a
list item, which the grammar puts on the whole item, so a list in a diff of a
Markdown file has no colour (TM-Diffs 1). The parse state does not see these changes.

## Tier

Tier 1.

## Allowed dependencies

No workspace crate. `xtask/src/deps.rs` holds the allowlist.

Third-party crates: `pulldown-cmark` (tables, strikethrough, task lists), `syntect`
(`parsing`, `dump-load`, `regex-fancy`, `plist-load` for `.tmTheme` files; no
oniguruma), `two-face` (`syntect-fancy`), `unicode-width`, `unicode-segmentation`
(grapheme clusters), `thiserror`.

## Invariant

- No IO and no environment: the crate never writes to a terminal, never reads a file
  and never reads `NO_COLOR`, `TERM` or anything else. The CLI decides and passes
  `RenderOptions`; a `.tmTheme` file comes in as bytes.
- Every colour goes through a `Role`, so one `Palette` changes all of them.
- Streaming is exact: what is committed is decided once per complete source line, from
  the text up to that line only, so any chunking of a reply commits byte for byte what
  `render` produces for the whole reply.
- Committed output is never taken back: a block commits only when no later text can
  change it, or (a long paragraph, earlier list items) when the rest is rendered as its
  continuation.
- Every output line opens and closes its own SGR state and hyperlink, so a caller can
  clip the live zone to whole lines, and `last_rows` can cut a line at a row.
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
plus every element without colour at both widths, a diff in a truecolor theme and
links without hyperlinks; a document of every role in 16 colours, without colour,
with overrides and with a hex palette in truecolor and in 16 colours; and the
`.tmTheme` fixture `fixtures/sample_theme.tmTheme`; the rows of `diff_rows` at 40 and
80 columns with and without colour. Escape bytes appear as `\e`. Unit tests check the
widths of ZWJ emoji, flags and variation selectors both ways, the live-zone row count
with them, the rows of a line with wide characters at the end of a row, the colours
and the link that `last_rows` opens again, and that the rows of a diff fit the width
and read back as its lines. Stream tests check that a long paragraph line and a long
table show their partial line at once, that a long quote with a code block keeps its
live zone until the line ends, and that the start of the next item after a long fence
shows at once, and that each line of a fence in a list item or a quote, with a
language and without one, commits when it ends while the pending text keeps the
head only. Property tests check that any chunking of a
document, byte-by-byte streaming included, commits what `render` does, and that early
commits render the elements exactly as one parse of the whole document does. More
property tests stream fences in list items and quotes (code, blank lines, closing
fences, lines that end the block, tabs, carriage returns) and check that after each
push the screen (all that was committed and the live zone) is what a renderer
without the line-by-line follow and without kept live zones shows. One more property
test keeps every live zone it can: a pending text without a code block still shows
what the renderer without kept live zones shows at each push. Unit tests check that
`Preload` loads the grammars and the theme only for colour on a terminal, and that
it finds the fences that open in pieces of text, also in list items and quotes. Unit
tests check the background roles (an RGB colour in truecolor only), `paint_with` in
each mode, the code points and the ASCII forms of `Glyphs`, and that `diff_lines`
gives each line its kind, its sign apart and the rows that `diff_rows` paints. The
test `design` of the test binary compares the output with a frame of the terminal
design (`efr_test_support::design`) in the depths that each board draws: TM-Markdown
1 to 5, TM-Code 1 to 6, the diff rows of TM-Diffs 1 to 3, TM-Depths 4, 8 and 12, and
TM-Legend 6, 20, 21, 37 and 38. Each module of it says where a board and the output
differ and which rule wins: the column gaps of a hand-drawn table, the syntax inside
a diff in 16 colours, and the code inside a diff in a fence, which names no language.
Its code themes are the code themes of the design system
(`efr_test_support::design::DESIGN_THEME`). The test `theme` renders the code blocks
of TM-Code with them and compares the colour and the attributes of each cell of code
with frame 1 (truecolor) and frame 2 (16 colours, the ANSI code theme); it fails when
one cell differs. A unit test checks that
the shared highlighter colours each line as
a new highlighter for each line did. Nothing touches the network, the terminal or the
home directory.

## Benches

Ignored tests that `just bench-perf` runs in release. They use the bench helpers of
`efr-test-support`, a dev-dependency only. A size that takes more than 10 s stops its
ladder. Their budgets are slopes and ratios, not times.

- `bench_perf_render_diff` (`tests/it/perf.rs`): `render` of a fenced diff of 2k, 10k
  and 20k lines of Rust, of a lockfile diff of 2k lines of 1 KB, and of a `text`
  fence of 20k, 50k and 100k lines, which has no syntax colours. Budget: the time of
  the Rust diff and of the text fence grows with a slope below 1.3, and no size
  stops.
- `bench_perf_live_nested_fence` (`tests/it/perf.rs`): a `1.` item that holds a Rust
  fence of 30, 150, 300 and 1,000 lines, pushed in pieces of 16 bytes. It gives the
  total time and the time of each push. Budget: no size stops, the total time grows
  with a slope below 1.3, and the median push at 1,000 lines takes at most
  three times the median push at 30 lines.
- `bench_perf_highlight_line` (`src/highlight/tests.rs`): the time of each line with
  the `ansi` and `catppuccin-mocha` themes, for 1k and 10k lines, the load of the
  grammars and the theme, and the first line of Rust, Bash, Python, JSON, TOML,
  TypeScript and Go after the load.

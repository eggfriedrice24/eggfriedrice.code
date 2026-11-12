//! TM-Code, the code block of TM-Legend and TM-Depths: a muted label `language ·
//! file`, each row of code on `code.bg` to the end of the row in truecolor, no
//! background in 16 colours, the fences back and dim without colour, a comment
//! italic, a diff in a fence with its rows on the tints of the diff roles.
//!
//! The colours of the tokens come from the code themes of the design system. The
//! tests compare every cell of a code row with the board, the text colour too, but
//! for the code inside the diff in a fence (see [`FENCED_DIFF`]).

use efr_render::{Renderer, render};
use efr_test_support::design::{Depth, Frame, Ink, Look, assert_rows, cells, expect};

use super::{assert_code_rows, code_options, plain, screen, soft_wrap};

/// The Markdown of TM-Code 1 to 3: the text of the `NO_COLOR` frame, whose fences are
/// the Markdown, with the rows that the terminal wraps joined, and the file of the
/// fences that name one.
const CODE: &[&str] = &[
    "The fix drains the channel before the end check:\n",
    "\n",
    "```rust crates/efr-daemon/src/turn.rs\n",
    "struct Turn<'a> { rx: &'a Receiver<Msg> }\n",
    "\n",
    "// drain steers before ending the turn\n",
    "pub fn finish(&mut self) -> usize {\n",
    "    let mut drained = 0;\n",
    "    'drain: loop {\n",
    "        let Ok(msg) = self.rx.try_recv()\n",
    "            else { break 'drain };\n",
    "        if msg.text.is_empty() {\n",
    "            continue 'drain;\n",
    "        }\n",
    "        self.pending.push(msg);\n",
    "        drained += 1;\n",
    "    }\n",
    "    tracing::debug!(\"turn {} drained {} steers, {} still pending\\n\", self.id, drained, self.pending.len());\n",
    "    drained\n",
    "}\n",
    "```\n",
    "\n",
    "```ts\n",
    "// \"claude-opus-5-5\" \u{2192} \"Opus 5.5\"\n",
    "const ID = /^claude-(\\w+)-(\\d+)(?:-(\\d+))?$/;\n",
    "\n",
    "export function shortName(id: string) {\n",
    "  const m = ID.exec(id);\n",
    "  if (!m) return id;\n",
    "  const v = m[3] ? `${m[2]}.${m[3]}` : m[2];\n",
    "  return `${cap(m[1])} ${v}`;\n",
    "}\n",
    "\n",
    "class ModelCache {\n",
    "  #byMachine = new Map<string, Model[]>();\n",
    "  get(machine: string): Model[] {\n",
    "    return this.#byMachine.get(machine) ?? this.#fetch(machine, { refresh: false, timeoutMs: 4000 });\n",
    "  }\n",
    "}\n",
    "```\n",
    "\n",
    "```bash\n",
    "#!/usr/bin/env bash\n",
    "# restart efrd, then show what it logged\n",
    "set -euo pipefail\n",
    "unit=efrd\n",
    "since=$(date -d '-5 min' +%H:%M)\n",
    "systemctl --user restart \"$unit\"\n",
    "sleep 2\n",
    "if [[ $? -ne 0 ]]; then\n",
    "  echo \"restart failed: ${unit}\" >&2\n",
    "  exit 1\n",
    "fi\n",
    "journalctl --user -u \"$unit\" --since \"$since\" -o cat | grep -E 'error|warn|steer' | tail -n 20\n",
    "```\n",
    "\n",
    "```json\n",
    "{\n",
    "  \"models\": [\n",
    "    {\n",
    "      \"id\": \"claude-opus-5-5\",\n",
    "      \"efforts\": [\"low\", \"medium\", \"high\"],\n",
    "      \"default_effort\": \"high\",\n",
    "      \"default\": true,\n",
    "      \"context_window\": 200000\n",
    "    },\n",
    "    { \"id\": \"claude-haiku-4-5\", \"efforts\": [], \"default_effort\": null, \"default\": false, \"context_window\": 200000 }\n",
    "  ]\n",
    "}\n",
    "```\n",
    "\n",
    "```toml ~/.config/efr/config.toml\n",
    "[render]\n",
    "theme = \"eggfriedrice\"          # code blocks and diffs\n",
    "motion = true\n",
    "diff_lines = 20\n",
    "\n",
    "[render.colors]\n",
    "accent = \"#ffc940\"\n",
    "muted = \"#8695b7\"\n",
    "\"diff.add\" = \"#60e654\"\n",
    "```\n",
    "\n",
    "```diff\n",
    "+ use rusqlite::Connection;\n",
    "@@ -88,3 +88,3 @@ fn import\n",
    "         let row = rows.next()?;\n",
    "-        let ended = row.ended_at.unwrap();\n",
    "+        let ended = row.ended_at.unwrap_or(row.started_at);\n",
    "```\n",
    "\n",
    "```\n",
    "efrd 0.0.5 \u{b7} protocol 1\n",
    "listening on 100.64.70.12:7447 (tailscale0)\n",
    "paired: sam's iPhone \u{b7} since Oct 2\n",
    "models: claude-opus-5-5, claude-sonnet-5, claude-haiku-4-5, gpt-5.5\n",
    "```\n",
];

/// The rows of TM-Code 1 and 2 with code inside the diff in a fence, whose text
/// colour the tests do not compare.
///
/// NOTE (open): the board draws the syntax of Rust inside them, but the fence says
/// only `diff` and the diff names no file, so there is no grammar for the code. A
/// guess would paint a wrong grammar on another diff.
const FENCED_DIFF: [usize; 4] = [81, 83, 84, 85];

/// TM-Code 1 and Legend 38: in truecolor the label is muted, each row of code is on
/// `code.bg` to the end of the row and the label and the blank line after a block are
/// not, a block with no language has no label, a diff in a fence has no label and its
/// changed rows are on the tints with a bold sign, the hunk header whole in
/// `diff.hunk`. The text colour of each cell is the board's, with the code themes of
/// the design system, except the code inside the diff in a fence ([`FENCED_DIFF`]).
#[test]
fn code_blocks_in_truecolor_are_the_board() {
    let frame = Frame::load("Code", 1).unwrap();
    let out = render(&CODE.concat(), &code_options(Depth::TrueColor, frame.cols));
    let rows = screen(&out, frame.cols);
    assert_code_rows(&frame, 0..frame.rows.len(), &rows, Depth::TrueColor, |row| {
        !FENCED_DIFF.contains(&row)
    });
    for (row, sign) in [(81, '+'), (84, '-'), (85, '+')] {
        assert_eq!(rows[row].cells[0].text, sign);
        assert!(rows[row].cells[0].look.bold);
    }
}

/// TM-Code 2: in 16 colours no background, the label dim, the tokens in the slots of
/// the ANSI code theme, the comment in slot 8 and italic.
#[test]
fn code_blocks_in_sixteen_colours_are_the_board() {
    let frame = Frame::load("Code", 1).unwrap();
    let out = render(&CODE.concat(), &code_options(Depth::Ansi16, frame.cols));
    let rows = screen(&out, frame.cols);
    // NOTE: a changed row of a diff is whole in its role's colour, with no syntax
    // (TM-Diffs 2, TM-Legend 21, spec 1.3); TM-Code 2 draws syntax in them.
    let ansi = expect(&frame, Depth::Ansi16).unwrap();
    assert_eq!(ansi.number, 2);
    assert_code_rows(&ansi, 0..ansi.rows.len(), &rows, Depth::Ansi16, |row| {
        !FENCED_DIFF.contains(&row)
    });
    let added = Look { fg: Ink::Slot(2), ..Look::default() };
    assert_eq!(rows[85].cells[10].look, added, "{}", rows[85].text());
    let removed = Look { fg: Ink::Slot(1), ..Look::default() };
    assert_eq!(rows[84].cells[10].look, removed, "{}", rows[84].text());
}

/// TM-Code 3: without colour the fences come back, dim, with the word of the fence
/// as typed and no file; the code is plain, a comment italic; a diff in a fence has
/// a bold sign and a dim hunk header.
#[test]
fn code_blocks_without_colour_are_the_board() {
    let frame = Frame::load("Code", 3).unwrap();
    let out = render(&CODE.concat(), &code_options(Depth::NoColor, frame.cols));
    let real = soft_wrap(&out, frame.cols);
    let lines: Vec<&str> = real.split_inclusive('\n').collect();
    assert_eq!(lines.len(), frame.rows.len());
    // NOTE: a removed row of a diff is dim without colour (TM-Legend 21, TM-Diffs 3,
    // spec 1.3); TM-Code 3 draws row 90 plain.
    assert_rows(&frame, ..90, &lines[..90].concat(), Depth::NoColor);
    assert_rows(&frame, 91.., &lines[91..].concat(), Depth::NoColor);
    let rows = cells(lines[90]);
    assert_eq!(rows[0].text(), frame.rows[90].text);
    assert_eq!(rows[0].cells[0].look, Look { bold: true, ..Look::default() });
    assert_eq!(rows[0].cells[9].look, Look { dim: true, ..Look::default() });
}

/// The tokens that a rule of the theme colours, against the board in truecolor and
/// in 16 colours: keywords, the names of functions and types, numbers, strings,
/// escapes, comments, operators, lifetimes, `self` (outside a parameter list, where
/// the grammar scopes it as a parameter).
#[test]
fn the_tokens_that_a_theme_rule_gives_are_the_board() {
    let tokens: &[(usize, &str)] = &[
        (3, "struct"),
        (3, "'a"),
        (5, "// drain steers before ending the turn"),
        (6, "pub"),
        (6, "finish"),
        (14, "self"),
        (7, "let"),
        (7, "="),
        (7, "0"),
        (8, "loop"),
        (12, "continue"),
        (15, "+="),
        (17, "\\n"),
        (23, "// \"claude-opus-5-5\" \u{2192} \"Opus 5.5\""),
        (26, "export"),
        (26, "function"),
        (26, "shortName"),
        (43, "# restart efrd, then show what it logged"),
        (59, "\"claude-opus-5-5\""),
        (62, "true"),
        (63, "200000"),
        (72, "\"eggfriedrice\""),
        (72, "# code blocks and diffs"),
    ];
    for depth in [Depth::TrueColor, Depth::Ansi16] {
        let frame = Frame::load("Code", 1).unwrap();
        let want = expect(&frame, depth).unwrap();
        let out = render(&CODE.concat(), &code_options(depth, frame.cols));
        let rows = screen(&out, frame.cols);
        for (row, token) in tokens {
            let text = &want.rows[*row].text;
            let column = text.find(token).map(|at| text[..at].chars().count()).unwrap();
            let segment = want.rows[*row]
                .segments
                .iter()
                .find(|segment| segment.text == *token)
                .unwrap_or_else(|| panic!("no segment {token:?} in row {row}"));
            let fill = rows[*row].fill;
            for offset in 0..token.chars().count() {
                let cell = rows[*row].cells[column + offset].look;
                assert_eq!(
                    plain(cell, fill),
                    plain(segment.look, want.rows[*row].fill),
                    "{depth:?} row {row} {token:?}"
                );
            }
        }
    }
}

/// TM-Code 4: a block that still streams. Each complete line commits on `code.bg`;
/// the open line is the live zone, on `code.bg` too. The rows under it are the status
/// row and the input row of the CLI.
#[test]
fn the_open_line_of_a_streaming_block_is_live_and_filled() {
    let frame = Frame::load("Code", 4).unwrap();
    let options = code_options(Depth::TrueColor, frame.cols);
    let text = CODE[..9].concat();
    let mut renderer = Renderer::new(options);
    let update = renderer.push(&format!("{text}        let Ok(\u{27e6}x.r: "));
    let screen = format!("{}{}", update.committed(), update.live());
    let rows = cells(&screen);
    // NOTE: `⟦x.r: ` of row 9 is a template mark of the board, not output.
    assert_code_rows(&frame, 0..10, &rows, Depth::TrueColor, |row| row != 9);
    assert!(frame.rows[9].live);
    assert_eq!(cells(update.live()).len(), 1);
}

/// TM-Code 5 and 6: a long line is never cut; the terminal wraps it, and the
/// background follows the wrapped row. At 160 columns it fits.
#[test]
fn a_long_line_of_code_wraps_on_the_terminal_with_its_fill() {
    let bash = CODE[41..55].concat();
    for number in [5, 6] {
        let frame = Frame::load("Code", number).unwrap();
        let out = render(&bash, &code_options(Depth::TrueColor, frame.cols));
        let rows = screen(&out, frame.cols);
        assert_code_rows(&frame, 0..frame.rows.len(), &rows, Depth::TrueColor, |_| true);
        let long = out.lines().last().unwrap();
        assert!(!long.contains('\n'), "efr writes the line as one");
    }
}

/// TM-Legend 38 and TM-Depths 4, 8, 12: the label `rust · src/turn.rs`, muted; the
/// code block of the Depths board in its three depths.
#[test]
fn the_code_block_is_the_legend_and_the_depths() {
    let legend = Frame::load("Legend", 38).unwrap();
    let block = "```rust src/turn.rs\nfn x() {}\n```\n";
    for depth in [Depth::TrueColor, Depth::Ansi16] {
        let out = render(block, &code_options(depth, legend.cols));
        let label: String = out.split_inclusive('\n').next().unwrap().to_owned();
        assert_rows(&legend, .., &label, depth);
    }
    let depths = Frame::load("Depths", 4).unwrap();
    let markdown = "```rust crates/efr-daemon/src/turn.rs\n\
pub fn finish(&mut self) -> usize {\n    \
while let Ok(msg) = self.rx.try_recv() { // drain first\n        \
self.pending.push(msg);\n    \
}\n\
}\n\
```\n";
    for depth in Depth::ALL {
        let frame = expect(&depths, depth).unwrap();
        let out = render(markdown, &code_options(depth, frame.cols));
        if depth == Depth::NoColor {
            assert_rows(&frame, .., &out, depth);
        } else {
            let rows = screen(&out, frame.cols);
            assert_code_rows(&frame, 0..frame.rows.len(), &rows, depth, |_| true);
        }
    }
}

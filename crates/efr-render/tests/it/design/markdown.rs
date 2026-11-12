//! TM-Markdown and the Markdown marks of TM-Legend: the marks stay where they carry
//! structure (`#`, `-`, the number of an item, `>`), in `muted`; a table is a grid
//! apart by spaces with one rule, or one record for each row.

use efr_render::render;
use efr_test_support::design::{CellRow, Depth, Frame, Ink, Look, assert_rows, cells};

use super::options;

/// The Markdown of TM-Markdown 1 and 2, with a line break at the end of each prose
/// row of the board (the indent of a nested item stands before the `\` that ends a
/// line of the literal, which drops the spaces after it). The `(18 s)` after the first numbered item is muted on the board:
/// no Markdown makes muted text but a link with no target shown, whose destination
/// follows in muted parentheses, so the mock gives the item such a link.
const SESSION_STORE: &str = "# Session store\n\
\n\
efrd keeps each conversation in one `SQLite` file. The **JSON reader** still builds, behind a feature,\n\
and runs *once* to import the old files. See [the storage notes](https://example.com/storage) or [docs/storage.md](https://example.com/docs/storage.md).\n\
\n\
## What changed\n\
\n\
- efrd opens one file at start instead of 412: `38 ms` instead of `1.9 s`\n\
- the importer reads `~/.local/share/efr/sessions` and leaves it in place\n  \
- a session with no `ended_at` takes its `started_at`\n  \
- a file that does not parse is skipped and named in the log\n\
- ~~the cache of parsed files~~ is gone\n\
\n\
### To do\n\
\n\
- [x] import the old files\n\
- [ ] drop `legacy-store` in 0.0.7\n\
\n\
#### Numbers\n\
\n\
1. build the store[](<18 s>)\n\
2. run the import on a copy\n\
3. compare the counts\n\
\n\
> Keep the JSON reader until two releases have shipped without it.\n\
> \u{2014} notes from the 0.0.5 review\n\
\n\
| store | open | write | size |\n\
|---|---|---|---|\n\
| json | `1.9 s` | 2.1 ms | 48 MB |\n\
| sqlite | `38 ms` | 0.4 ms | 31 MB |\n\
\n\
---\n\
\n\
A last paragraph after a rule. Top-level prose is not wrapped by efr: the terminal reflows it when\n\
the window changes, so a copy gives the author's lines back.\n";

/// The rows of the table in TM-Markdown 1 and 2.
const TABLE_ROWS: std::ops::Range<usize> = 27..31;

/// The rows of painted output, each with its newline.
fn lines(out: &str) -> Vec<&str> {
    out.split_inclusive('\n').collect()
}

/// The rows `range` of `out` as one text.
fn part(out: &str, range: std::ops::Range<usize>) -> String {
    lines(out)[range].concat()
}

/// The colour of the `text` role of the design: the terminal's own on the boards.
const TEXT: Ink = Ink::Rgb(0xd8, 0xd3, 0xc3);

/// The look of the cell at `column` of `row`, with the `text` role of the design as
/// the terminal's own colour, as the boards draw it.
fn look(row: &CellRow, column: usize) -> Look {
    let look = row.cells[column].look;
    if look.fg == TEXT { Look { fg: Ink::Default, ..look } } else { look }
}

/// TM-Markdown 1 (truecolor) and 2 (`NO_COLOR`): headings with their `#` marks, `-` at
/// each level of a list, the muted number, the task boxes, `>` before a quote, a rule,
/// inline code with backticks without colour. The table has a test of its own.
#[test]
fn markdown_marks_are_the_board() {
    for (number, depth) in [(1, Depth::TrueColor), (2, Depth::NoColor)] {
        let frame = Frame::load("Markdown", number).unwrap();
        let out = render(SESSION_STORE, &options(depth, frame.cols));
        assert_eq!(lines(&out).len(), frame.rows.len(), "{out}");
        assert_rows(&frame, ..TABLE_ROWS.start, &part(&out, 0..TABLE_ROWS.start), depth);
        let after = TABLE_ROWS.end..frame.rows.len();
        assert_rows(&frame, after.clone(), &part(&out, after), depth);
    }
}

/// TM-Markdown 1 in 16 colours, which the board does not draw: the frame through the
/// table of `expect`, but the headings of level 1 and 2 keep the accent (slot 3).
/// DECISIONS.md 3 beats the Legend's slot 1 for `heading`: the default stays the
/// accent, and a palette sets the colour.
#[test]
fn markdown_marks_in_sixteen_colours_keep_the_accent_for_headings() {
    let frame = Frame::load("Markdown", 1).unwrap();
    let out = render(SESSION_STORE, &options(Depth::Ansi16, frame.cols));
    for (from, to) in [(1, 5), (6, TABLE_ROWS.start), (TABLE_ROWS.end, frame.rows.len())] {
        assert_rows(&frame, from..to, &part(&out, from..to), Depth::Ansi16);
    }
    let rows = cells(&out);
    for (row, text, heading) in [(0, "# Session store", 2), (5, "## What changed", 3)] {
        assert_eq!(rows[row].text(), text);
        assert_eq!(look(&rows[row], 0), Look { dim: true, ..Look::default() });
        let underline = row == 0;
        let want = Look { fg: Ink::Slot(3), bold: true, underline, ..Look::default() };
        assert_eq!(look(&rows[row], heading), want, "{text}");
    }
}

/// The table of TM-Markdown 1 and 2: a grid with its columns apart by spaces, the
/// header bold, one muted rule as wide as the table under it, no padding at the end
/// of a row, inline code green and with backticks without colour.
///
/// The board places the columns by hand: gaps of 3 and 4 columns in frame 1, the rule
/// 8 columns wider than the table, and in frame 2 the cells with backticks no longer
/// under their header. Spec 1.2 ("Table, fits": one muted rule as wide as the table,
/// no row ends in padding) gives the rule here; the gap is 3 spaces, the gap of most
/// columns on the board.
#[test]
fn a_table_that_fits_is_a_grid_with_one_rule() {
    let rule = "\u{2500}".repeat(31);
    let truecolor = [
        "store    open    write    size",
        rule.as_str(),
        "json     1.9 s   2.1 ms   48 MB",
        "sqlite   38 ms   0.4 ms   31 MB",
    ];
    let rule = "\u{2500}".repeat(33);
    let no_colour = [
        "store    open      write    size",
        rule.as_str(),
        "json     `1.9 s`   2.1 ms   48 MB",
        "sqlite   `38 ms`   0.4 ms   31 MB",
    ];
    for (depth, want) in [(Depth::TrueColor, truecolor), (Depth::NoColor, no_colour)] {
        let out = render(SESSION_STORE, &options(depth, 100));
        let rows = cells(&out);
        let table = &rows[TABLE_ROWS];
        let texts: Vec<String> = table.iter().map(CellRow::text).collect();
        assert_eq!(texts, want, "{depth:?}");
        let bold = Look { bold: true, ..Look::default() };
        assert_eq!(look(&table[0], 0), bold);
        assert_eq!(look(&table[0], 9), bold);
        assert_eq!(look(&table[0], 6), Look::default(), "the gap is no part of the header");
        let muted = match depth {
            Depth::TrueColor => Look { fg: Ink::Rgb(0x86, 0x95, 0xb7), ..Look::default() },
            _ => Look { dim: true, ..Look::default() },
        };
        assert_eq!(look(&table[1], 0), muted);
        let code = match depth {
            Depth::TrueColor => Look { fg: Ink::Rgb(0x60, 0xe6, 0x54), ..Look::default() },
            _ => Look::default(),
        };
        assert_eq!(look(&table[2], 9), code);
    }
}

/// TM-Markdown 3: at 60 columns, a list item wraps at its indent, and a nested item
/// at its own. The board breaks the item rows before 60 columns, at the author's line
/// breaks; the mock gives them.
#[test]
fn lists_wrap_at_their_indent() {
    let frame = Frame::load("Markdown", 3).unwrap();
    let markdown = "## What changed\n\
\n\
- efrd opens one file at start instead of 412: `38 ms`\n  instead of `1.9 s`\n\
- the importer reads `~/.local/share/efr/sessions` and\n  leaves it in place\n  \
- a session with no `ended_at` takes its\n    `started_at`\n\
\n\
Top-level prose is not wrapped by efr: the terminal reflows\nit when the window changes.\n";
    let out = render(markdown, &options(Depth::TrueColor, frame.cols));
    assert_rows(&frame, .., &out, Depth::TrueColor);
    // A line wider than the item wraps at the item's indent too.
    let long = "- the importer reads `~/.local/share/efr/sessions` and leaves it in place\n";
    let rows = cells(&render(long, &options(Depth::TrueColor, frame.cols)));
    let texts: Vec<String> = rows.iter().map(CellRow::text).collect();
    assert_eq!(
        texts,
        ["- the importer reads ~/.local/share/efr/sessions and leaves", "  it in place"]
    );
}

const PROXY_TABLE: &str = "The proxy refused these hosts this week:\n\
\n\
| host | port | reason | times | first seen | conversation |\n\
|---|---|---|---|---|---|\n\
| index.crates.io | 443 | not on the allow list | 3 | 2026-11-09 14:02 | move the session store to SQLite |\n\
| registry.npmjs.org | 443 | denied for this command | 1 | 2026-11-10 09:40 | web: bump vite |\n";

/// TM-Markdown 4: the table fits 160 columns, so it is a grid. The board places the
/// columns by hand (gaps of 3 to 8 columns, the values of `times` and `first seen` 3
/// columns right of their header, the rule 13 columns wider than the table); the grid
/// takes the rule of TM-Markdown 1 (see `a_table_that_fits_is_a_grid_with_one_rule`).
#[test]
fn a_wide_table_that_fits_is_a_grid() {
    let frame = Frame::load("Markdown", 4).unwrap();
    let out = render(PROXY_TABLE, &options(Depth::TrueColor, frame.cols));
    assert_rows(&frame, ..2, &part(&out, 0..2), Depth::TrueColor);
    let rows = cells(&out);
    let texts: Vec<String> = rows[2..].iter().map(CellRow::text).collect();
    let rule = "\u{2500}".repeat(113);
    assert_eq!(
        texts,
        [
            "host                 port   reason                    times   first seen         conversation",
            rule.as_str(),
            "index.crates.io      443    not on the allow list     3       2026-11-09 14:02   move the session store to SQLite",
            "registry.npmjs.org   443    denied for this command   1       2026-11-10 09:40   web: bump vite",
        ]
    );
    let cells_of = |text: &str| -> Vec<String> {
        text.split("  ").map(str::trim).filter(|w| !w.is_empty()).map(str::to_owned).collect()
    };
    for (row, real) in [(4, &texts[2]), (5, &texts[3])] {
        assert_eq!(cells_of(real), cells_of(&frame.rows[row].text), "the cells of the board");
    }
}

/// TM-Markdown 5: the same table at 60 columns is one record for each row: the label
/// muted in a column of its own, two spaces, the value; the first value bold; a blank
/// line between records.
#[test]
fn a_table_too_wide_is_one_record_for_each_row() {
    let frame = Frame::load("Markdown", 5).unwrap();
    for depth in [Depth::TrueColor, Depth::Ansi16, Depth::NoColor] {
        let out = render(PROXY_TABLE, &options(depth, frame.cols));
        assert_rows(&frame, .., &out, depth);
    }
}

/// TM-Legend 37: `## What changed  - item  > quote`, the marks muted, the heading in
/// its role and bold, the quote italic in its role. The Legend draws the three marks
/// on one row; each is rendered alone and the rows are joined with its two spaces.
#[test]
fn the_markdown_marks_are_the_legend() {
    let frame = Frame::load("Legend", 37).unwrap();
    for depth in [Depth::TrueColor, Depth::Ansi16, Depth::NoColor] {
        let options = options(depth, frame.cols);
        let pieces: Vec<String> = ["## What changed\n", "- item\n", "> quote\n"]
            .iter()
            .map(|markdown| render(markdown, &options).trim_end_matches('\n').to_owned())
            .collect();
        let row = format!("{}\n", pieces.join("  "));
        if depth == Depth::Ansi16 {
            // DECISIONS.md 3: the heading keeps the accent (slot 3), not the Legend's
            // slot 1; the rest is the frame through the table.
            let rows = cells(&row);
            let heading = Look { fg: Ink::Slot(3), bold: true, ..Look::default() };
            assert_eq!(look(&rows[0], 3), heading);
            let rest = row.replacen("\x1b[1;33m", "\x1b[1;31m", 1);
            assert_rows(&frame, .., &rest, depth);
        } else {
            assert_rows(&frame, .., &row, depth);
        }
    }
}

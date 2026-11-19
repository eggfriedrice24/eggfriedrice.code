//! TM-Narrow: a finished row does not depend on the width of the window. efr writes
//! each paragraph, heading, list item and quote as one logical line, and the terminal
//! wraps it by cell ([`soft_wrap`]); a row of code has its tint under its text and 1
//! column after it. Each test renders the board's text once, at the width of the
//! frame "at 100 columns", and compares what the terminal shows at 100 columns and,
//! after the window shrinks, at 60 columns: the same output in both.

use efr_render::render;
use efr_test_support::design::{Depth, Frame, assert_rows, soft_wrap};

use super::{assert_code_rows, code_options, options, screen};

/// The prose of TM-Narrow 3 and 4.
const PROSE: &str = "## What the import does\n\
\n\
efrd keeps each conversation in one `SQLite` file. The JSON reader still builds, behind a \
feature, and runs once to import the old files. A file that does not parse is skipped and \
named in the log, so one bad file does not stop the import of the others.\n";

/// The list and the quote of TM-Narrow 7 and 8.
const LIST: &str = "- the importer reads `~/.local/share/efr/sessions` and leaves it in place, so \
a second run finds the same files and imports nothing twice\n  \
- a session with no `ended_at` takes its `started_at`, and the list of conversations sorts it \
by that time\n\
\n\
> Keep the JSON reader until two releases have shipped without it, then drop it in one commit.\n";

/// The code block of TM-Narrow 11 and 12.
const CODE: &str = "```rust crates/efr-daemon/src/turn.rs\n\
pub fn finish(&mut self) -> usize {\n    \
let mut drained = 0;\n    \
while let Ok(msg) = self.rx.try_recv() {\n        \
self.pending.push(msg);\n        \
drained += 1;\n    \
}\n    \
tracing::debug!(\"turn {} drained {} steers\", self.id, drained);\n    \
drained\n\
}\n\
```\n";

/// The output of `markdown` at 100 columns, as the terminal shows it in frame
/// `number` (100 columns) and `number + 1` (shrunk to 60).
fn check_both(markdown: &str, number: u32) {
    let out = render(markdown, &options(Depth::TrueColor, 100));
    for number in [number, number + 1] {
        let frame = Frame::load("Narrow", number).unwrap();
        assert_rows(&frame, .., &soft_wrap(&out, frame.cols), Depth::TrueColor);
    }
}

/// TM-Narrow 3 and 4: a paragraph is one logical row; the terminal wraps it, also
/// inside a word, and wraps it again in a narrower window, with no short rows.
#[test]
fn a_paragraph_and_a_heading_wrap_on_the_terminal() {
    check_both(PROSE, 3);
}

/// TM-Narrow 7 and 8: one logical row for each item, the marker at its start only; the
/// rows that go on start at column 0.
#[test]
fn a_list_item_and_a_quote_wrap_on_the_terminal() {
    check_both(LIST, 7);
}

/// TM-Narrow 11 and 12: `code.bg` under the text and 1 column after it; in a narrower
/// window only a row longer than the window wraps, with its tint.
#[test]
fn a_code_block_has_its_tint_under_its_text() {
    let out = render(CODE, &code_options(Depth::TrueColor, 100));
    assert!(!out.contains("\x1b[K"), "{out:?}");
    for number in [11, 12] {
        let frame = Frame::load("Narrow", number).unwrap();
        let rows = screen(&out, frame.cols);
        assert_code_rows(&frame, 0..frame.rows.len(), &rows, Depth::TrueColor, |_| true);
    }
}

/// The output does not depend on the width at which efr wrote it: prose, lists, quotes
/// and code come out the same at 40, 60 and 100 columns.
#[test]
fn the_rows_are_the_same_at_any_width() {
    for markdown in [PROSE, LIST, CODE] {
        let at_100 = render(markdown, &code_options(Depth::TrueColor, 100));
        for cols in [40, 60] {
            assert_eq!(render(markdown, &code_options(Depth::TrueColor, cols)), at_100);
        }
    }
}

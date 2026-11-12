use pretty_assertions::assert_eq;

use super::{
    DESIGN_ROLES, Depth, Frame, Ink, Look, assert_rows, cells, check_rows, check_rows_where,
    expect, rgb,
};

const YELLOW: Ink = Ink::Rgb(0xff, 0xc9, 0x40);
const MUTED: Ink = Ink::Rgb(0x86, 0x95, 0xb7);

#[test]
fn every_board_of_the_fixture_loads_with_its_depths() {
    let boards = [
        "BeforeAfter",
        "Calls",
        "Code",
        "Commands",
        "Depths",
        "Diffs",
        "Endings",
        "Legend",
        "Live",
        "LongTurn",
        "Markdown",
        "Questions",
        "Scope",
        "Sequence",
        "Web",
    ];
    for board in boards {
        let first = super::board_frames(&format!("TM-{board}")).unwrap();
        assert!(!first.is_empty(), "{board}");
        for frame in &first {
            assert!(frame.cols >= 40, "{board} {}", frame.number);
            assert_eq!(frame.foreground, Ink::Rgb(0xd8, 0xd3, 0xc3));
        }
    }
    let calls = Frame::load("Calls", 18).unwrap();
    assert_eq!((calls.depth, calls.variant_of), (Depth::Ansi16, Some(17)));
    assert_eq!(
        Frame::load("TM-Calls", 2).unwrap().texts(),
        ["\u{2713} $ cargo test -p efr-daemon steer  38s"]
    );
    assert!(Frame::load("Calls", 99).is_err());
    assert!(Frame::load("Nothing", 1).is_err());
}

#[test]
fn the_fixture_has_the_corrections_of_the_spec() {
    // No live row for the reasoning, and the detail at the end of the status row.
    let live = Frame::load("Live", 3).unwrap();
    assert_eq!(
        live.texts(),
        [
            "\u{283c} thinking   9s   ctx 12%   work/claude-opus-5-5 high   app   planning the migration"
        ]
    );
    // The diff sign at column 2.
    let sequence = Frame::load("Sequence", 8).unwrap();
    assert!(sequence.texts().contains(&"  -         } else {"), "{:?}", sequence.texts());
    // Mock names only.
    let sudo = Frame::load("Live", 22).unwrap();
    assert!(sudo.texts().iter().any(|row| row.contains("password for sam:")));
    // The frames that lose are not there.
    for (board, number) in [("BeforeAfter", 1), ("LongTurn", 5), ("Scope", 12), ("Depths", 13)] {
        assert!(Frame::load(board, number).is_err(), "{board} {number}");
    }
}

#[test]
fn the_web_board_has_its_corrections_and_its_depths() {
    // A run of searches is one row each, never folded, and a find gets no row.
    let run = Frame::load("Web", 3).unwrap();
    assert_eq!(run.texts().len(), 5);
    assert!(run.texts().iter().all(|row| row.starts_with("\u{2713} web ")), "{:?}", run.texts());
    let steps = Frame::load("Web", 5).unwrap();
    assert_eq!(steps.texts().len(), 5);
    assert!(steps.texts().iter().all(|row| !row.contains("web find")), "{:?}", steps.texts());
    // No row of today's end line.
    let ends = Frame::load("Web", 13).unwrap();
    assert_eq!(
        ends.texts()[0],
        "\u{2713} done  42s \u{b7} ctx 43% \u{b7} 1.1k out \u{b7} cache 91%"
    );
    // The depths that the board draws.
    let no_color = Frame::load("Web", 11).unwrap();
    assert_eq!((no_color.depth, no_color.variant_of), (Depth::NoColor, Some(10)));
    let depths: Vec<Depth> =
        [16, 17, 18, 19].map(|number| Frame::load("Web", number).unwrap().depth).to_vec();
    assert_eq!(depths, [Depth::Ansi16, Depth::NoColor, Depth::Ansi16, Depth::NoColor]);
}

#[test]
fn cells_read_sgr_erase_and_hyperlinks() {
    let rows = cells(
        "\x1b[1;38;2;255;201;64m!\x1b[0m \x1b]8;;https://x\x1b\\link\x1b]8;;\x1b\\\n\x1b[48;2;18;23;34mcode\x1b[K\x1b[0m\n",
    );
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].text(), "! link");
    assert_eq!(rows[0].cells[0].look, Look { fg: YELLOW, bold: true, ..Look::default() });
    assert_eq!(rows[0].cells[1].look, Look::default());
    assert_eq!(rows[1].text(), "code");
    assert_eq!(rows[1].fill, Some(Ink::Rgb(0x12, 0x17, 0x22)));
    let sixteen = cells("\x1b[2mmuted\x1b[22m \x1b[33;7mbadge\x1b[27m\x1b[94mx\x1b[39m\ry");
    assert_eq!(sixteen[0].text(), "yuted badgex");
    assert_eq!(sixteen[0].cells[1].look, Look { dim: true, ..Look::default() });
    assert_eq!(
        sixteen[0].cells[6].look,
        Look { fg: Ink::Slot(3), reverse: true, ..Look::default() }
    );
    assert_eq!(sixteen[0].cells[11].look.fg, Ink::Slot(12));
}

#[test]
fn a_frame_of_another_depth_is_the_boards_own_or_derived_through_the_table() {
    // The board's own frame in 16 colours.
    let truecolor = Frame::load("Calls", 17).unwrap();
    assert_eq!(expect(&truecolor, Depth::Ansi16).unwrap().number, 18);
    assert_eq!(expect(&truecolor, Depth::NoColor).unwrap().number, 19);
    // Derived: a muted row is dim, a warning bold without colour, a badge bold.
    let legend = Frame::load("Legend", 23).unwrap();
    let ansi = expect(&legend, Depth::Ansi16).unwrap();
    let badge = ansi.rows[0].segments[0].look;
    assert_eq!(badge, Look { fg: Ink::Slot(3), bold: true, reverse: true, ..Look::default() });
    let none = expect(&legend, Depth::NoColor).unwrap();
    assert_eq!(none.rows[0].segments[0].look, Look { bold: true, ..Look::default() });
    let muted = expect(&Frame::load("Legend", 7).unwrap(), Depth::Ansi16).unwrap();
    assert_eq!(muted.rows[0].segments[2].look, Look { dim: true, ..Look::default() });
}

#[test]
fn the_check_compares_text_colour_and_attributes_of_each_cell() {
    let frame = Frame::load("Legend", 7).unwrap();
    let green = "\x1b[38;2;96;230;84m";
    let muted = "\x1b[38;2;134;149;183m";
    let good =
        format!("{green}\u{2713}\x1b[0m {muted}$ cargo test -p app\x1b[0m  {muted}38s\x1b[0m\n");
    assert_rows(&frame, .., &good, Depth::TrueColor);
    // A trailing space with no colour is a blank, as on a terminal.
    assert_rows(&frame, .., &format!("{}  ", good.trim_end()), Depth::TrueColor);
    let bold = good.replacen(green, "\x1b[1;38;2;96;230;84m", 1);
    let problem = check_rows(&frame, .., &bold, Depth::TrueColor).unwrap_err();
    assert!(problem.contains("column 0"), "{problem}");
    let text = good.replace("38s", "39s");
    assert!(check_rows(&frame, .., &text, Depth::TrueColor).unwrap_err().contains("column 24"));
    let rows = format!("{good}{good}");
    assert!(
        check_rows(&frame, .., &rows, Depth::TrueColor).unwrap_err().contains("1 rows expected")
    );
    // In 16 colours the derived frame: the mark in slot 2, the rest dim.
    let sixteen = "\x1b[32m\u{2713}\x1b[0m \x1b[2m$ cargo test -p app\x1b[0m  \x1b[2m38s\x1b[0m\n";
    assert_rows(&frame, .., sixteen, Depth::Ansi16);
    // The terminal's own foreground and the text role of the design compare equal.
    let prose = Frame::load("Legend", 9).unwrap();
    assert_rows(
        &prose,
        ..,
        "    \x1b[38;2;216;211;195mmake: *** No rule to make target\x1b[0m\n",
        Depth::TrueColor,
    );
    assert_rows(&prose, .., "    make: *** No rule to make target\n", Depth::TrueColor);
}

#[test]
fn the_design_roles_are_the_sixteen_roles_with_their_hex() {
    assert_eq!(DESIGN_ROLES.len(), 16);
    assert!(DESIGN_ROLES.iter().all(|(_, hex)| rgb(hex).is_some()));
    assert_eq!(DESIGN_ROLES[1], ("muted", "#8695b7"));
    assert_eq!(rgb("#ffc940"), Some((0xff, 0xc9, 0x40)));
    assert_eq!(rgb("ffc940"), None);
    assert_eq!(Ink::Rgb(0xff, 0xc9, 0x40), YELLOW);
    assert_ne!(MUTED, YELLOW);
}

#[test]
fn a_cell_on_the_fill_of_its_row_compares_as_a_cell_on_no_background() {
    // TM-Legend 38 has no fill; TM-Code 1 row 87 is a code row of plain text on
    // `code.bg`.
    let frame = Frame::load("Code", 1).unwrap();
    let fill = "\x1b[48;2;18;23;34m";
    let text = "efrd 0.0.5 \u{b7} protocol 1";
    // The fill behind each cell and to the edge, and the fill only to the edge.
    let behind = format!("{fill}{text}\x1b[K\x1b[0m\n");
    assert_rows(&frame, 87..88, &behind, Depth::TrueColor);
    let edge = format!("{text}{fill}\x1b[K\x1b[0m\n");
    assert_rows(&frame, 87..88, &edge, Depth::TrueColor);
    // A cell on another background still differs.
    let other = format!("\x1b[48;2;1;2;3m{text}\x1b[0m{fill}\x1b[K\x1b[0m\n");
    let problem = check_rows(&frame, 87..88, &other, Depth::TrueColor).unwrap_err();
    assert!(problem.contains("column 0"), "{problem}");
    // No fill at all differs too.
    let bare = format!("{text}\n");
    assert!(check_rows(&frame, 87..88, &bare, Depth::TrueColor).unwrap_err().contains("fill"));
}

#[test]
fn a_cell_on_the_fill_of_its_row_is_the_row_and_a_part_can_be_text_only() {
    // TM-Legend 21: an added row on `diff.add.bg`. efr writes each cell on the fill
    // and erases the rest of the row with it; the board draws the fill behind the row.
    let frame = Frame::load("Legend", 21).unwrap();
    let fill = "\x1b[48;2;25;44;23m";
    let sign = "\x1b[1;38;2;96;230;84;48;2;25;44;23m+\x1b[0m";
    let code = format!("\x1b[38;2;192;132;252mlet\x1b[0m{fill} x = 1;");
    let row = format!("{fill}  {sign}{fill} {code}\x1b[K\x1b[0m\n");
    // The colours of the code are not the board's: the check fails on them, and passes
    // when they compare by their text.
    let problem = check_rows(&frame, .., &row, Depth::TrueColor).unwrap_err();
    assert!(problem.contains("column 8"), "{problem}");
    check_rows_where(&frame, .., &row, Depth::TrueColor, |_, column| column >= 4).unwrap();
    // The text still counts.
    let other = row.replace("x = 1", "y = 1");
    assert!(
        check_rows_where(&frame, .., &other, Depth::TrueColor, |_, column| column >= 4).is_err()
    );
}

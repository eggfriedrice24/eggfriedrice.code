use efr_render::{ColourMode, RenderOptions, WidthMethod, display_width};
use pretty_assertions::assert_eq;

use super::{ALLOW_KEYS, Card, Footer, Row, approval};
use crate::format::{CommandRow, RowEnd, Tone, WORD_MARK, WRAP_MARK, wrap_command, wrap_spans};
use crate::testing::readable;

fn plain(columns: u16) -> RenderOptions {
    RenderOptions::new(columns).with_colour(ColourMode::None)
}

/// `text` without its escape sequences.
fn bare(text: &str) -> String {
    let mut out = String::new();
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            for c in chars.by_ref() {
                if c.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn row(text: &str, indented: bool, end: RowEnd) -> CommandRow {
    CommandRow { text: text.to_owned(), indented, end }
}

#[test]
fn a_command_goes_on_after_a_space_with_a_backslash() {
    let rows = wrap_command(
        "cd ~/p/app && find target/debug/build -path '*ghostty*' -type f",
        40,
        6,
        WidthMethod::CodePoint,
    );
    assert_eq!(
        rows,
        [
            row("cd ~/p/app && find target/debug/build ", false, RowEnd::Space),
            row("-path '*ghostty*' -type f", true, RowEnd::Last),
        ]
    );
    // The rows hold the whole line.
    let joined: String = rows.iter().map(|row| row.text.as_str()).collect();
    assert_eq!(joined, "cd ~/p/app && find target/debug/build -path '*ghostty*' -type f");
}

#[test]
fn a_row_is_cut_at_a_space_even_early_in_the_row() {
    // The space after `echo` is the only one: the path goes on in the next row,
    // indented, and only the rows of the path itself are cut inside it, each at the
    // column of the row before.
    let path = format!("/a/{}", "b".repeat(30));
    let rows = wrap_command(&format!("echo {path}"), 20, 4, WidthMethod::CodePoint);
    assert_eq!(rows[0], row("echo ", false, RowEnd::Space));
    assert!(rows[1..rows.len() - 1].iter().all(|row| row.end == RowEnd::Word), "{rows:?}");
    assert!(rows[1..].iter().all(|row| row.indented), "{rows:?}");
    assert_eq!(rows.last().unwrap().end, RowEnd::Last);
    for row in &rows[1..] {
        let mark = usize::from(row.mark().is_some());
        assert!(display_width(&row.text, WidthMethod::CodePoint) + mark <= 16, "{rows:?}");
    }
}

#[test]
fn a_word_wider_than_a_row_is_cut_inside_it_and_nothing_is_lost() {
    let word = "x".repeat(25);
    let rows = wrap_command(&word, 10, 4, WidthMethod::CodePoint);
    let joined: String = rows.iter().map(|row| row.text.as_str()).collect();
    assert_eq!(joined, word);
    // No row is indented after a cut inside a word: each has the whole room.
    for row in &rows {
        let mark = usize::from(row.mark().is_some());
        assert!(display_width(&row.text, WidthMethod::CodePoint) + mark <= 10, "{rows:?}");
        assert!(!row.indented, "{rows:?}");
    }
    assert!(rows[..rows.len() - 1].iter().all(|row| row.end == RowEnd::Word), "{rows:?}");
    assert_eq!(rows.last().unwrap().end, RowEnd::Last);
    // A row too narrow for anything still takes one character.
    assert_eq!(wrap_command("ab", 1, 1, WidthMethod::CodePoint).len(), 2);
}

/// The command that the rows of a rendered card hold: the bar goes, the indent of the
/// rows after a cut at a space goes, and the marks go. Nothing else goes, so a space
/// that a cut inside a word added would be a space of the command.
fn command_of(shown: &str) -> String {
    let mut command = String::new();
    let mut indented = false;
    for row in bare(shown).lines().skip(1).filter(|row| !row.contains(" allow ")) {
        let mut row = row.strip_prefix("\u{2502} ").unwrap();
        if indented {
            row = row.strip_prefix("    ").unwrap();
        }
        if let Some(cut) = row.strip_suffix(WRAP_MARK) {
            row = cut;
            indented = true;
        } else if let Some(cut) = row.strip_suffix(WORD_MARK) {
            row = cut;
        } else {
            // The end of one line of the command: the next one starts after the bar.
            indented = false;
        }
        command.push_str(row);
    }
    command
}

/// A line whose path is wider than a row at 40 columns, as a model writes one.
const LONG_ECHO: &str = "echo \"built $HOME/p/eggfriedrice.code/target/debug/build/libghostty-vt-1a2b3c4d5e6f/out/lib/libghostty-vt.so\"";

#[test]
fn a_cut_inside_a_word_shows_no_space_that_the_command_does_not_have() {
    let card = approval(&format!("shell: run {LONG_ECHO:?}"), Some("shell"), Some(LONG_ECHO));
    for columns in [24_u16, 40, 80] {
        let shown = card.render(Some(Footer::Keys(ALLOW_KEYS)), &plain(columns));
        let rows: Vec<String> = bare(&shown).lines().map(str::to_owned).collect();
        for (at, row) in rows.iter().enumerate() {
            assert!(display_width(row, WidthMethod::CodePoint) <= usize::from(columns), "{row:?}");
            // A backslash and the indent after it stand for a space, so one is there.
            if let Some(before) = row.strip_suffix(WRAP_MARK) {
                assert!(before.ends_with(' '), "{columns}: {shown}");
            }
            // The row after a cut inside a word starts at the column of the word.
            if let Some(before) = row.strip_suffix(WORD_MARK) {
                let column = |row: &str| row.len() - row.trim_start_matches(' ').len();
                let (this, next) =
                    (&before["\u{2502} ".len()..], &rows[at + 1]["\u{2502} ".len()..]);
                assert_eq!(column(this), column(next), "{columns}: {shown}");
                assert!(!before.ends_with(' '), "{columns}: {shown}");
            }
        }
        assert_eq!(command_of(&shown), LONG_ECHO, "{columns}: {shown}");
    }
    let shown = card.render(Some(Footer::Keys(ALLOW_KEYS)), &plain(40));
    insta::assert_snapshot!(bare(&shown));
}

#[test]
fn text_goes_on_at_spaces_and_keeps_its_tones() {
    let spans = vec![
        ("programs: cd, ".to_owned(), Tone::Dim),
        ("./setup.sh".to_owned(), Tone::Dim),
        (" (untrusted: written in the sandbox)".to_owned(), Tone::Attention),
    ];
    let rows = wrap_spans(&spans, 30, 28, WidthMethod::CodePoint);
    assert_eq!(
        rows,
        [
            vec![("programs: cd, ./setup.sh".to_owned(), Tone::Dim)],
            vec![("(untrusted: written in the".to_owned(), Tone::Attention)],
            vec![("sandbox)".to_owned(), Tone::Attention)],
        ]
    );
}

#[test]
fn a_word_wider_than_the_first_row_starts_in_it() {
    let url = format!("https://example.com/{}", "a".repeat(40));
    let rows = wrap_spans(&[(url.clone(), Tone::Dim)], 20, 18, WidthMethod::CodePoint);
    assert!(rows.iter().all(|row| !row.is_empty()), "{rows:?}");
    assert_eq!(rows[0], [(url[..20].to_owned(), Tone::Dim)]);
    assert!(rows[1..].iter().all(|row| row[0].0.len() <= 18), "{rows:?}");
    let joined: String = rows.iter().map(|row| row[0].0.as_str()).collect();
    assert_eq!(joined, url);

    // A row of a card at a narrow width.
    let card = Card { title: "allow this call".to_owned(), rows: vec![Row::text(url, Tone::Dim)] };
    let shown = bare(&card.render(None, &plain(24)));
    assert!(shown.lines().all(|row| row.trim_end() != "\u{2502}"), "{shown}");
    assert!(
        shown.lines().nth(1).is_some_and(|row| row.starts_with("\u{2502} https://")),
        "{shown}"
    );
}

fn command_card() -> Card {
    approval(
        "shell: run \"cd ~/p/eggfriedrice.code && find target/debug/build -path '*libghostty*' -type f | awk '{print $1}'\"",
        Some("shell"),
        Some(
            "cd ~/p/eggfriedrice.code && find target/debug/build -path '*libghostty*' -type f | awk '{print $1}'",
        ),
    )
}

#[test]
fn every_row_of_a_card_fits_the_screen_and_the_command_shows_whole() {
    let card = command_card();
    for columns in [24_u16, 40, 80] {
        let shown = card.render(Some(Footer::Keys(ALLOW_KEYS)), &plain(columns));
        for row in bare(&shown).lines() {
            let width = display_width(row, WidthMethod::CodePoint);
            assert!(width <= usize::from(columns), "{columns}: {row:?}");
        }
        // Without the bar, the indent and the marks, the rows give the command back.
        assert_eq!(
            command_of(&shown),
            "cd ~/p/eggfriedrice.code && find target/debug/build -path '*libghostty*' -type f | awk '{print $1}'"
        );
    }
}

#[test]
fn a_card_at_40_and_80_columns_with_colour_and_without() {
    let card = command_card();
    let mut shown = Vec::new();
    for columns in [40_u16, 80] {
        for (name, options) in
            [("colour", RenderOptions::new(columns)), ("NO_COLOR", plain(columns))]
        {
            let text = card.render(Some(Footer::Keys(ALLOW_KEYS)), &options);
            shown.push(format!("{columns} columns, {name}:\n{}", readable(&text)));
        }
    }
    let piped = card.render(Some(Footer::Waiting), &RenderOptions::new(40).with_terminal(false));
    shown.push(format!("not a terminal:\n{piped}"));
    insta::assert_snapshot!(shown.join("\n"));
}

#[test]
fn a_summary_that_does_not_quote_the_command_shows_as_one_row() {
    let card = approval("write_file: write ~/.zshrc (user config)", Some("write_file"), None);
    assert_eq!(card.title, "allow this write");
    assert_eq!(card.rows, [Row::text("write ~/.zshrc (user config)", Tone::Plain)]);
    let card = approval(
        "shell: run \"cd src\\nls\"; read ~ (home)\nasks for: ls",
        Some("shell"),
        Some("cd src\nls"),
    );
    assert_eq!(
        card.rows,
        [
            Row::Command("cd src".to_owned()),
            Row::Command("ls".to_owned()),
            Row::text("why: read ~ (home)", Tone::Dim),
            Row::text("asks for: ls", Tone::Dim),
        ]
    );
}

#[test]
fn a_command_keeps_its_control_and_format_characters_as_stand_ins() {
    let card =
        approval("shell: run \"echo a\\u{202e}b\\r\"", Some("shell"), Some("echo a\u{202e}b\r"));
    assert_eq!(card.rows, [Row::Command("echo a\u{fffd}b\u{240d}".to_owned())]);
}

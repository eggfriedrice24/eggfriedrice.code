use efr_render::{ColourMode, RenderOptions, WidthMethod, display_width};
use pretty_assertions::assert_eq;

use super::{ALLOW_KEYS, Card, Footer, Row, approval};
use crate::format::{Tone, wrap_command, wrap_spans};
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

#[test]
fn a_command_goes_on_after_a_space_with_a_backslash() {
    let rows = wrap_command(
        "cd ~/p/app && find target/debug/build -path '*ghostty*' -type f",
        40,
        34,
        WidthMethod::CodePoint,
    );
    assert_eq!(
        rows,
        [
            ("cd ~/p/app && find target/debug/build ".to_owned(), true),
            ("-path '*ghostty*' -type f".to_owned(), false),
        ]
    );
    // The rows hold the whole line.
    let joined: String = rows.iter().map(|(text, _)| text.as_str()).collect();
    assert_eq!(joined, "cd ~/p/app && find target/debug/build -path '*ghostty*' -type f");
}

#[test]
fn a_word_wider_than_a_row_is_cut_inside_it_and_nothing_is_lost() {
    let word = "x".repeat(25);
    let rows = wrap_command(&word, 10, 6, WidthMethod::CodePoint);
    let joined: String = rows.iter().map(|(text, _)| text.as_str()).collect();
    assert_eq!(joined, word);
    for (text, goes_on) in &rows {
        let room = if text.len() == 9 { 10 } else { 6 };
        assert!(display_width(text, WidthMethod::CodePoint) + usize::from(*goes_on) <= room);
    }
    assert!(!rows.last().unwrap().1);
    // A row too narrow for anything still takes one character.
    assert_eq!(wrap_command("ab", 1, 1, WidthMethod::CodePoint).len(), 2);
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
        let command: String = bare(&shown)
            .lines()
            .skip(1)
            .filter(|row| !row.contains(" allow "))
            .map(|row| {
                let row = row.trim_start_matches("\u{2502} ").trim_start();
                row.strip_suffix('\\').unwrap_or(row).to_owned()
            })
            .collect();
        assert_eq!(
            command,
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

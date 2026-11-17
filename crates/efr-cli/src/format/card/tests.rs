use efr_protocol::{Origin, QuestionOption, QuestionReply, UserAnswer};
use efr_render::{ColourMode, Glyphs, RenderOptions, WidthMethod, display_width};
use pretty_assertions::assert_eq;

use super::{
    ALLOW_KEYS, Asks, Card, Footer, QuestionEnd, Row, approval, footer_row, question_end,
    user_question, user_question_footer,
};
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
fn a_space_early_in_the_row_does_not_leave_a_short_word_alone() {
    // The space after `cp` is the only one, and a cut there would fill 3 columns of
    // 20: the row is cut inside the path instead, and the path goes on at the same
    // column, not indented.
    let path = format!("/a/{}", "b".repeat(30));
    let rows = wrap_command(&format!("cp {path}"), 20, 4, WidthMethod::CodePoint);
    assert_eq!(rows[0], row(&format!("cp {}", &path[..16]), false, RowEnd::Word));
    assert!(rows[..rows.len() - 1].iter().all(|row| row.end == RowEnd::Word), "{rows:?}");
    assert!(rows.iter().all(|row| !row.indented), "{rows:?}");
    assert_eq!(rows.last().unwrap().end, RowEnd::Last);
    for row in &rows {
        let mark = usize::from(row.mark_in(&Glyphs::UNICODE).is_some());
        assert!(display_width(&row.text, WidthMethod::CodePoint) + mark <= 20, "{rows:?}");
    }
    let joined: String = rows.iter().map(|row| row.text.as_str()).collect();
    assert_eq!(joined, format!("cp {path}"));
}

#[test]
fn a_space_cuts_a_row_that_is_at_least_half_full() {
    // `echo hello ` fills 11 columns of 20: the cut stays at the space.
    let rows = wrap_command("echo hello /a/bbbbbbbbbbbbbbbbbbbb", 20, 4, WidthMethod::CodePoint);
    assert_eq!(rows[0], row("echo hello ", false, RowEnd::Space));
    assert!(rows[1].indented, "{rows:?}");
    // `echo hi ` fills 8 of 20, less than half: the row is cut inside the path.
    let rows = wrap_command("echo hi /a/bbbbbbbbbbbbbbbbbbbbbbbb", 20, 4, WidthMethod::CodePoint);
    assert_eq!(rows[0], row("echo hi /a/bbbbbbbb", false, RowEnd::Word));
    assert!(!rows[1].indented, "{rows:?}");
}

#[test]
fn a_word_wider_than_a_row_is_cut_inside_it_and_nothing_is_lost() {
    let word = "x".repeat(25);
    let rows = wrap_command(&word, 10, 4, WidthMethod::CodePoint);
    let joined: String = rows.iter().map(|row| row.text.as_str()).collect();
    assert_eq!(joined, word);
    // No row is indented after a cut inside a word: each has the whole room.
    for row in &rows {
        let mark = usize::from(row.mark_in(&Glyphs::UNICODE).is_some());
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
        let mut row = row.strip_prefix("    ").unwrap();
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
        let shown = card.render(Some(&Footer::Keys(ALLOW_KEYS)), &plain(columns));
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
                let (this, next) = (&before["    ".len()..], &rows[at + 1]["    ".len()..]);
                assert_eq!(column(this), column(next), "{columns}: {shown}");
                assert!(!before.ends_with(' '), "{columns}: {shown}");
            }
        }
        assert_eq!(command_of(&shown), LONG_ECHO, "{columns}: {shown}");
    }
    let shown = card.render(Some(&Footer::Keys(ALLOW_KEYS)), &plain(40));
    insta::assert_snapshot!(bare(&shown));
}

#[test]
fn text_goes_on_at_spaces_and_keeps_its_tones() {
    let spans = vec![
        ("programs  cd, ".to_owned(), Tone::Dim),
        ("./setup.sh".to_owned(), Tone::Dim),
        (" (untrusted: written in the sandbox)".to_owned(), Tone::Attention),
    ];
    let rows = wrap_spans(&spans, 30, 28, WidthMethod::CodePoint);
    assert_eq!(
        rows,
        [
            vec![("programs  cd, ./setup.sh".to_owned(), Tone::Dim)],
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
    assert!(shown.lines().nth(1).is_some_and(|row| row.starts_with("    https://")), "{shown}");
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
        let shown = card.render(Some(&Footer::Keys(ALLOW_KEYS)), &plain(columns));
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
            let text = card.render(Some(&Footer::Keys(ALLOW_KEYS)), &options);
            shown.push(format!("{columns} columns, {name}:\n{}", readable(&text)));
        }
    }
    let piped = card.render(Some(&Footer::Waiting), &RenderOptions::new(40).with_terminal(false));
    shown.push(format!("not a terminal:\n{piped}"));
    insta::assert_snapshot!(shown.join("\n"));
}

#[test]
fn each_tool_names_what_a_yes_allows() {
    let title = |tool| approval("x", Some(tool), None).title;
    assert_eq!(title("shell"), "allow this command");
    assert_eq!(title("write_file"), "allow this write");
    assert_eq!(title("apply_patch"), "allow this patch");
    assert_eq!(title("edit"), "allow this edit");
    assert_eq!(title("read_file"), "allow this read");
    assert_eq!(approval("x", None, None).title, "allow this call");
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
            Row::labelled("why", "read ~ (home)"),
            Row::labelled("asks for", "ls"),
        ]
    );
}

#[test]
fn a_wake_question_keeps_its_command_rows() {
    // efrd puts the mark of a job notice turn in front of the summary; the card shows it
    // in the title and keeps the command rows and the `why:` row.
    let summary = format!(
        "{}shell: run \"cd src\\nmake check\"; read ~ (home)\nasks for: make",
        efr_protocol::WAKE_QUESTION
    );
    let card = approval(&summary, Some("shell"), Some("cd src\nmake check"));
    assert_eq!(card.title, "allow this command (job notice turn)");
    assert_eq!(
        card.rows,
        [
            Row::Command("cd src".to_owned()),
            Row::Command("make check".to_owned()),
            Row::labelled("why", "read ~ (home)"),
            Row::labelled("asks for", "make"),
        ]
    );
    // A question that full mode still asks keeps its guard row first.
    let summary = format!(
        "{}full mode still asks for this: delete of ~ (your home directory)",
        efr_protocol::WAKE_QUESTION
    );
    let guarded = approval(&summary, Some("shell"), Some("rm -rf ~"));
    assert_eq!(guarded.title, "allow this command (job notice turn)");
    assert_eq!(
        guarded.rows,
        [
            Row::text(
                "full mode still asks for this: delete of ~ (your home directory)",
                Tone::Attention
            ),
            Row::Command("rm -rf ~".to_owned()),
        ]
    );
    insta::assert_snapshot!(readable(&card.render(Some(&Footer::Keys(ALLOW_KEYS)), &plain(40))));
}

#[test]
fn a_command_keeps_its_control_and_format_characters_as_stand_ins() {
    let card =
        approval("shell: run \"echo a\\u{202e}b\\r\"", Some("shell"), Some("echo a\u{202e}b\r"));
    assert_eq!(card.rows, [Row::Command("echo a\u{fffd}b\u{240d}".to_owned())]);
}

#[test]
fn the_always_key_and_its_confirm_follow_the_keys_and_wrap_at_the_indent() {
    let always = Footer::Always("write under ~/notes".to_owned());
    assert_eq!(bare(&footer_row(&always, &plain(80))), "    y allow   n deny   a always\u{2026}\n");
    let confirm = Footer::Confirm("write under ~/notes".to_owned());
    assert_eq!(
        bare(&footer_row(&confirm, &plain(80))),
        "    always saves the rule  write under ~/notes\n    y save the rule and allow   n back\n"
    );
    // A long rule goes on in the next row, two columns further in, never past the screen.
    let long = Footer::Confirm("write under ~/projects/a-rather-long-name/notes".to_owned());
    let rows = bare(&footer_row(&long, &plain(30)));
    assert!(rows.lines().count() > 2, "{rows}");
    for line in rows.lines() {
        assert!(line.starts_with("    "), "{rows}");
        assert!(display_width(line, WidthMethod::Grapheme) <= 30, "{rows}");
    }
    // Without a rule, the keys paint as they always did.
    let painted = footer_row(&Footer::Keys(ALLOW_KEYS), &RenderOptions::new(40));
    let card = Card { title: "t".to_owned(), rows: Vec::new() };
    let whole = card.render(Some(&Footer::Keys(ALLOW_KEYS)), &RenderOptions::new(40));
    assert!(whole.ends_with(&painted), "{whole:?}");
    assert_eq!(bare(&painted), "    y allow   n deny\n");
}

/// An option of the model's question.
fn option(label: &str, description: Option<&str>) -> QuestionOption {
    QuestionOption { label: label.to_owned(), description: description.map(str::to_owned) }
}

/// The options of TM-Questions 0.0.12, frame 1.
fn databases() -> Vec<QuestionOption> {
    vec![
        option("Postgres 16", Some("the same as production; I recommend it")),
        option("SQLite", Some("fast and needs no server, but it has no JSONB")),
        option("both", Some("each test runs twice, so CI takes about 2 times longer")),
    ]
}

/// The options of TM-Questions 0.0.12, frame 2: one description goes on.
fn backups() -> Vec<QuestionOption> {
    vec![
        option("the NAS", Some("/mnt/nas/backups on the LAN; I recommend it")),
        option(
            "Backblaze B2",
            Some(
                "off site, about 2 USD a month for 400 GB; restic needs the key in \
                 ~/.config/restic/b2.env, and that file does not exist yet",
            ),
        ),
        option("both", Some("the NAS each night and B2 each Sunday")),
        option("an external disk", Some("only when it is plugged in")),
    ]
}

/// The card of the model's question as `asks` says, rendered with `options`.
fn question(text: &str, options: &[QuestionOption], asks: Asks<'_>, at: &RenderOptions) -> String {
    let card = user_question(text, options, asks, at.glyphs());
    card.render(Some(&user_question_footer(asks)), at)
}

#[test]
fn the_card_of_the_models_question_in_each_state_is_questions_0012() {
    const DATABASE: &str = "Which database should the tests use?";
    let frames = [
        question(DATABASE, &databases(), Asks::Options, &plain(100)),
        question("Where should the nightly backup go?", &backups(), Asks::Options, &plain(100)),
        question("Where should the nightly backup go?", &backups(), Asks::Options, &plain(60)),
        question(
            DATABASE,
            &databases(),
            Asks::Typing("Postgres 16, but on port 5433"),
            &plain(100),
        ),
        question(DATABASE, &databases(), Asks::Elsewhere, &plain(100)),
        question(
            &format!("{}Which target should I fix first?", efr_protocol::WAKE_QUESTION),
            &databases()[..2],
            Asks::Options,
            &plain(100),
        ),
    ];
    insta::assert_snapshot!(frames.map(|frame| bare(&frame)).join("\n---\n"));
}

#[test]
fn a_digit_is_a_key_only_while_it_answers() {
    let digit = |asks: Asks<'_>| {
        let card = user_question("q", &databases(), asks, &Glyphs::UNICODE);
        let Row::Text(pieces) = &card.rows[0] else { panic!("an option is a text row") };
        pieces[0].clone()
    };
    assert_eq!(digit(Asks::Options), ("1".to_owned(), Tone::AttentionBold));
    assert_eq!(digit(Asks::Typing("")), ("1".to_owned(), Tone::Dim));
    assert_eq!(digit(Asks::Elsewhere), ("1".to_owned(), Tone::Dim));
    let card = user_question("q", &databases(), Asks::Options, &Glyphs::UNICODE);
    let Row::Text(pieces) = &card.rows[1] else { panic!("an option is a text row") };
    assert_eq!(pieces[2], ("SQLite".to_owned(), Tone::Text));
    assert_eq!(pieces[4].1, Tone::Dim, "the description is muted");
}

#[test]
fn without_colour_the_keys_of_the_question_stay_bold_is_questions_0012_frame_7() {
    let shown = question("q", &databases()[..2], Asks::Options, &plain(100));
    insta::assert_snapshot!(readable(&shown));
}

#[test]
fn the_text_of_the_model_goes_through_one_line() {
    let options = [option("a\nb\u{1b}[2J", Some("c\u{202e}d")), option("e", None)];
    let card = user_question("why\u{1b}]52;c;x\u{7}?", &options, Asks::Options, &Glyphs::UNICODE);
    let piped = RenderOptions::new(100).with_terminal(false);
    let shown = card.render(Some(&user_question_footer(Asks::Options)), &piped);
    assert!(!shown.contains('\u{1b}') && !shown.contains('\u{202e}'), "{shown:?}");
    // An option with no description has no gap after its label.
    assert!(shown.contains("    2 e\n"), "{shown:?}");
}

#[test]
fn the_end_of_the_question_says_how_it_ended_is_questions_0012_frame_5() {
    let labels: Vec<String> = databases().into_iter().map(|option| option.label).collect();
    let replied = |reply| UserAnswer::Replied { reply };
    let cases = [
        (
            replied(QuestionReply::Option { index: 0 }),
            None,
            QuestionEnd::Answered,
            "answered · Postgres 16",
        ),
        (
            replied(QuestionReply::Text { text: "port\n5433".to_owned() }),
            Some(Origin::Shell),
            QuestionEnd::Answered,
            "answered · \"port 5433\"",
        ),
        (
            replied(QuestionReply::Option { index: 1 }),
            Some(Origin::Phone),
            QuestionEnd::Answered,
            "answered on the phone · SQLite",
        ),
        (
            replied(QuestionReply::Skipped),
            Some(Origin::Shell),
            QuestionEnd::Skipped,
            "skipped · the model goes on with a choice that is safe and easy to undo",
        ),
        (
            UserAnswer::Unanswered { away: false },
            None,
            QuestionEnd::Unanswered,
            "nobody answered · the model goes on with a choice that is safe and easy to undo",
        ),
        (
            UserAnswer::Unanswered { away: true },
            None,
            QuestionEnd::Unanswered,
            "nobody answered while you were away · the model goes on with a safe choice",
        ),
        (
            UserAnswer::Interrupted,
            None,
            QuestionEnd::Interrupted,
            "not answered · you interrupted the turn",
        ),
        // An index that the labels do not have names its number.
        (
            replied(QuestionReply::Option { index: 7 }),
            None,
            QuestionEnd::Answered,
            "answered · option 8",
        ),
    ];
    for (answer, origin, end, words) in cases {
        assert_eq!(question_end(&answer, origin, &labels, "\u{b7}"), (end, words.to_owned()));
    }
    let glyphs = Glyphs::UNICODE;
    assert_eq!(QuestionEnd::Answered.mark(&glyphs), ("\u{2713}", Tone::Success));
    assert_eq!(QuestionEnd::Skipped.mark(&glyphs), ("\u{b7}", Tone::Dim));
    assert_eq!(QuestionEnd::Unanswered.mark(&glyphs), ("\u{2717}", Tone::Failure));
    assert_eq!(QuestionEnd::Interrupted.mark(&glyphs), ("\u{25a0}", Tone::Attention));
}

mod design;

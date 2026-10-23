use std::time::{Duration, Instant};

use efr_protocol::{Seq, TurnId};
use efr_render::RenderOptions;
use efr_test_support::bench::Report;
use pretty_assertions::assert_eq;

use super::{BACKGROUND_HINT, HINT, Input, MAX_ROWS, first_line, user_message};
use crate::keys::Key;
use crate::live::Cursor;
use crate::testing::readable;

fn plain() -> RenderOptions {
    RenderOptions::new(60).with_terminal(false)
}

fn turn(n: u8) -> TurnId {
    format!("019a9b1c-3d00-7a10-8b20-0000000001{n:02x}").parse().unwrap()
}

fn typed(text: &str) -> Input {
    let mut input = Input::default();
    for byte in text.bytes() {
        input.line.key(Key::Byte(byte));
    }
    input
}

#[test]
fn the_hint_names_ctrl_b_only_while_a_call_can_move_and_the_row_is_empty() {
    let mut input = Input { background: true, ..Input::default() };
    let tail = input.row(&plain(), 80);
    assert_eq!(tail.text, format!("\u{203a}  {BACKGROUND_HINT}{HINT}\n"));
    assert!(tail.text.starts_with("\u{203a}  ctrl+b background \u{b7} enter"), "{}", tail.text);
    input.background = false;
    assert_eq!(input.row(&plain(), 60).text, format!("\u{203a}  {HINT}\n"));
    // With text in the row, no hint shows, but Ctrl+B still moves the call.
    let mut typed = typed("a");
    typed.background = true;
    assert_eq!(typed.row(&plain(), 60).text, "\u{203a} a\n");
}

#[test]
fn an_empty_row_shows_the_hint_and_waits_after_the_mark() {
    let tail = Input::default().row(&plain(), 60);
    // The cursor waits on a blank cell and the hint starts after it, so a block cursor
    // covers no letter of the hint. Typed text starts at the cursor.
    assert_eq!(tail.text, format!("\u{203a}  {HINT}\n"));
    assert_eq!(tail.cursor, Some(Cursor { line: 0, column: 2 }));
    assert_eq!(tail.text.chars().nth(2), Some(' '), "the cell under the cursor is blank");
    assert_eq!(typed("a").row(&plain(), 60).text, "\u{203a} a\n");
    // In colour, the mark is in the accent role and the hint muted.
    let painted = Input::default().row(&RenderOptions::new(60), 60);
    assert_eq!(readable(&painted.text), format!("\\e[33m\u{203a}\\e[0m  \\e[2m{HINT}\\e[0m\n"));
}

#[test]
fn the_hint_is_cut_to_a_narrow_screen() {
    let tail = Input::default().row(&plain(), 20);
    assert_eq!(tail.text, "\u{203a}  enter steer \u{b7} t\u{2026}\n");
}

#[test]
fn a_long_text_goes_on_in_the_next_rows_under_the_mark() {
    let tail = typed("a prompt that is too long for one row of this screen").row(&plain(), 20);
    assert_eq!(
        tail.text,
        "\u{203a} a prompt that is \n  too long for one \n  row of this scree\n  n\n"
    );
    assert_eq!(tail.cursor, Some(Cursor { line: 3, column: 3 }));
}

#[test]
fn at_most_five_rows_show_and_the_cursor_row_is_one_of_them() {
    let text = (1..=8).map(|n| format!("line {n}")).collect::<Vec<_>>().join("\n");
    let mut input = typed(&text);
    let tail = input.row(&plain(), 40);
    assert_eq!(tail.text.lines().count(), MAX_ROWS);
    assert_eq!(tail.text.lines().next(), Some("  line 4"), "the rows up to the cursor show");
    assert_eq!(tail.cursor, Some(Cursor { line: 4, column: 8 }));
    // To the start of the first line: Ctrl+A, then Left over the newline before it.
    for _ in 0..7 {
        input.line.key(Key::Byte(0x01));
        for byte in b"\x1b[D" {
            input.line.key(Key::Byte(*byte));
        }
    }
    input.line.key(Key::Byte(0x01));
    let tail = input.row(&plain(), 40);
    assert_eq!(tail.text.lines().next(), Some("\u{203a} line 1"));
    assert_eq!(tail.cursor, Some(Cursor { line: 0, column: 2 }));
}

#[test]
fn unread_steers_and_queued_prompts_wait_above_the_status_row() {
    let mut input = Input::default();
    assert_eq!(input.pending(&plain(), 40), "");
    input.steered(Seq::new(20), "use the release build\nand log it".to_owned());
    input.queued(turn(1), "then update the changelog".to_owned(), false);
    input.queued(turn(2), "and the docs".to_owned(), true);
    input.queued(turn(2), "the same turn twice".to_owned(), false);
    assert_eq!(
        input.pending(&plain(), 80),
        "\u{21b3} steer: use the release build \u{2026}\n\
         \u{21b3} queued: then update the changelog\n\
         \u{21b3} queued: and the docs (too late to steer, so it waits in the queue)\n"
    );
    assert_eq!(input.queued_turns(), [turn(1), turn(2)]);
    assert_eq!(input.newest(), Some(turn(2)));
    // Cut to the width.
    let narrow = input.pending(&plain(), 20);
    assert!(narrow.lines().all(|line| line.chars().count() <= 20), "{narrow}");
}

#[test]
fn a_delivered_steer_leaves_the_list_and_a_resent_one_runs_first() {
    let mut input = Input::default();
    input.steered(Seq::new(20), "one".to_owned());
    input.steered(Seq::new(22), "two".to_owned());
    input.steered(Seq::new(24), "three".to_owned());
    input.queued(turn(1), "queued".to_owned(), false);
    assert_eq!(input.delivered(&[Seq::new(22), Seq::new(99)]), ["two"]);
    assert!(input.is_steer(Seq::new(20)) && !input.is_steer(Seq::new(22)));
    assert!(input.resent(turn(2), &[Seq::new(20), Seq::new(24)]));
    assert!(!input.resent(turn(3), &[Seq::new(20)]), "nothing left to resend");
    assert_eq!(input.unread(), []);
    assert_eq!(input.queued_turns(), [turn(2), turn(1)]);
    let next = input.next().unwrap();
    assert_eq!((next.turn, next.text.as_str()), (turn(2), "one\nthree"));
}

#[test]
fn the_followed_prompt_shows_as_queued_until_it_starts() {
    let input = Input { prompt: Some("from the row".to_owned()), ..Input::default() };
    assert_eq!(input.pending(&plain(), 40), "\u{21b3} queued: from the row\n");
}

#[test]
fn the_first_line_is_safe_to_print() {
    assert_eq!(first_line("\n  first \x1b[2J\nsecond"), "first \u{241b}[2J \u{2026}");
    assert_eq!(first_line(""), "");
}

#[test]
fn a_steer_reads_back_as_the_users_message() {
    assert_eq!(user_message("check the logs\nfirst\n", &plain()), "> check the logs\n> first\n");
    let bold = user_message("x", &RenderOptions::new(40));
    assert_eq!(readable(&bold), "\\e[1m> x\\e[0m\n");
}

#[test]
#[ignore = "bench: run by just bench-perf"]
fn bench_perf_input_row() {
    const DRAWS: usize = 600;
    let limit = Duration::from_secs(10);
    let options = RenderOptions::new(80);
    let mut report = Report::new("bench_perf_input_row");
    let mut points = Vec::new();
    for kb in [1_u64, 20, 256] {
        // A paste of one long line, the cursor at its end.
        let unit = "cargo test -p efr-cli -- --nocapture ";
        let text =
            unit.repeat(kb as usize * 1024 / unit.len() + 1)[..kb as usize * 1024].to_owned();
        let mut input = Input { shown: true, ..Input::default() };
        input.line.append(&text);
        assert_eq!(input.line.text().len(), text.len());
        let mut draws = Vec::with_capacity(DRAWS);
        let start = Instant::now();
        for _ in 0..DRAWS {
            let draw = Instant::now();
            let tail = input.row(&options, 80);
            draws.push(draw.elapsed());
            assert_eq!(tail.text.lines().count(), MAX_ROWS);
            if start.elapsed() > limit {
                break;
            }
        }
        let total = start.elapsed();
        report.time(&format!("{kb} KB: per draw"), &draws);
        report.time(&format!("{kb} KB: {} draws total", draws.len()), &[total]);
        if draws.len() < DRAWS {
            report.count("stopped_at_kb", kb);
            break;
        }
        points.push((kb, total));
    }
    report.slope("total slope over bytes", &points);
    report.finish();
}

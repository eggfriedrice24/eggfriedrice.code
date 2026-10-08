//! The input row on a simulated screen: where it stands in the live zone, where the
//! cursor waits, what waits for the turn above the status row, the steer that a model
//! call read, and the prompt that the view follows after the turn.

use efr_protocol::{Event, EventEnvelope, Scope, Seq, TurnId, TurnInterruptResult};
use efr_render::{ColourMode, RenderOptions};
use pretty_assertions::assert_eq;

use super::super::{Look, TurnEnd, TurnView};
use super::at;
use crate::keys::Key;
use crate::row::Action;
use crate::terminal::Size;
use crate::testing::{Grid, turn};

const SIZE: Size = Size { cols: 50, rows: 20 };

/// No motion and no progress bar, so the screen is the same at every frame.
const LOOK: Look = Look { motion: false, summary: true, progress: false, diff_lines: 20 };

fn turn_2() -> TurnId {
    "019a9b1c-3d00-7a10-8b20-000000000012".parse().unwrap()
}

/// A view with the input row that shows, on a screen of [`SIZE`].
fn row_view() -> (TurnView, Grid) {
    let options = RenderOptions::new(SIZE.cols).with_colour(ColourMode::None);
    let mut view = TurnView::new(turn(), options).with_look(LOOK).with_input();
    view.start();
    view.show_row(true);
    (view, Grid::new(SIZE.cols))
}

/// Types `text` into the row of `view`.
fn type_in(view: &mut TurnView, text: &str) {
    for byte in text.bytes() {
        let action = view.row_key(Key::Byte(byte));
        assert!(matches!(action, Action::Edited | Action::None), "{action:?}");
    }
}

/// The screen and the cursor, with the cursor drawn as `▏` in its cell.
fn screen(view: &mut TurnView, grid: &mut Grid, millis: i64) -> String {
    grid.write(&view.frame(SIZE, at(millis)));
    let (row, col) = grid.cursor();
    let mut lines = grid.lines();
    if lines.len() <= row {
        lines.resize(row + 1, String::new());
    }
    let line = &mut lines[row];
    let mut chars: Vec<char> = line.chars().collect();
    if chars.len() < col {
        chars.resize(col, ' ');
    }
    chars.insert(col, '\u{258f}');
    *line = chars.into_iter().collect();
    lines.join("\n")
}

fn event(view: &mut TurnView, seq: u64, millis: i64, event: Event) {
    let envelope =
        EventEnvelope { seq: Seq::new(seq), conversation_id: None, at: at(millis), event };
    view.envelope(&envelope, SIZE, true);
}

#[test]
fn the_row_waits_below_the_status_row_with_what_waits_for_the_turn() {
    let (mut view, mut grid) = row_view();
    let mut shots = vec![format!("[empty]\n{}", screen(&mut view, &mut grid, 0))];
    type_in(&mut view, "check the logs\nand then the config");
    shots.push(format!("[typed]\n{}", screen(&mut view, &mut grid, 100)));
    view.steered(Seq::new(11), "use the release build".to_owned());
    view.queued_prompt(turn_2(), "then update the docs".to_owned(), false);
    type_in(&mut view, "\x15");
    shots.push(format!("[pending]\n{}", screen(&mut view, &mut grid, 200)));
    let reading = Event::SteeringDelivered { turn_id: turn(), steers: vec![Seq::new(11)] };
    event(&mut view, 12, 300, reading);
    let message = Event::AssistantMessageCompleted {
        turn_id: turn(),
        index: 0,
        text: "The release build passes.".to_owned(),
    };
    event(&mut view, 13, 400, message);
    shots.push(format!("[read]\n{}", screen(&mut view, &mut grid, 400)));
    insta::assert_snapshot!(shots.join("\n\n"));
}

#[test]
fn a_question_hides_the_row_and_the_cursor_goes_below_it() {
    let (mut view, mut grid) = row_view();
    type_in(&mut view, "draft");
    screen(&mut view, &mut grid, 0);
    view.show_row(false);
    let shown = screen(&mut view, &mut grid, 100);
    assert!(!shown.contains('\u{203a}'), "{shown}");
    view.show_row(true);
    let shown = screen(&mut view, &mut grid, 200);
    assert!(shown.ends_with("\u{203a} draft\u{258f}"), "{shown}");
}

#[test]
fn the_view_follows_a_prompt_that_it_queued_after_the_turn() {
    let (mut view, mut grid) = row_view();
    view.queued_prompt(turn_2(), "then update the docs".to_owned(), false);
    event(
        &mut view,
        11,
        0,
        Event::TurnStarted {
            turn_id: turn(),
            cwd: "/home/user/project".into(),
            scope: Scope::Machine,
            settings: None,
        },
    );
    let done = Event::TurnCompleted { turn_id: turn(), usage: None, changes: None };
    let envelope =
        EventEnvelope { seq: Seq::new(12), conversation_id: None, at: at(2000), event: done };
    let step = view.envelope(&envelope, SIZE, true);
    assert_eq!(step.end, None, "the command goes on");
    assert_eq!(view.turn(), turn_2());
    assert!(view.is_queued());
    let waiting = screen(&mut view, &mut grid, 2000);
    event(
        &mut view,
        13,
        2100,
        Event::TurnStarted {
            turn_id: turn_2(),
            cwd: "/home/user/project".into(),
            scope: Scope::Machine,
            settings: None,
        },
    );
    let running = screen(&mut view, &mut grid, 2100);
    let ended = Event::TurnCompleted { turn_id: turn_2(), usage: None, changes: None };
    let envelope =
        EventEnvelope { seq: Seq::new(14), conversation_id: None, at: at(3000), event: ended };
    let step = view.envelope(&envelope, SIZE, true);
    assert_eq!(step.end, Some(TurnEnd::Completed));
    let last = screen(&mut view, &mut grid, 3000);
    insta::assert_snapshot!(format!(
        "[waiting]\n{waiting}\n\n[running]\n{running}\n\n[end]\n{last}"
    ));
}

#[test]
fn esc_puts_the_prompts_it_took_back_into_the_row_after_the_draft() {
    let (mut view, mut grid) = row_view();
    view.queued_prompt(turn_2(), "then update the docs".to_owned(), false);
    type_in(&mut view, "draft");
    let result = TurnInterruptResult {
        turn_id: turn(),
        seq: Seq::new(12),
        resent: None,
        withdrawn: vec![efr_protocol::WithdrawnPrompt {
            turn_id: turn_2(),
            seq: Seq::new(13),
            text: "then update the docs".to_owned(),
        }],
        withdrawn_steers: Vec::new(),
    };
    view.interrupt_result(&result, SIZE);
    assert_eq!(view.row_text(), "draft\nthen update the docs");
    assert_eq!(view.queued_turns(), []);
    let shown = screen(&mut view, &mut grid, 0);
    assert!(!shown.contains('\u{21b3}'), "nothing waits now: {shown}");
}

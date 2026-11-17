//! The model's question (`ask_user`, TM-Questions 0.0.12): its card in place of the
//! row of its call, the answer line in the card, and the one line that takes the
//! card's place at its end.

use efr_protocol::{
    Event, Origin, QuestionId, QuestionOption, QuestionReply, UserAnswer, WAKE_QUESTION,
};
use efr_render::{ColourMode, RenderOptions};
use pretty_assertions::assert_eq;
use serde_json::json;

use super::super::{Ask, Step, TurnView};
use super::{feed, raw_view, turn_completed};
use crate::terminal::Size;
use crate::testing::{bare, call, now, turn};

const WIDE: Size = Size { cols: 100, rows: 30 };

fn question_id() -> QuestionId {
    "019a9b1c-3d00-7a10-8b20-0000000000e1".parse().unwrap()
}

/// A terminal view of 100 columns without colour.
fn terminal() -> TurnView {
    TurnView::new(turn(), RenderOptions::new(WIDE.cols).with_colour(ColourMode::None))
}

/// `step` with the frame that the follow loop writes after it.
fn framed(mut step: Step, view: &mut TurnView) -> Step {
    step.out.push_str(&view.frame(WIDE, now()));
    step
}

fn ask_started() -> Event {
    Event::ToolCallStarted {
        turn_id: turn(),
        call_id: call(),
        tool: "ask_user".to_owned(),
        input: json!({ "question": "Which database?", "options": [] }),
        manual_input: false,
        launch: None,
        freeform: false,
    }
}

fn ask_completed() -> Event {
    Event::ToolCallCompleted {
        turn_id: turn(),
        call_id: call(),
        output: "The user chose option 1: \"Postgres 16\".".to_owned(),
        truncated: false,
        is_error: false,
        exit_code: None,
        sandbox: None,
        refusal: None,
        changes: None,
        diff: None,
        left_running: false,
    }
}

fn requested(question: &str) -> Event {
    let option = |label: &str, description: &str| QuestionOption {
        label: label.to_owned(),
        description: Some(description.to_owned()),
    };
    Event::UserQuestionRequested {
        turn_id: turn(),
        call_id: call(),
        question_id: question_id(),
        question: question.to_owned(),
        options: vec![
            option("Postgres 16", "the same as production; I recommend it"),
            option("SQLite", "fast and needs no server, but it has no JSONB"),
        ],
    }
}

fn answered(answer: UserAnswer, origin: Option<Origin>) -> Event {
    Event::UserQuestionAnswered { turn_id: turn(), question_id: question_id(), answer, origin }
}

#[test]
fn the_card_is_the_call_and_a_digit_answers_it_in_place() {
    let mut view = terminal();
    let started = framed(view.event(&ask_started(), WIDE, true), &mut view);
    assert_eq!(bare(&started.out), "", "the call of the question has no row");
    let step = framed(
        view.event(&requested("Which database should the tests use?"), WIDE, true),
        &mut view,
    );
    assert_eq!(step.ask, Some(Ask::Question { question_id: question_id(), options: 2 }));
    assert!(view.waits_for_user());
    let card = bare(&step.out);
    assert!(card.contains(" ?  Which database should the tests use?\n"), "{card}");
    assert!(card.contains("    1 Postgres 16  the same as production; I recommend it\n"), "{card}");
    assert!(card.contains("    t type an answer   esc skip\n"), "{card}");

    let reply = QuestionReply::Option { index: 0 };
    let line = framed(view.user_replied(question_id(), &reply, WIDE), &mut view);
    assert!(
        bare(&line.out).contains("\u{2713} answered \u{b7} Postgres 16\n"),
        "{}",
        bare(&line.out)
    );
    assert!(!view.waits_for_user());
    // The daemon's event for this client's own answer and the end of the call add nothing.
    let event = answered(UserAnswer::Replied { reply }, Some(Origin::Shell));
    assert_eq!(framed(view.event(&event, WIDE, true), &mut view), Step::default());
    let done = framed(view.event(&ask_completed(), WIDE, true), &mut view);
    assert!(!bare(&done.out).contains("ask_user"), "{}", bare(&done.out));
}

#[test]
fn the_answer_line_opens_in_the_card_and_closes_again() {
    let mut view = terminal();
    framed(view.event(&requested("Which database?"), WIDE, true), &mut view);
    let open = framed(view.user_typing(question_id(), Some("port 5433"), WIDE), &mut view);
    let shown = bare(&open.out);
    assert!(shown.contains("    \u{203a} port 5433\n"), "{shown}");
    assert!(shown.contains("    Do not type a password or a token here.\n"), "{shown}");
    assert!(shown.contains("    enter send   esc back to the options\n"), "{shown}");
    let back = framed(view.user_typing(question_id(), None, WIDE), &mut view);
    let shown = bare(&back.out);
    assert!(!shown.contains("port 5433"), "{shown}");
    assert!(shown.contains("    t type an answer   esc skip\n"), "{shown}");
}

#[test]
fn a_typed_answer_is_cut_to_one_row() {
    let mut view = terminal();
    framed(view.event(&requested("Which database?"), WIDE, true), &mut view);
    let text = format!("Postgres 16, but on port 5433, {}", "and more words ".repeat(10));
    let reply = QuestionReply::Text { text };
    let line = framed(view.user_replied(question_id(), &reply, WIDE), &mut view);
    let shown = bare(&line.out);
    let start = shown.find('\u{2713}').unwrap();
    let row = shown[start..].lines().next().unwrap();
    assert!(row.starts_with("\u{2713} answered \u{b7} \"Postgres 16, but on port 5433"), "{row}");
    assert!(row.ends_with('\u{2026}'), "{row}");
    assert!(row.chars().count() <= usize::from(WIDE.cols), "{row}");
}

#[test]
fn an_answer_from_the_phone_or_no_answer_settles_the_keys_with_its_line() {
    let cases = [
        (
            UserAnswer::Replied { reply: QuestionReply::Option { index: 1 } },
            Some(Origin::Phone),
            "\u{2713} answered on the phone \u{b7} SQLite\n",
        ),
        (
            UserAnswer::Replied { reply: QuestionReply::Skipped },
            Some(Origin::Shell),
            "\u{b7} skipped \u{b7} the model goes on with a choice that is safe and easy to undo\n",
        ),
        (
            UserAnswer::Unanswered { away: true },
            None,
            "\u{2717} nobody answered while you were away \u{b7} the model goes on with a safe \
             choice\n",
        ),
        (UserAnswer::Interrupted, None, "\u{25a0} not answered \u{b7} you interrupted the turn\n"),
    ];
    for (answer, origin, line) in cases {
        let mut view = raw_view();
        let (_, err, _) = feed(&mut view, &[requested("Which database?")], true);
        assert!(err.ends_with("    t type an answer   esc skip\n"), "{err}");
        let step = view.event(&answered(answer, origin), WIDE, true);
        assert!(step.settled);
        assert_eq!(step.err, line);
    }
}

#[test]
fn without_keys_another_client_answers_and_nothing_settles() {
    let mut view = raw_view();
    let step = view.event(&requested("Which database?"), WIDE, false);
    assert_eq!(step.ask, None);
    assert!(step.err.ends_with("    waiting for another client to answer\n"), "{}", step.err);
    assert!(!view.waits_for_user());
    let event = answered(
        UserAnswer::Replied { reply: QuestionReply::Option { index: 0 } },
        Some(Origin::Phone),
    );
    let step = view.event(&event, WIDE, false);
    assert!(!step.settled);
    assert_eq!(step.err, "\u{2713} answered on the phone \u{b7} Postgres 16\n");
    assert!(!view.event(&turn_completed(), WIDE, false).settled);
}

#[test]
fn the_end_of_the_turn_stops_the_keys_of_the_question() {
    let mut view = raw_view();
    view.event(&requested("Which database?"), WIDE, true);
    assert!(view.event(&turn_completed(), WIDE, true).settled);
}

#[test]
fn a_question_in_a_job_notice_turn_says_so_after_its_title() {
    let mut view = raw_view();
    let question = format!("{WAKE_QUESTION}Which target should I fix first?");
    let step = view.event(&requested(&question), WIDE, true);
    assert!(
        step.err.starts_with(" ?  Which target should I fix first? (job notice turn)\n"),
        "{}",
        step.err
    );
}

#[test]
fn piped_the_answer_line_echoes_after_its_rows() {
    let mut view = raw_view();
    view.event(&requested("Which database?"), WIDE, true);
    let open = view.user_typing(question_id(), Some(""), WIDE);
    assert_eq!(
        open.err,
        "    Do not type a password or a token here.\n    enter send   esc back to the options\n\
         \u{203a} "
    );
    assert_eq!(view.user_typing(question_id(), Some("pg"), WIDE).err, "pg");
    let back = view.user_typing(question_id(), None, WIDE);
    assert_eq!(back.err, "\n    t type an answer   esc skip\n");
}

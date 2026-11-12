//! Always-allow rules in the view (phase E): the `a` key on an exit question that offers
//! a rule, its confirm, the line of what an `a` answer saved, and the line of a call
//! that a rule covered.

use std::cell::Cell;
use std::path::PathBuf;

use efr_protocol::{
    AlwaysSaved, ApprovalDecision, Event, ExitFacts, ExitInfo, ExitKind, Grant, JudgeKind, Launch,
    NotSavedReason, Origin, Verdict,
};
use pretty_assertions::assert_eq;

use super::super::{Ask, TurnView};
use super::{
    WIDE, contained_completed, contained_started, exit_events, feed, framed, sandbox_view,
};
use crate::format::card::RENDERS;
use crate::terminal::Size;
use crate::testing::{call, exit_info, now, readable, turn};

/// The rule that the test's questions offer.
const RULE: &str = "write under ~/notes";

/// A write exit to `~/notes/x` whose question offers `always`.
fn write_exit(always: Option<&str>) -> ExitInfo {
    let grants = vec![Grant::Write { path: PathBuf::from("/home/user/notes") }];
    ExitInfo {
        always: always.map(str::to_owned),
        ..exit_info(&[ExitKind::Write], Launch::Contained { grants })
    }
}

fn write_events(always: Option<&str>) -> [Event; 2] {
    exit_events("echo hi > ~/notes/x", ExitFacts::default(), write_exit(always))
}

fn saved(outcome: AlwaysSaved) -> Event {
    Event::AlwaysRuleSaved { turn_id: turn(), call_id: call(), outcome }
}

fn resolved(decision: ApprovalDecision) -> Event {
    Event::ApprovalResolved { turn_id: turn(), call_id: call(), decision, origin: Origin::Cli }
}

fn covered() -> Event {
    Event::ExitJudged {
        turn_id: turn(),
        call_id: call(),
        judge: JudgeKind::Always,
        verdict: Verdict::Allow,
        model: None,
        latency_ms: None,
        risk: None,
        user_authorization: None,
        category: Some("sandbox.allow[2]".to_owned()),
        rationale: Some(RULE.to_owned()),
        record_sha256: None,
        cached: false,
        chain_step: None,
        skipped: Vec::new(),
        reasked: false,
    }
}

#[test]
fn card_offers_always_only_when_set() {
    let mut view = sandbox_view(false);
    let (_, err, _) = feed(&mut view, &write_events(Some(RULE)), true);
    insta::assert_snapshot!(err);
    assert!(view.offers_always(call()));
    // An older daemon fills no rule: the card keeps its two keys.
    let mut view = sandbox_view(false);
    let (_, err, _) = feed(&mut view, &write_events(None), true);
    assert!(err.ends_with("    y allow   n deny\n"), "{err}");
    assert!(!view.offers_always(call()));
    // A question that another client must answer offers nothing here.
    let mut view = sandbox_view(false);
    let (_, err, _) = feed(&mut view, &write_events(Some(RULE)), false);
    assert!(err.ends_with("    waiting for another client to answer\n"), "{err}");
    assert!(!view.offers_always(call()));
}

#[test]
fn the_rule_from_the_daemon_cannot_drive_the_terminal() {
    let mut view = sandbox_view(false);
    feed(&mut view, &write_events(Some("write under ~/a\x1b[2J\nb")), true);
    // The rule shows in the confirm after `a` (TM-Questions 4).
    let step = view.confirm_always(call(), WIDE);
    assert!(
        step.err.starts_with("    always saves the rule  write under ~/a\u{241b}[2J b\n"),
        "{:?}",
        step.err
    );
}

#[test]
fn the_confirm_takes_the_place_of_the_keys_until_the_next_key() {
    let mut view = sandbox_view(true);
    let [record, approval] = write_events(Some(RULE));
    framed(view.event(&record, WIDE, true), &mut view);
    let asked = framed(view.event(&approval, WIDE, true), &mut view);
    assert_eq!(asked.ask, Some(Ask::Approval(call())));
    let mut frames = vec![readable(&asked.out)];
    frames.push(readable(&framed(view.confirm_always(call(), WIDE), &mut view).out));
    frames.push(readable(&framed(view.always_cancelled(call(), WIDE), &mut view).out));
    frames.push(readable(&framed(view.confirm_always(call(), WIDE), &mut view).out));
    let answered = view.answered(call(), ApprovalDecision::AllowAlways, WIDE);
    frames.push(readable(&framed(answered, &mut view).out));
    insta::assert_snapshot!(frames.join("\n---\n"));
}

#[test]
fn a_card_renders_again_only_when_its_footer_or_the_width_changes() {
    let renders = || RENDERS.with(Cell::get);
    let mut view = sandbox_view(true);
    let [record, approval] = write_events(Some(RULE));
    view.event(&record, WIDE, true);
    view.event(&approval, WIDE, true);
    view.frame(WIDE, now());
    let start = renders();
    view.tick(WIDE, now());
    view.tick(WIDE, now());
    assert_eq!(renders(), start, "ticks with no change");
    view.confirm_always(call(), WIDE);
    let confirm = view.frame(WIDE, now());
    assert!(confirm.contains("always saves the rule"), "{}", readable(&confirm));
    assert_eq!(renders(), start + 1, "a frame with a new footer");
    view.tick(Size { cols: 80, ..WIDE }, now());
    assert_eq!(renders(), start + 2, "a tick at a new width");
}

#[test]
fn a_refused_always_answer_asks_again_without_the_a() {
    let mut view = sandbox_view(false);
    feed(&mut view, &write_events(Some(RULE)), true);
    let step = view.confirm_always(call(), WIDE);
    assert_eq!(
        step.err,
        "    always saves the rule  write under ~/notes\n    y save the rule and allow   n back\n"
    );
    let step = view.always_refused(call(), "the question of the call offers no always rule", WIDE);
    assert_eq!(
        step.err,
        "\nthe answer was not taken: the question of the call offers no always rule\n    \
             y allow   n deny\n"
    );
    assert!(!view.offers_always(call()));
    // The question is open still: `y` answers it.
    let step = view.answered(call(), ApprovalDecision::Allow, WIDE);
    assert!(step.err.contains("\u{2713} allowed \u{b7} write ~/notes\n"), "{:?}", step.err);
}

#[test]
fn always_saved_line() {
    let mut view = sandbox_view(false);
    feed(&mut view, &[contained_started("echo hi > ~/notes/x")], true);
    feed(&mut view, &write_events(Some(RULE)), true);
    let answered = view.answered(call(), ApprovalDecision::AllowAlways, WIDE);
    let (_, err, _) = feed(
        &mut view,
        &[
            resolved(ApprovalDecision::AllowAlways),
            saved(AlwaysSaved::Saved { index: 2, rule: RULE.to_owned() }),
            contained_completed(0, None),
        ],
        true,
    );
    insta::assert_snapshot!(format!("{}{err}", answered.err));
}

#[test]
fn always_not_saved_line() {
    let mut view = sandbox_view(false);
    feed(&mut view, &[contained_started("echo hi > ~/notes/x")], true);
    feed(&mut view, &write_events(Some(RULE)), true);
    view.answered(call(), ApprovalDecision::AllowAlways, WIDE);
    let (_, err, _) = feed(
        &mut view,
        &[
            resolved(ApprovalDecision::AllowAlways),
            saved(AlwaysSaved::NotSaved { reason: NotSavedReason::ReadOnly }),
        ],
        true,
    );
    assert!(
        err.contains("rule not saved: config.toml is read-only; the command ran once\n"),
        "{err}"
    );
}

#[test]
fn an_always_answer_from_another_client_shows_as_one() {
    let mut view = sandbox_view(false);
    feed(&mut view, &write_events(Some(RULE)), false);
    let (_, err, _) = feed(&mut view, &[resolved(ApprovalDecision::AllowAlways)], false);
    assert!(err.contains("\u{2713} allowed always \u{b7} from efr\n"), "{err}");
}

#[test]
fn covered_call_line() {
    let mut view = sandbox_view(false);
    let record = write_events(None)[0].clone();
    let (_, err, _) = feed(
        &mut view,
        &[
            contained_started("echo hi > ~/notes/x"),
            record,
            covered(),
            contained_completed(0, None),
        ],
        true,
    );
    insta::assert_snapshot!(err);
    // On a terminal the line is dim, and the call runs with no question.
    let mut view = sandbox_view(true);
    let record = write_events(None)[0].clone();
    feed(&mut view, &[contained_started("echo hi > ~/notes/x"), record], true);
    let step = framed(view.event(&covered(), WIDE, true), &mut view);
    assert_eq!(step.ask, None);
    let out = readable(&step.out);
    assert!(out.contains("allowed by your rule sandbox.allow[2]: write under ~/notes"), "{out}");
}

#[test]
fn a_view_without_a_question_offers_no_always() {
    let mut view = TurnView::new(turn(), efr_render::RenderOptions::new(40));
    assert!(!view.offers_always(call()));
    assert_eq!(view.confirm_always(call(), WIDE), super::super::Step::default());
}

mod design {
    //! The line that takes the place of a question against the frames of the design
    //! (TM-Questions 7, 8, TM-Legend 26, TM-Sequence 10), and the words of each of its
    //! forms from the events that settle a question.

    use efr_protocol::{AlwaysSaved, ApprovalDecision, Event, Origin};
    use efr_test_support::design::{Depth, Frame, assert_rows};

    use super::super::super::asking::answer_text;
    use super::super::{WIDE, framed, sandbox_view, sent};
    use super::{RULE, saved, write_events};
    use crate::testing::{bare, call, design_options, turn};

    /// The forms of TM-Questions 7, each with whether it is good.
    const FORMS: [(bool, &str); 6] = [
        (true, "allowed \u{b7} outside the sandbox"),
        (true, "allowed \u{b7} from the phone"),
        (
            true,
            "allowed always \u{b7} saved as sandbox.allow[2]: network index.crates.io for cargo",
        ),
        (false, "denied \u{b7} network (index.crates.io)"),
        (false, "the approval expired \u{b7} nobody answered in 10 min; the call did not run"),
        (false, "expired while you were away \u{b7} the call did not run"),
    ];

    #[test]
    fn the_mark_is_in_its_role_and_the_rest_muted_questions_7_8_legend_26_sequence_10() {
        let frame = Frame::load("Questions", 7).unwrap();
        for depth in Depth::ALL {
            let options = design_options(depth, frame.cols);
            let lines: String =
                FORMS.iter().map(|(good, line)| answer_text(*good, line, &options)).collect();
            assert_rows(&frame, .., &lines, depth);
            let network = answer_text(true, "allowed \u{b7} network (github.com)", &options);
            assert_rows(&Frame::load("Questions", 8).unwrap(), 2..3, &network, depth);
            assert_rows(&Frame::load("Legend", 26).unwrap(), .., &network, depth);
            assert_rows(&Frame::load("Sequence", 10).unwrap(), 4..5, &network, depth);
        }
    }

    /// The scrollback that a terminal view writes for `events` after the question of
    /// `write_events`, as plain text.
    fn settled(answer: impl FnOnce(&mut super::super::super::TurnView) -> String) -> String {
        let mut view = sandbox_view(true);
        for (seq, event) in write_events(Some(RULE)).into_iter().enumerate() {
            framed(view.envelope(&sent(11 + seq as u64, 0, event), WIDE, true), &mut view);
        }
        view.frame(WIDE, super::now());
        bare(&answer(&mut view))
    }

    #[test]
    fn each_form_names_what_was_asked() {
        // A key here: the `why` of the card follows the answer.
        let shown =
            settled(|view| framed(view.answered(call(), ApprovalDecision::Deny, WIDE), view).out);
        assert!(shown.contains("\u{2717} denied \u{b7} write ~/notes\n"), "{shown}");
        // An answer from elsewhere names where it came from.
        let shown = settled(|view| {
            let resolved = Event::ApprovalResolved {
                turn_id: turn(),
                call_id: call(),
                decision: ApprovalDecision::Allow,
                origin: Origin::Phone,
            };
            framed(view.envelope(&sent(13, 0, resolved), WIDE, true), view).out
        });
        assert!(shown.contains("\u{2713} allowed \u{b7} from the phone\n"), "{shown}");
        // An `a` answer and what it saved make one line.
        let shown = settled(|view| {
            let held = framed(view.answered(call(), ApprovalDecision::AllowAlways, WIDE), view);
            let saved = saved(AlwaysSaved::Saved { index: 2, rule: RULE.to_owned() });
            let line = framed(view.envelope(&sent(13, 0, saved), WIDE, true), view);
            format!("{}{}", held.out, line.out)
        });
        let line =
            "\u{2713} allowed always \u{b7} saved as sandbox.allow[2]: write under ~/notes\n";
        assert!(shown.contains(line), "{shown}");
        assert_eq!(shown.matches("allowed always").count(), 1, "{shown}");
        // An expiry says how long the question waited, on the daemon's clock.
        let shown = settled(|view| {
            let expired = Event::ApprovalExpired { turn_id: turn(), call_id: call(), away: false };
            framed(view.envelope(&sent(13, 600_000, expired), WIDE, true), view).out
        });
        let line = "\u{2717} the approval expired \u{b7} nobody answered in 10 min; the call did \
                    not run\n";
        assert!(shown.contains(line), "{shown}");
        let shown = settled(|view| {
            let expired = Event::ApprovalExpired { turn_id: turn(), call_id: call(), away: true };
            framed(view.envelope(&sent(13, 60_000, expired), WIDE, true), view).out
        });
        let line = "\u{2717} expired while you were away \u{b7} the call did not run\n";
        assert!(shown.contains(line), "{shown}");
    }
}

#[test]
fn the_bell_rings_once_with_the_frame_that_first_shows_a_card() {
    use super::super::Look;

    let bell = |on: bool| {
        let look = Look { bell: on, ..Look::default() };
        let mut view = sandbox_view(true).with_look(look);
        let [record, approval] = write_events(Some(RULE));
        framed(view.event(&record, WIDE, true), &mut view);
        let first = framed(view.event(&approval, WIDE, true), &mut view).out;
        let later = view.tick(WIDE, now());
        let confirm = framed(view.confirm_always(call(), WIDE), &mut view).out;
        (first, format!("{later}{confirm}"))
    };
    let (first, later) = bell(true);
    assert_eq!(first.matches('\x07').count(), 1, "{}", readable(&first));
    assert!(!later.contains('\x07'), "{}", readable(&later));
    // `render.bell` is off by default.
    let (first, later) = bell(false);
    assert!(!first.contains('\x07') && !later.contains('\x07'), "{}", readable(&first));
}

//! Always-allow rules in the view (phase E): the `a` key on an exit question that offers
//! a rule, its confirm, the line of what an `a` answer saved, and the line of a call
//! that a rule covered.

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
use crate::testing::{call, exit_info, readable, turn};

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
    assert!(err.ends_with("\u{2502} y allow \u{b7} n deny\n"), "{err}");
    assert!(!view.offers_always(call()));
    // A question that another client must answer offers nothing here.
    let mut view = sandbox_view(false);
    let (_, err, _) = feed(&mut view, &write_events(Some(RULE)), false);
    assert!(err.ends_with("\u{2502} waiting for another client to answer\n"), "{err}");
    assert!(!view.offers_always(call()));
}

#[test]
fn the_rule_from_the_daemon_cannot_drive_the_terminal() {
    let mut view = sandbox_view(false);
    let (_, err, _) = feed(&mut view, &write_events(Some("write under ~/a\x1b[2J\nb")), true);
    assert!(err.ends_with("a always: write under ~/a\u{241b}[2J b\n"), "{err:?}");
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
fn a_refused_always_answer_asks_again_without_the_a() {
    let mut view = sandbox_view(false);
    feed(&mut view, &write_events(Some(RULE)), true);
    let step = view.confirm_always(call(), WIDE);
    assert_eq!(step.err, "\u{2502} save rule \"write under ~/notes\"? y/n\n");
    let step = view.always_refused(call(), "the question of the call offers no always rule", WIDE);
    assert_eq!(
        step.err,
        "\nthe answer was not taken: the question of the call offers no always rule\n\
         \u{2502} y allow \u{b7} n deny\n"
    );
    assert!(!view.offers_always(call()));
    // The question is open still: `y` answers it.
    let step = view.answered(call(), ApprovalDecision::Allow, WIDE);
    assert!(step.err.contains("\u{2713} allowed\n"), "{:?}", step.err);
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
    assert!(err.contains("\u{2713} allowed always from efr\n"), "{err}");
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

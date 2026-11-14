//! The limits on refusals in `auto`: classifier denials pause the classifier for the
//! turn, floor refusals stop it, and only a person resets the floor count.

use std::sync::Arc;

use efr_permissions::Requirements;
use efr_protocol::{
    ApprovalDecision, ErrorCode, Event, JudgeKind, Mode, PauseReason, TurnId, Verdict,
};
use pretty_assertions::assert_eq;
use serde_json::Value;

use super::review::{judgements, reviewed, shell};
use crate::JudgeOutcome;
use crate::exit::refusals_stopped;
use crate::testing::{FakeJudge, Harness, Script, allows, denies, unanswered};

/// Runs a turn of the setup of `judge` whose model makes `calls` after the user said
/// `said`, answering each question with `answer`, with `declared` requirements for
/// their keys. Returns the harness, the model and the terminal event.
async fn run_answering(
    judge: &Arc<FakeJudge>,
    away: bool,
    calls: Vec<(&str, Value)>,
    declared: Vec<(String, Requirements)>,
    answer: ApprovalDecision,
) -> (Harness, Arc<Script>, Event) {
    let setup = reviewed(Some(Arc::clone(judge)));
    setup.presence.set_away(away);
    let script = Script::new(calls);
    let mut h = setup.start_model(Arc::clone(&script) as Arc<dyn efr_provider::Provider>).await;
    for (key, requirements) in declared {
        h.toolbox.declared.lock().unwrap().insert(key, requirements);
    }
    let sent = h.prompt("push this branch").await;
    let end = answer_until_end(&mut h, sent.turn_id, answer).await;
    (h, script, end)
}

/// Answers every question of the turn `turn_id` with `answer` until it ends.
async fn answer_until_end(h: &mut Harness, turn_id: TurnId, answer: ApprovalDecision) -> Event {
    loop {
        let event = h
            .wait_for(|e| {
                matches!(e, Event::ApprovalRequested { .. })
                    || (e.turn_id() == Some(turn_id)
                        && matches!(
                            e,
                            Event::TurnCompleted { .. }
                                | Event::TurnFailed { .. }
                                | Event::TurnInterrupted { .. }
                        ))
            })
            .await;
        match event {
            Event::ApprovalRequested { call_id, .. } => {
                h.answer(call_id, answer).await;
            }
            end => return end,
        }
    }
}

/// `n` pushes.
fn pushes(n: usize) -> Vec<(&'static str, Value)> {
    (0..n).map(|_| shell("git push origin main")).collect()
}

/// The `classifier_paused` events of the log.
async fn pauses(h: &Harness) -> Vec<(PauseReason, u8)> {
    h.events()
        .await
        .into_iter()
        .filter_map(|event| match event {
            Event::ClassifierPaused { reason, denials, .. } => Some((reason, denials)),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn denials_three_in_a_row_pause_classifier() {
    let judge = FakeJudge::answering(vec![denies(), denies(), denies()]);
    let (h, script, end) =
        run_answering(&judge, false, pushes(3), Vec::new(), ApprovalDecision::Deny).await;

    assert!(matches!(end, Event::TurnCompleted { .. }), "a pause does not stop the turn: {end:?}");
    assert_eq!(pauses(&h).await, vec![(PauseReason::InARow, 3)]);
    let kinds = h.kinds().await;
    let paused = kinds.iter().position(|kind| kind == "classifier_paused").unwrap();
    let judged = kinds.iter().rposition(|kind| kind == "exit_judged").unwrap();
    assert_eq!(paused, judged + 1, "the pause comes with the third denial");
    assert_eq!(script.results().len(), 3);
    let seen = judge.seen();
    assert_eq!(seen[2].record.facts.denials_in_a_row, 2, "the record shows the denials before it");
    assert_eq!(seen[2].record.facts.denials_this_turn, 2);
}

#[tokio::test]
async fn denials_ten_per_turn_pause_classifier() {
    // Two denials and an allow, four times, then two denials: never three in a row, and
    // the tenth denial in the turn at the fourteenth exit. The fifteenth stops the turn.
    let mut outcomes: Vec<JudgeOutcome> = Vec::new();
    for _ in 0..5 {
        outcomes.extend([denies(), denies(), allows()]);
    }
    let judge = FakeJudge::answering(outcomes);
    let (h, _, end) =
        run_answering(&judge, false, pushes(15), Vec::new(), ApprovalDecision::Deny).await;

    let Event::TurnFailed { error, .. } = end else {
        panic!("the exit after the pause stops the turn: {end:?}");
    };
    assert_eq!(
        error.message,
        "auto stopped this turn: 10 actions of this turn were refused. Read the answers, then \
         send a new prompt that names what you want."
    );
    assert_eq!(pauses(&h).await, vec![(PauseReason::PerTurn, 10)]);
    assert_eq!(judge.calls(), 14, "nobody judges the fifteenth exit");
    assert_eq!(judgements(&h).await.last(), Some(&(JudgeKind::Classifier, Verdict::Deny)));
}

/// After the pause the model can still finish the work that stays in the sandbox. Its
/// next exit stops the turn, with the user present or away: nobody asks the user.
#[rstest::rstest]
#[case::present(false)]
#[case::away(true)]
#[tokio::test]
async fn paused_classifier_stops_the_turn_at_the_next_exit(#[case] away: bool) {
    let judge = FakeJudge::answering(vec![denies(), denies(), denies(), allows()]);
    let (h, script, end) =
        run_answering(&judge, away, pushes(4), Vec::new(), ApprovalDecision::Allow).await;

    let Event::TurnFailed { error, .. } = end else {
        panic!("nobody decides the fourth exit, so the turn stops: {end:?}");
    };
    assert_eq!(error.code, ErrorCode::Forbidden);
    assert_eq!(
        error.message,
        "auto stopped this turn: 3 actions in a row were refused. Read the answers, then send \
         a new prompt that names what you want."
    );
    assert_eq!(judge.calls(), 3, "nobody judges the fourth exit");
    assert_eq!(h.kinds().await.iter().filter(|kind| *kind == "approval_requested").count(), 0);
    // The fourth exit, which stopped the turn, keeps its record too.
    assert_eq!(h.kinds().await.iter().filter(|kind| *kind == "exit_requested").count(), 4);
    // The turn ends after the fourth call, so no model call reads its result.
    assert_eq!(script.results().len(), 3);
    assert!(h.toolbox.invoked().is_empty());
}

#[tokio::test]
async fn errors_and_expiries_are_not_denials() {
    // Each step of the chain failed, twice in a row, two times: none of those calls
    // runs, nothing pauses, and no record counts them as denials.
    let outcomes = vec![unanswered(), unanswered(), denies(), unanswered(), unanswered()];
    let judge = FakeJudge::answering(outcomes);
    let (h, _, end) =
        run_answering(&judge, false, pushes(5), Vec::new(), ApprovalDecision::Deny).await;

    assert!(matches!(end, Event::TurnCompleted { .. }), "{end:?}");
    assert_eq!(pauses(&h).await, Vec::new());
    let seen = judge.seen();
    assert_eq!(seen.len(), 5);
    let denials: Vec<u8> = seen.iter().map(|seen| seen.record.facts.denials_this_turn).collect();
    assert_eq!(denials, [0, 0, 0, 1, 1], "only the reviewer's own denial counts");
}

#[tokio::test]
async fn allow_resets_counter() {
    let judge = FakeJudge::answering(vec![denies(), denies(), allows(), denies(), denies()]);
    let (h, _, end) =
        run_answering(&judge, false, pushes(5), Vec::new(), ApprovalDecision::Deny).await;

    assert!(matches!(end, Event::TurnCompleted { .. }), "{end:?}");
    assert_eq!(pauses(&h).await, Vec::new());
    let seen = judge.seen();
    assert_eq!(seen[4].record.facts.denials_in_a_row, 1);
    assert_eq!(seen[4].record.facts.denials_this_turn, 3);
}

#[tokio::test]
async fn a_silent_reviewer_does_not_reset_the_count() {
    // A review with no answer is no denial and no allow: the count in a row stays.
    let judge = FakeJudge::answering(vec![denies(), denies(), unanswered(), denies()]);
    let (h, _, end) =
        run_answering(&judge, false, pushes(4), Vec::new(), ApprovalDecision::Deny).await;

    assert!(matches!(end, Event::TurnCompleted { .. }), "{end:?}");
    assert_eq!(judge.seen()[3].record.facts.denials_in_a_row, 2);
    assert_eq!(pauses(&h).await, vec![(PauseReason::InARow, 3)]);
}

#[tokio::test]
async fn classifier_allow_keeps_floor_count() {
    let judge = FakeJudge::answering(vec![allows()]);
    let setup = reviewed(Some(Arc::clone(&judge)));
    let key = std::path::PathBuf::from("/etc/shadow");
    let line = format!("cat {}", key.display());
    let calls = vec![shell(&line), shell(&line), shell("git push origin main"), shell(&line)];
    let script = Script::new(calls);
    let mut h = setup.start_model(Arc::clone(&script) as Arc<dyn efr_provider::Provider>).await;
    let requirements = Requirements::none().with_command(&line).with_read(&key);
    h.toolbox.declared.lock().unwrap().insert(line.clone(), requirements);
    let sent = h.prompt("push this branch").await;
    let end = answer_until_end(&mut h, sent.turn_id, ApprovalDecision::Deny).await;

    let Event::TurnFailed { error, .. } = end else {
        panic!("three floor refusals stop the turn: {end:?}");
    };
    assert_eq!(error.message, refusals_stopped(Mode::Auto));
    assert_eq!(
        judgements(&h).await,
        vec![
            (JudgeKind::Floor, Verdict::Deny),
            (JudgeKind::Floor, Verdict::Deny),
            (JudgeKind::Classifier, Verdict::Allow),
            (JudgeKind::Floor, Verdict::Deny),
        ]
    );
}

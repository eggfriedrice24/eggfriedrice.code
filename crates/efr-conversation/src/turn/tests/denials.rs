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
async fn paused_classifier_asks_present_user() {
    let judge = FakeJudge::answering(vec![denies(), denies(), denies(), allows()]);
    let (h, _, end) =
        run_answering(&judge, false, pushes(4), Vec::new(), ApprovalDecision::Allow).await;

    assert!(matches!(end, Event::TurnCompleted { .. }), "{end:?}");
    assert_eq!(judge.calls(), 3, "the fourth exit goes to the user");
    let judged = judgements(&h).await;
    assert_eq!(judged.last(), Some(&(JudgeKind::User, Verdict::Allow)));
    assert_eq!(h.toolbox.invoked().len(), 1);
}

#[tokio::test]
async fn denials_ten_per_turn_pause_classifier() {
    // Two denials and an allow, four times, then two denials: never three in a row, and
    // the tenth denial in the turn at the fourteenth exit.
    let mut outcomes: Vec<JudgeOutcome> = Vec::new();
    for _ in 0..5 {
        outcomes.extend([denies(), denies(), allows()]);
    }
    let judge = FakeJudge::answering(outcomes);
    let (h, _, end) =
        run_answering(&judge, false, pushes(15), Vec::new(), ApprovalDecision::Deny).await;

    assert!(matches!(end, Event::TurnCompleted { .. }), "{end:?}");
    assert_eq!(pauses(&h).await, vec![(PauseReason::PerTurn, 10)]);
    assert_eq!(judge.calls(), 14, "the fifteenth exit goes to the user");
    assert_eq!(judgements(&h).await.last(), Some(&(JudgeKind::User, Verdict::Deny)));
}

#[tokio::test]
async fn paused_classifier_stops_turn_when_away() {
    let judge = FakeJudge::answering(vec![denies(), denies(), denies()]);
    let (h, script, end) =
        run_answering(&judge, true, pushes(4), Vec::new(), ApprovalDecision::Allow).await;

    let Event::TurnFailed { error, .. } = end else {
        panic!("nobody can decide, so the turn stops: {end:?}");
    };
    assert_eq!(error.code, ErrorCode::Forbidden);
    // The text of efr's auto spec, section 14.7.
    assert_eq!(
        error.message,
        "auto stopped this turn: the reviewer refused 3 actions in a row and nobody could \
         decide. Read the answers, then send a new prompt."
    );
    assert_eq!(h.kinds().await.iter().filter(|kind| *kind == "approval_requested").count(), 0);
    // The fourth exit, which stopped the turn, keeps its record too.
    assert_eq!(h.kinds().await.iter().filter(|kind| *kind == "exit_requested").count(), 4);
    // The turn ends after the fourth call, so no model call reads its result.
    assert_eq!(script.results().len(), 3);
    assert!(h.toolbox.invoked().is_empty());
}

#[tokio::test]
async fn errors_and_expiries_are_not_denials() {
    // Each step of the chain failed three times: the user decides each, and nothing
    // pauses.
    let judge = FakeJudge::answering(vec![unanswered(), unanswered(), unanswered(), denies()]);
    let (h, _, end) =
        run_answering(&judge, false, pushes(4), Vec::new(), ApprovalDecision::Deny).await;

    assert!(matches!(end, Event::TurnCompleted { .. }), "{end:?}");
    assert_eq!(pauses(&h).await, Vec::new());
    let seen = judge.seen();
    assert_eq!(seen.len(), 4);
    assert!(seen.iter().all(|seen| seen.record.facts.denials_this_turn == 0));
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
async fn user_answer_resets_counter() {
    let judge = FakeJudge::answering(vec![denies(), denies(), unanswered(), denies(), denies()]);
    let (h, _, end) =
        run_answering(&judge, false, pushes(5), Vec::new(), ApprovalDecision::Deny).await;

    assert!(matches!(end, Event::TurnCompleted { .. }), "{end:?}");
    assert_eq!(pauses(&h).await, Vec::new(), "the user's answer started the count again");
    assert_eq!(judge.seen()[4].record.facts.denials_in_a_row, 1);
}

#[tokio::test]
async fn classifier_allow_keeps_floor_count() {
    let judge = FakeJudge::answering(vec![allows()]);
    let setup = reviewed(Some(Arc::clone(&judge)));
    let key = setup.home().join(".ssh/id_ed25519");
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

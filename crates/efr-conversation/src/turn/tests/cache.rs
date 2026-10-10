//! Deletes that a contained call made in its private cache layer, raised as a `write`
//! exit after the call and applied to the real caches after an allow.

use std::path::PathBuf;
use std::sync::Arc;

use efr_protocol::{
    ApprovalDecision, Event, ExitKind, ExitSource, JudgeKind, SandboxSummary, Verdict,
};
use pretty_assertions::assert_eq;

use super::review::{judgements, reviewed, shell};
use crate::ToolOutcome;
use crate::testing::{FakeJudge, Harness, Script, allows, denies};

/// A contained call's outcome that reports deletes of `paths` in its cache layer.
fn deleted(paths: &[PathBuf]) -> ToolOutcome {
    let summary = SandboxSummary {
        confined: true,
        cache_deletes: paths.to_vec(),
        ..SandboxSummary::default()
    };
    ToolOutcome::ok("done").with_exit_code(Some(0)).with_sandbox(Some(summary))
}

/// Runs a turn of the setup of `judge` whose model runs `line` `times` times, each call
/// reporting the deletes of `paths` (made from the harness's home), and answers each
/// question with `answer`.
async fn run(
    judge: Option<Arc<FakeJudge>>,
    line: &str,
    times: usize,
    paths: &[&str],
    answer: ApprovalDecision,
) -> (Harness, Arc<Script>, Vec<PathBuf>) {
    let setup = reviewed(judge);
    let paths: Vec<PathBuf> = paths.iter().map(|path| setup.home().join(path)).collect();
    let script = Script::new((0..times).map(|_| shell(line)).collect());
    let mut h = setup.start_model(Arc::clone(&script) as Arc<dyn efr_provider::Provider>).await;
    h.toolbox.outcomes.lock().unwrap().insert(line.to_owned(), deleted(&paths));
    let sent = h.prompt("free some disk space").await;
    loop {
        let event = h
            .wait_for(|e| {
                matches!(e, Event::ApprovalRequested { .. })
                    || (e.turn_id() == Some(sent.turn_id)
                        && matches!(e, Event::TurnCompleted { .. }))
            })
            .await;
        match event {
            Event::ApprovalRequested { call_id, .. } => {
                h.answer(call_id, answer).await;
            }
            _ => break,
        }
    }
    (h, script, paths)
}

#[tokio::test]
async fn cache_delete_exit_goes_to_classifier() {
    let judge = FakeJudge::answering(vec![allows()]);
    let (h, script, paths) = run(
        Some(Arc::clone(&judge)),
        "go clean -cache",
        1,
        &[".cache/go-build/ab"],
        ApprovalDecision::Deny,
    )
    .await;

    assert_eq!(h.toolbox.deleted.lock().unwrap().clone(), vec![paths.clone()]);
    let seen = judge.seen();
    assert_eq!(seen.len(), 1);
    let record = &seen[0].record;
    assert_eq!(record.action.source, ExitSource::CacheDelete);
    assert_eq!(record.action.exits, vec![ExitKind::Write]);
    assert_eq!(record.action.line, "go clean -cache", "the classifier reads the line");
    assert_eq!(record.facts.targets[0].class, Some(efr_protocol::PathClassName::Cache));
    assert!(!record.facts.targets[0].named_in_user_messages, "no name is needed");
    assert_eq!(judgements(&h).await, vec![(JudgeKind::Classifier, Verdict::Allow)]);
    let events = h.events().await;
    let call_id = h.call_ids().await[0];
    assert!(events.contains(&Event::CacheDeleteApplied {
        turn_id: events.iter().find_map(|e| e.turn_id()).unwrap(),
        call_id,
        paths: paths.clone(),
        freed_bytes: 1000,
    }));
    // The review has a call id of its own; the deletes name the call that made them.
    let Some(Event::ExitRequested { call_id: asked, .. }) =
        events.iter().find(|e| matches!(e, Event::ExitRequested { .. }))
    else {
        panic!("the deletes have their record");
    };
    assert_ne!(*asked, call_id);
    let kinds = h.kinds().await;
    let at = |kind: &str| kinds.iter().position(|known| known == kind).unwrap();
    assert!(at("tool_call_completed") < at("exit_requested"));
    assert!(at("exit_judged") < at("cache_delete_applied"));
    assert_eq!(
        script.results(),
        vec![(
            "done\n[efr deleted 1 path in the real tool caches and freed 1000 bytes.]".to_owned(),
            false
        )]
    );
}

#[tokio::test]
async fn denied_cache_delete_not_raised_again() {
    let judge = FakeJudge::answering(vec![denies()]);
    let (h, script, _) = run(
        Some(Arc::clone(&judge)),
        "pip cache purge",
        2,
        &[".cache/pip/x"],
        ApprovalDecision::Deny,
    )
    .await;

    assert_eq!(judge.calls(), 1, "the second call raises the same paths no more");
    assert!(h.toolbox.deleted.lock().unwrap().is_empty());
    let results = script.results();
    assert_eq!(
        results[0].0,
        "done\n[the deletes in the tool caches stayed private: the deletes were refused.]"
    );
    assert_eq!(results[1].0, "done");
    assert_eq!(h.toolbox.invoked().len(), 2, "a denied delete never stops the call itself");
}

#[tokio::test]
async fn a_cache_delete_below_a_pin_stays_private_with_no_question() {
    let judge = FakeJudge::answering(vec![allows()]);
    let (h, script, _) = run(
        Some(Arc::clone(&judge)),
        "cargo uninstall x",
        1,
        &[".cargo/bin/x"],
        ApprovalDecision::Deny,
    )
    .await;

    assert_eq!(judge.calls(), 0);
    assert_eq!(h.kinds().await.iter().filter(|kind| *kind == "exit_requested").count(), 0);
    assert!(h.toolbox.deleted.lock().unwrap().is_empty());
    assert_eq!(
        script.results()[0].0,
        "done\n[the deletes in the tool caches stayed private: efr deletes none of these paths.]"
    );
}

#[tokio::test]
async fn without_a_judge_the_user_decides_a_cache_delete() {
    let (h, _, paths) = run(
        None,
        "go clean -cache",
        1,
        &[".cache/go-build/ab", ".cache/go-build/cd"],
        ApprovalDecision::Allow,
    )
    .await;

    assert_eq!(h.toolbox.deleted.lock().unwrap().clone(), vec![paths.clone()]);
    let events = h.events().await;
    let Some(Event::ApprovalRequested { summary, exit: Some(exit), .. }) =
        events.iter().find(|e| matches!(e, Event::ApprovalRequested { .. }))
    else {
        panic!("the user is asked");
    };
    assert_eq!(
        *summary,
        format!("delete 2 paths in the real tool caches, such as {}", paths[0].display())
    );
    assert_eq!(exit.kinds, vec![ExitKind::Write]);
    assert!(!exit.user_only);
    assert_eq!(judgements(&h).await, vec![(JudgeKind::User, Verdict::Allow)]);
}

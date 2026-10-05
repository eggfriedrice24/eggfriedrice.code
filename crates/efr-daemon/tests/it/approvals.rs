//! Approvals over a `TestDaemon`: a write outside `$SCRATCH` waits for the user, runs
//! when allowed and never runs when denied; an answer is a write with a receipt.

use efr_protocol::{
    ApprovalDecision, ApprovalRespond, ApprovalRespondResult, ErrorCode, Event, Method,
};
use efr_test_daemon::{ClientError, Replay, command_id};
use pretty_assertions::assert_eq;
use serde_json::Value;

fn answer(replay: &Replay, command: u128, decision: ApprovalDecision) -> Method {
    let call_id = replay.bindings().get("<call:1>").unwrap().parse().unwrap();
    Method::ApprovalRespond(ApprovalRespond {
        command_id: command_id(command),
        conversation_id: replay.conversation().unwrap(),
        call_id,
        decision,
    })
}

#[tokio::test]
async fn approval_ask_then_allow() {
    let replay = Replay::run("approval_ask_then_allow").await.unwrap();

    let note = replay.daemon().cwd().join("note.txt");
    assert_eq!(std::fs::read_to_string(&note).unwrap(), "remember the milk\n");
    let events = replay.events().await.unwrap();
    let asked = events.iter().position(|e| e.event.kind() == "approval_requested").unwrap();
    let resolved = events.iter().position(|e| e.event.kind() == "approval_resolved").unwrap();
    let completed = events.iter().position(|e| e.event.kind() == "tool_call_completed").unwrap();
    assert!(asked < resolved && resolved < completed, "the call ran only after the answer");
    // The user sees what the write would put there before answering.
    let Event::ApprovalRequested { diff_preview, .. } = &events[asked].event else {
        panic!("{:?}", events[asked]);
    };
    assert_eq!(
        diff_preview.as_deref(),
        Some(
            format!(
                "--- /dev/null\n+++ b{}\n@@ -0,0 +1,1 @@\n+remember the milk\n",
                note.display()
            )
            .as_str()
        )
    );
    replay.stop().await.unwrap();
}

#[tokio::test]
async fn approval_deny() {
    let replay = Replay::run("approval_deny").await.unwrap();

    assert!(!replay.daemon().cwd().join("note.txt").exists(), "a denied write never runs");
    let events = replay.events().await.unwrap();
    let denied = events.iter().find_map(|envelope| match &envelope.event {
        Event::ToolCallCompleted { is_error, output, .. } => Some((*is_error, output.clone())),
        _ => None,
    });
    let (is_error, output) = denied.unwrap();
    assert!(is_error);
    assert!(output.contains("denied"), "{output}");
    replay.stop().await.unwrap();
}

#[tokio::test]
async fn a_retried_answer_gets_its_receipt_and_a_late_one_is_not_found() {
    let replay = Replay::run("approval_ask_then_allow").await.unwrap();
    let first = replay.result(2).unwrap().clone().unwrap();

    // The same command again: answered from the receipt, nothing runs twice.
    let retried: Value =
        replay.client().call(answer(&replay, 2, ApprovalDecision::Allow)).await.unwrap();
    assert_eq!(retried, first);
    let parsed: ApprovalRespondResult = serde_json::from_value(retried).unwrap();
    assert!(parsed.seq.get() > 0);

    // A new answer to the call that is no longer pending.
    let late = replay.client().call::<Value>(answer(&replay, 3, ApprovalDecision::Deny)).await;
    let Err(ClientError::Server { body }) = late else { panic!("{late:?}") };
    assert_eq!(body.code, ErrorCode::NotFound);
    let resolved = replay.events().await.unwrap();
    let resolved = resolved.iter().filter(|e| e.event.kind() == "approval_resolved");
    assert_eq!(resolved.count(), 1);
    replay.stop().await.unwrap();
}

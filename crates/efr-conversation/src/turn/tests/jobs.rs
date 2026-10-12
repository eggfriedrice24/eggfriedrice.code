//! Background jobs at the check point (phase C): what starts at once, what the
//! classifier or the user decides, what no job may hold, and the question that each
//! job asks outside `auto`.

use std::sync::Arc;

use efr_protocol::Event;
use pretty_assertions::assert_eq;
use serde_json::{Value, json};

use super::review::{reviewed, run};
use crate::testing::{FakeJudge, Script, Setup};

/// One shell call of `line` that starts a background job.
fn job(line: &str) -> (&'static str, Value) {
    ("shell", json!({ "command": line, "background": true }))
}

#[tokio::test]
async fn privilege_job_refused_as_job() {
    let judge = FakeJudge::answering(Vec::new());
    let (h, script) =
        run(reviewed(Some(Arc::clone(&judge))), vec![job("docker run --rm alpine")], "go").await;

    let results = script.results();
    assert_eq!(results.len(), 1);
    let (output, is_error) = &results[0];
    assert!(*is_error);
    assert_eq!(
        output,
        "The shell call did not start a job: \"docker run --rm alpine\" cannot run as a job. \
         Run that part in the foreground, and the rest as a job."
    );
    assert_eq!(judge.calls(), 0, "no review");
    assert!(h.toolbox.invoked().is_empty(), "no job started");
    let events = h.events().await;
    assert!(!events.iter().any(|e| matches!(e, Event::ApprovalRequested { .. })), "no question");
}

#[tokio::test]
async fn job_refused_kind_refused_in_cautious() {
    let script = Script::new(vec![job("xdotool type hello")]);
    let mut h =
        Setup::new().start_model(Arc::clone(&script) as Arc<dyn efr_provider::Provider>).await;
    let sent = h.prompt("type hello for me later").await;
    h.wait_end(sent.turn_id).await;

    let (output, is_error) = &script.results()[0];
    assert!(*is_error && output.contains("cannot run as a job"), "{output}");
    assert!(h.toolbox.invoked().is_empty());
    let events = h.events().await;
    assert!(!events.iter().any(|e| matches!(e, Event::ApprovalRequested { .. })), "no question");
    let refusal = events.iter().find_map(|event| match event {
        Event::ToolCallCompleted { refusal, .. } => refusal.clone(),
        _ => None,
    });
    assert_eq!(
        refusal.as_deref(),
        Some("run \"xdotool type hello\" as a job (desktop_ipc): it cannot run as a job")
    );
}

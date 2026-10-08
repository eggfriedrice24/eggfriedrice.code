//! `apply_patch` end to end, through the real subscription provider against a local
//! Responses server: the request offers the tool in its freeform (`custom`) form with
//! its grammar, the model answers with a `custom_tool_call` whose input is the patch
//! text, and the result goes back as a `custom_tool_call_output`.

use efr_protocol::{
    AdminProjectAdd, AdminProjectAddResult, ApprovalDecision, ApprovalRespond,
    ApprovalRespondResult, ChangeKind, Event, EventEnvelope, Method, PromptSendResult,
};
use efr_test_daemon::{
    ResponsesAnswer, ResponsesServer, TTY, TestDaemon, command_id, events_until,
};
use pretty_assertions::assert_eq;
use serde_json::{Value, json};

/// The text of a patch that changes one line of `src/lib.rs`.
const UPDATE: &str = "*** Begin Patch\n*** Update File: src/lib.rs\n@@ fn main\n-    old();\n+    new();\n*** End Patch\n";

/// A daemon on the subscription provider whose model calls `apply_patch` with `patch`
/// and then answers `Done.`, with its working directory registered as a project that
/// holds `src/lib.rs`.
async fn daemon_with_patch(patch: &str) -> (ResponsesServer, TestDaemon) {
    let server = ResponsesServer::start().await;
    server.push(ResponsesAnswer::custom_tool_call("call_patch_1", "apply_patch", patch));
    server.push(ResponsesAnswer::text("Done."));
    let daemon = TestDaemon::builder().subscription(&server).start().await.unwrap();
    std::fs::create_dir_all(daemon.cwd().join("src")).unwrap();
    std::fs::write(daemon.cwd().join("src/lib.rs"), "fn main() {\n    old();\n}\n").unwrap();
    let client = daemon.client().await.unwrap();
    let add = Method::AdminProjectAdd(AdminProjectAdd {
        path: daemon.cwd().to_path_buf(),
        name: None,
        git_root: false,
    });
    let _: AdminProjectAddResult = client.call(add).await.unwrap();
    (server, daemon)
}

/// A turn's events, with every approval answered `decision`.
async fn run_turn(daemon: &TestDaemon, decision: ApprovalDecision) -> Vec<EventEnvelope> {
    let client = daemon.client_for_tty(TTY).await.unwrap();
    let sent: PromptSendResult = client.call(daemon.prompt(1, "edit it", TTY)).await.unwrap();
    let mut follow = daemon.follow(&client, sent.conversation_id).await.unwrap();
    let mut seen = Vec::new();
    let mut answers = 10;
    loop {
        let events = events_until(&mut follow, |event| {
            matches!(
                event,
                Event::ApprovalRequested { .. }
                    | Event::TurnCompleted { .. }
                    | Event::TurnFailed { .. }
            )
        })
        .await
        .unwrap();
        let last = events.last().unwrap().event.clone();
        seen.extend(events);
        let Event::ApprovalRequested { call_id, .. } = last else { break };
        answers += 1;
        let answer = Method::ApprovalRespond(ApprovalRespond {
            command_id: command_id(answers),
            conversation_id: sent.conversation_id,
            call_id,
            decision,
        });
        let _: ApprovalRespondResult = client.call(answer).await.unwrap();
    }
    seen
}

/// The output that the model read for its call, from the second request.
fn output_sent_back(server: &ResponsesServer) -> Value {
    let requests = server.received();
    assert_eq!(requests.len(), 2, "the call, then the answer");
    let input = requests[1].body["input"].as_array().unwrap().clone();
    input
        .into_iter()
        .find(|item| item["type"] == "custom_tool_call_output")
        .unwrap_or_else(|| panic!("no custom output in {:#?}", requests[1].body["input"]))
}

#[tokio::test]
async fn a_custom_tool_call_with_a_patch_runs_and_its_result_goes_back_as_custom_output() {
    let (server, daemon) = daemon_with_patch(UPDATE).await;

    let events = run_turn(&daemon, ApprovalDecision::Allow).await;

    let tools = server.received()[0].body["tools"].as_array().unwrap().clone();
    let tool = tools.iter().find(|tool| tool["name"] == "apply_patch").unwrap();
    assert_eq!(tool["type"], "custom", "{tool:#}");
    assert_eq!(tool["format"]["type"], "grammar");
    assert_eq!(tool["format"]["syntax"], "lark");
    assert!(tool["format"]["definition"].as_str().unwrap().contains("*** Begin Patch"));

    let started = events.iter().find_map(|envelope| match &envelope.event {
        Event::ToolCallStarted { tool, input, freeform, .. } => {
            Some((tool.clone(), input.clone(), *freeform))
        }
        _ => None,
    });
    assert_eq!(started, Some(("apply_patch".to_owned(), json!(UPDATE), true)));

    let output = output_sent_back(&server);
    assert_eq!(output["call_id"], "call_patch_1");
    assert!(events.iter().any(|envelope| matches!(envelope.event, Event::TurnCompleted { .. })));
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn a_patch_in_the_project_changes_the_file_and_shows_its_diff_without_a_question() {
    let (server, daemon) = daemon_with_patch(UPDATE).await;

    let events = run_turn(&daemon, ApprovalDecision::Deny).await;

    assert!(
        !events.iter().any(|envelope| matches!(envelope.event, Event::ApprovalRequested { .. }))
    );
    assert_eq!(
        std::fs::read_to_string(daemon.cwd().join("src/lib.rs")).unwrap(),
        "fn main() {\n    new();\n}\n"
    );
    let (changes, diff) = events
        .iter()
        .find_map(|envelope| match &envelope.event {
            Event::ToolCallCompleted { changes, diff, is_error: false, .. } => {
                Some((changes.clone().unwrap(), diff.clone().unwrap()))
            }
            _ => None,
        })
        .unwrap();
    assert_eq!(changes.files[0].path, "src/lib.rs");
    assert_eq!(changes.files[0].kind, ChangeKind::Modified);
    assert_eq!((changes.added, changes.removed), (1, 1));
    assert!(diff.starts_with("--- a/src/lib.rs\n+++ b/src/lib.rs\n"), "{diff}");
    assert_eq!(output_sent_back(&server)["output"], "Success. Updated: src/lib.rs");
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn a_delete_asks_even_in_the_project_and_a_no_keeps_the_file() {
    let patch = "*** Begin Patch\n*** Delete File: src/lib.rs\n*** End Patch\n";
    let (server, daemon) = daemon_with_patch(patch).await;

    let events = run_turn(&daemon, ApprovalDecision::Deny).await;

    let (summary, preview) = events
        .iter()
        .find_map(|envelope| match &envelope.event {
            Event::ApprovalRequested { summary, diff_preview, .. } => {
                Some((summary.clone(), diff_preview.clone().unwrap()))
            }
            _ => None,
        })
        .unwrap();
    assert_eq!(summary, "apply_patch: delete or move files");
    let path = daemon.cwd().join("src/lib.rs");
    assert_eq!(
        preview,
        "delete src/lib.rs\n--- a/src/lib.rs\n+++ /dev/null\n@@ -1,3 +0,0 @@\n\
         -fn main() {\n-    old();\n-}\n",
        "the question names the path as the project shows it"
    );
    assert!(path.exists(), "the user said no");
    let output = output_sent_back(&server);
    assert!(output["output"].as_str().unwrap().contains("denied"), "{output:#}");
    daemon.stop().await.unwrap();
}

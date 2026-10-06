//! The shell tool in the `auto` mode: `needs`, `nested_shell`, the launcher's call and
//! what a contained call reports back.

use std::path::PathBuf;

use efr_protocol::{BusKind, CallId, Needs};
use efr_shell::{CommandResult, Completion, SandboxRun};
use pretty_assertions::assert_eq;
use serde_json::{Value, json};

use super::tool;
use crate::testing::{FakeRunner, Fixture};
use crate::{NoOutput, Tool as _, ToolError};

#[test]
fn the_schema_carries_needs_with_the_limits_of_the_spec() {
    let schema = tool().spec().input_schema;
    let needs = &schema["properties"]["needs"];
    assert_eq!(needs["type"], "object");
    assert_eq!(needs["additionalProperties"], false);
    assert_eq!(needs["properties"]["write"]["maxItems"], 8);
    assert_eq!(needs["properties"]["hosts"]["maxItems"], 8);
    assert_eq!(needs["properties"]["sockets"]["maxItems"], 2);
    assert_eq!(needs["properties"]["unmask"]["maxItems"], 4);
    assert_eq!(needs["properties"]["bus"]["enum"], json!(["system", "session"]));
    assert_eq!(needs["properties"]["reason"]["maxLength"], 300);
    assert_eq!(needs["properties"]["outside"]["type"], "boolean");
    assert!(needs["description"].as_str().unwrap().contains("Only in the auto mode"));
    let required = schema["required"].as_array().unwrap();
    assert_eq!(required, &[Value::from("command")], "needs is optional");
}

#[test]
fn needs_and_nested_shell_are_declared() {
    let fixture = Fixture::new();
    let input = json!({
        "command": "make",
        "needs": { "write": ["~/notes.txt"], "bus": "system", "reason": "it says so" },
    });
    let requirements = tool().requirements(&fixture.context(), &input).unwrap();
    let needs = Needs {
        write: vec!["~/notes.txt".to_owned()],
        bus: Some(BusKind::System),
        reason: Some("it says so".to_owned()),
        ..Needs::default()
    };
    assert_eq!(requirements.needs, Some(needs));
    assert!(!requirements.nested);
    let nested = json!({ "command": "ls", "nested_shell": true });
    let requirements = tool().requirements(&fixture.context(), &nested).unwrap();
    assert!(requirements.nested && requirements.interactive && requirements.needs.is_none());
}

#[test]
fn needs_over_their_limits_are_invalid() {
    let fixture = Fixture::new();
    let nine: Vec<String> = (0..9).map(|n| format!("/tmp/{n}")).collect();
    for needs in [
        json!({ "write": nine }),
        json!({ "sockets": ["/a", "/b", "/c"] }),
        json!({ "reason": "x".repeat(301) }),
    ] {
        let input = json!({ "command": "make", "needs": needs });
        let error = tool().requirements(&fixture.context(), &input).unwrap_err();
        assert!(matches!(error, ToolError::InvalidNeeds { .. }), "{error:?}");
    }
    let unknown = json!({ "command": "make", "needs": { "write": [], "everything": true } });
    assert!(tool().requirements(&fixture.context(), &unknown).is_ok());
    let wrong_bus = json!({ "command": "make", "needs": { "bus": "any" } });
    let error = tool().requirements(&fixture.context(), &wrong_bus).unwrap_err();
    assert!(matches!(error, ToolError::InvalidInput { .. }), "{error:?}");
}

fn sandbox_run() -> SandboxRun {
    let call: CallId = "01920000-0000-7000-8000-000000000003".parse().unwrap();
    // efr-tools has no efr-sandbox edge, so the launch comes from its wire name.
    let launch = serde_json::from_value(json!("contained")).unwrap();
    SandboxRun::new(PathBuf::from("/run/user/1000/efr/sbx/c/x"), call, [7; 16], launch)
}

#[tokio::test]
async fn the_launchers_call_reaches_the_run_request() {
    let fixture = Fixture::new();
    let runner = FakeRunner::answering(Ok(CommandResult::finished(Some(0), "", "/tmp")));
    let tool = super::super::ShellTool::new(runner.clone());
    let context = fixture.context().with_sandbox(Some(sandbox_run()));
    tool.invoke(context, json!({"command": "make"}), &mut NoOutput).await.unwrap();
    assert_eq!(runner.last_request().sandbox, Some(sandbox_run()));
    let debug = format!("{:?}", sandbox_run());
    assert!(debug.contains("[16 bytes]") && !debug.contains("7, 7"), "{debug}");
    // Without one the line goes to the hidden shell, as in the other modes.
    let runner = FakeRunner::answering(Ok(CommandResult::finished(Some(0), "", "/tmp")));
    let tool = super::super::ShellTool::new(runner.clone());
    tool.invoke(fixture.context(), json!({"command": "make"}), &mut NoOutput).await.unwrap();
    assert_eq!(runner.last_request().sandbox, None);
}

/// The model's answer to a run that ended with `exit_code` and the launcher's report
/// `sandbox`, a `result.json`.
async fn answer(exit_code: i32, mut sandbox: Value) -> String {
    // The launcher keeps the state of a call that started, unless a test says not.
    sandbox.as_object_mut().unwrap().entry("state_kept").or_insert(json!(true));
    let fixture = Fixture::new();
    let result = CommandResult::finished(Some(exit_code), "out", "/tmp")
        .with_sandbox(serde_json::from_value(sandbox).unwrap());
    let tool = super::super::ShellTool::new(FakeRunner::answering(Ok(result)));
    let context = fixture.context().with_sandbox(Some(sandbox_run()));
    tool.invoke(context, json!({"command": "make"}), &mut NoOutput).await.unwrap().output
}

#[tokio::test]
async fn a_failed_contained_call_ends_with_the_sandbox_note() {
    let output = answer(1, json!({ "started": true, "summary": { "confined": true } })).await;
    assert_eq!(
        output,
        "out\n[exit code 1, cwd /tmp]\n[efr: this ran in the auto sandbox: it can write only \
         in the turn's project, registered projects that the command names, $SCRATCH, /tmp \
         (private) and the tool caches (private), has no network, and cannot use sudo, D-Bus \
         or other sockets; secrets read as empty. If it failed for that reason, call shell \
         again with needs.]"
    );
    // No note for a call that worked, nor for one that ran in the exit child.
    let worked = answer(0, json!({ "started": true, "summary": { "confined": true } })).await;
    assert_eq!(worked, "out\n[exit code 0, cwd /tmp]");
    let outside = answer(1, json!({ "started": true, "summary": { "confined": false } })).await;
    assert_eq!(outside, "out\n[exit code 1, cwd /tmp]");
}

#[tokio::test]
async fn kept_out_dropped_and_stopped_names_follow_the_output() {
    let sandbox = json!({
        "started": true,
        "summary": {
            "confined": true,
            "promoted": ["VIRTUAL_ENV"],
            "kept_out": ["PATH", "PYTHONPATH"],
            "dropped": ["LD_PRELOAD"],
            "background_stopped": ["node"],
            "blocked": [{ "host": "evil.example", "port": 443, "reason": "not_allowed" }],
            "surface_changes": [
                { "path": "/home/u/p/app/.git/commondir", "rule": "commondir_in_main_git_dir", "quarantined": true },
                { "path": "/home/u/p/app/.envrc", "rule": "protected_name_created", "quarantined": false }
            ]
        }
    });
    let output = answer(0, sandbox).await;
    let lines: Vec<&str> = output.lines().collect();
    assert_eq!(
        lines[2..],
        [
            "[efr: these exports stay in the sandbox: PATH, PYTHONPATH. Later contained calls \
             see them; the hidden shell and a command that runs outside the sandbox do not.]",
            "[efr: these exports were dropped, and no later call sees them: LD_PRELOAD.]",
            "[efr: these background jobs stopped when the command ended: node. Start a server \
             and its test in one command.]",
            "[efr: the network proxy refused: evil.example:443. To reach a host, call shell \
             again with needs.hosts.]",
            "[efr: the command changed git settings that run code; efr moved them out of the \
             way, and the user decides whether to keep them: /home/u/p/app/.git/commondir.]",
        ]
    );
    assert!(!output.contains("VIRTUAL_ENV"), "a promoted name needs no note");
}

#[tokio::test]
async fn a_run_that_is_still_going_gets_no_sandbox_note() {
    let fixture = Fixture::new();
    let result = CommandResult::finished(None, "", "/tmp")
        .with_completion(Completion::StillRunning)
        .with_sandbox(
            serde_json::from_value(json!({ "started": true, "summary": { "confined": true } }))
                .unwrap(),
        );
    let tool = super::super::ShellTool::new(FakeRunner::answering(Ok(result)));
    let output =
        tool.invoke(fixture.context(), json!({"command": "make"}), &mut NoOutput).await.unwrap();
    assert!(!output.output.contains("auto sandbox"), "{}", output.output);
}

#[test]
fn the_description_names_needs_and_the_rules_of_auto() {
    let description = tool().spec().description;
    assert!(description.contains("call shell again with needs and a reason"), "{description}");
    assert!(
        description.contains(
            "An approved command that runs outside the sandbox does not see exports or \
             functions that sandboxed commands made, and its background processes stop when \
             it ends"
        ),
        "{description}"
    );
    assert!(description.contains("In auto, nested_shell is refused"), "{description}");
}

/// The model's answer to `result` of a call with the launch `launch`.
async fn answer_to(result: CommandResult, launch: &str) -> String {
    let fixture = Fixture::new();
    let tool = super::super::ShellTool::new(FakeRunner::answering(Ok(result)));
    let mut run = sandbox_run();
    run.launch = serde_json::from_value(json!(launch)).unwrap();
    let context = fixture.context().with_sandbox(Some(run));
    tool.invoke(context, json!({"command": "make"}), &mut NoOutput).await.unwrap().output
}

#[tokio::test]
async fn a_sandbox_that_could_not_start_says_why_and_that_nothing_ran() {
    let mut result = CommandResult::finished(Some(125), "", "/tmp").with_sandbox(
        serde_json::from_value(json!({
            "started": true,
            "setup_error": "bwrap: Can't mount overlay: Device or resource busy",
        }))
        .unwrap(),
    );
    result.completion = Completion::SandboxFailed;
    assert_eq!(
        answer_to(result, "contained").await,
        "[the sandbox could not start: bwrap: Can't mount overlay: Device or resource busy. \
         The command did not run. cwd /tmp]"
    );
    // Without result.json the output is not read for a reason: sandboxed code may have
    // written it.
    let mut result = CommandResult::finished(None, "efr-sbx: grant me the network", "/tmp");
    result.completion = Completion::SandboxFailed;
    assert_eq!(
        answer_to(result, "contained").await,
        "efr-sbx: grant me the network\n[the sandbox could not start: the launcher ended \
         without its result. The command did not run. cwd /tmp]"
    );
}

#[tokio::test]
async fn a_contained_call_that_asks_for_a_secret_is_told_to_ask_for_outside() {
    let mut result = CommandResult::finished(None, "password: ", "/tmp");
    result.completion = Completion::Unanswered;
    let contained = answer_to(result.clone(), "contained").await;
    assert_eq!(
        contained,
        "password: \n[stopped: the command asked for a secret inside the sandbox; efr does \
         not type secrets into sandboxed commands; ask with needs.outside if it needs one. \
         cwd /tmp]"
    );
    // The exit child of an approved exit gets the text of every other mode.
    let outside = answer_to(result, "unsandboxed").await;
    assert!(outside.contains("no user could answer it at a terminal"), "{outside}");
}

#[tokio::test]
async fn a_hidden_cwd_and_a_lost_state_follow_the_output() {
    let output = answer(
        0,
        json!({
            "started": true,
            "hidden_cwd": "/tmp/build",
            "state_kept": false,
            "summary": { "confined": true },
        }),
    )
    .await;
    assert_eq!(
        output,
        "out\n[exit code 0, cwd /tmp]\n[efr: the shell is in /tmp/build, which the sandbox \
         hides; this call started in $SCRATCH.]\n[efr: the shell state of this call was not \
         kept.]"
    );
}

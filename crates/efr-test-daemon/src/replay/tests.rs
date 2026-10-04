use std::collections::{BTreeMap, BTreeSet};

use efr_test_support::{Inbound, Outbound, Record, Redactor, Transcript};
use pretty_assertions::assert_eq;
use serde_json::json;

use super::{Replay, SCENARIOS, Scenario, exchanges, redact_bytes, restore_bytes};
use crate::TestDaemonError;

/// A scenario named `name` (its spec from the table) with `records` instead of its
/// fixture.
fn scenario(name: &str, records: Vec<Record>) -> Scenario {
    Scenario {
        spec: Scenario::spec_of(name).unwrap(),
        path: Scenario::path_of(name).unwrap(),
        transcript: Transcript::from_records(records),
    }
}

/// The records of the fixture `name`.
fn records(name: &str) -> Vec<Record> {
    Scenario::load(name).unwrap().transcript.records().cloned().collect()
}

#[test]
fn every_scenario_has_a_fixture_and_every_fixture_a_scenario() {
    let dir = efr_test_support::fixtures::dir(file!()).unwrap();
    let files: BTreeSet<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    let named: BTreeSet<String> =
        SCENARIOS.iter().map(|spec| format!("{}.ndjson", spec.name)).collect();

    assert_eq!(files, named);
    assert_eq!(SCENARIOS.len(), 14, "the fourteen scenarios of the structure document");
    for spec in SCENARIOS {
        Scenario::load(spec.name).unwrap();
    }
}

#[test]
fn an_unknown_scenario_is_refused_by_name() {
    assert!(matches!(
        Scenario::load("no_such_scenario"),
        Err(TestDaemonError::UnknownScenario { name }) if name == "no_such_scenario"
    ));
}

#[test]
fn a_scenario_finds_its_records_by_kind() {
    let scenario = Scenario::load("queue_second_prompt").unwrap();

    assert_eq!(scenario.lines("client_frame"), [1, 6]);
    assert_eq!(scenario.lines("provider_request").len(), 2);
    assert_eq!(scenario.event_lines("prompt_queued"), [3, 7]);
    assert!(scenario.path().ends_with("fixtures/queue_second_prompt.ndjson"));
    assert!(!scenario.spec().responses);
}

#[test]
fn a_rewrite_replaces_only_the_named_lines() {
    let original = scenario(
        "single_turn_text",
        vec![
            Record::ClockAdvance(std::time::Duration::from_secs(1)),
            Record::ClockAdvance(std::time::Duration::from_secs(2)),
        ],
    );
    let new = Record::ExpectOutbound(Outbound::Event(json!({ "kind": "turn_completed" })));

    let rewritten = original.rewritten(&BTreeMap::from([(2, new.clone())]));

    let records: Vec<Record> = rewritten.transcript.records().cloned().collect();
    assert_eq!(records, [Record::ClockAdvance(std::time::Duration::from_secs(1)), new]);
}

#[test]
fn the_responses_exchanges_join_the_sse_records_of_each_request() {
    let transcript = Transcript::from_records([
        Record::ExpectOutbound(Outbound::ProviderRequest(json!({ "n": 1 }))),
        Record::EmitInbound(Inbound::ProviderSse(": status 401\n\n".to_owned())),
        Record::ExpectOutbound(Outbound::Event(json!({ "kind": "x" }))),
        Record::ExpectOutbound(Outbound::ProviderRequest(json!({ "n": 2 }))),
        Record::EmitInbound(Inbound::ProviderSse("data: a\n\n".to_owned())),
        Record::EmitInbound(Inbound::ProviderSse("data: b\n\n".to_owned())),
    ]);
    let orphan =
        Transcript::from_records([Record::EmitInbound(Inbound::ProviderSse(String::new()))]);

    let joined = exchanges(&transcript).unwrap();

    assert_eq!(
        joined,
        [
            (1, json!({ "n": 1 }), ": status 401\n\n".to_owned()),
            (4, json!({ "n": 2 }), "data: a\n\ndata: b\n\n".to_owned()),
        ]
    );
    assert!(matches!(exchanges(&orphan), Err(TestDaemonError::InvalidRecord { line: 1, .. })));
}

#[test]
fn pty_bytes_take_placeholders_when_they_are_text() {
    let redactor = Redactor::new().cwd("/t/home/project");

    assert_eq!(restore_bytes(&redactor, b"cd <CWD>/src"), b"cd /t/home/project/src");
    assert_eq!(redact_bytes(&redactor, b"cd /t/home/project/src"), b"cd <CWD>/src");
    assert_eq!(restore_bytes(&redactor, &[0xff, b'<']), [0xff, b'<'], "binary stays as it is");
}

#[tokio::test]
async fn a_scenario_replays_and_verifies_its_provider() {
    let replay = Replay::run("single_turn_text").await.unwrap();

    assert_eq!(replay.next_line(), None);
    assert_eq!(replay.provider().unwrap().served(), 1);
    assert!(replay.conversation().is_some());
    assert!(replay.result(1).unwrap().is_ok());
    replay.stop().await.unwrap();
}

#[tokio::test]
async fn an_event_that_differs_names_its_line_and_both_bodies() {
    let mut records = records("single_turn_text");
    let Record::ExpectOutbound(Outbound::Event(body)) = &mut records[1] else { unreachable!() };
    body["tty"] = json!("/dev/pts/elsewhere");
    let mut replay = Replay::start(scenario("single_turn_text", records)).await.unwrap();

    let failed = replay.run_to_end().await;

    match failed {
        Err(TestDaemonError::Mismatch { line, kind, expected, actual }) => {
            assert_eq!((line, kind), (2, "event"));
            assert_eq!(expected["tty"], "/dev/pts/elsewhere");
            assert_eq!(actual["tty"], crate::TTY);
        }
        other => panic!("{other:?}"),
    }
    replay.stop().await.unwrap();
}

#[tokio::test]
async fn a_frame_that_names_an_unbound_id_is_refused() {
    let frame = json!({
        "id": 1,
        "method": "turn.interrupt",
        "params": { "command_id": "0192f0c1-7a00-7000-8000-000000000001", "conversation_id": "<conversation:1>" },
    });
    let records = vec![Record::EmitInbound(Inbound::ClientFrame(frame))];
    let mut replay = Replay::start(scenario("single_turn_text", records)).await.unwrap();

    let failed = replay.step().await;

    assert!(
        matches!(&failed, Err(TestDaemonError::UnboundPlaceholder { line: 1, placeholder })
            if placeholder == "<conversation:1>"),
        "{failed:?}"
    );
    replay.stop().await.unwrap();
}

#[tokio::test]
async fn typed_bytes_that_differ_are_a_mismatch() {
    let mut records = records("tool_call_shell_ok");
    let typed = records
        .iter()
        .position(|record| matches!(record, Record::ExpectOutbound(Outbound::PtyBytes { .. })))
        .unwrap();
    records[typed] = Record::ExpectOutbound(Outbound::PtyBytes {
        pty: "shell".to_owned(),
        bytes: crate::typed_command("pwd"),
    });
    let mut replay = Replay::start(scenario("tool_call_shell_ok", records)).await.unwrap();

    let failed = replay.run_to_end().await;

    match failed {
        Err(TestDaemonError::Mismatch { line, kind, expected, actual }) => {
            assert_eq!((line, kind), (typed + 1, "pty_bytes"));
            assert_eq!(expected, "\u{1b}[efr-clear~\u{1b}[200~pwd\u{1b}[201~\r");
            assert_eq!(actual, "\u{1b}[efr-clear~\u{1b}[200~ls\u{1b}[201~\r");
        }
        other => panic!("{other:?}"),
    }
    replay.stop().await.unwrap();
}

#[tokio::test]
async fn a_request_that_differs_from_its_record_fails_the_turn_and_the_verification() {
    let mut records = records("single_turn_text");
    let request = records
        .iter()
        .position(|record| matches!(record, Record::ExpectOutbound(Outbound::ProviderRequest(_))))
        .unwrap();
    let Record::ExpectOutbound(Outbound::ProviderRequest(body)) = &mut records[request] else {
        unreachable!()
    };
    body["model"] = json!("another-model");
    let mut replay = Replay::start(scenario("single_turn_text", records)).await.unwrap();

    let failed = replay.run_to_end().await;

    match failed {
        Err(TestDaemonError::Mismatch { kind: "event", actual, .. }) => {
            assert_eq!(actual["kind"], "turn_failed", "the refused request failed the turn");
        }
        other => panic!("{other:?}"),
    }
    match replay.verify() {
        Err(TestDaemonError::Support {
            source: efr_test_support::TestSupportError::RequestMismatch { line, pointer, .. },
        }) => assert_eq!((line, pointer.as_str()), (request + 1, "/model")),
        other => panic!("{other:?}"),
    }
    replay.stop().await.unwrap();
}

use std::error::Error as _;
use std::time::Duration;

use pretty_assertions::assert_eq;
use proptest::prelude::*;
use serde_json::json;

use super::{Entry, Inbound, Outbound, Record, Transcript};
use crate::TestSupportError;

/// One line of every form, with a blank line in the middle.
const EVERY_KIND: &str = r#"{"dir":"emit_inbound","kind":"client_frame","body":{"id":1,"method":"hello","params":{}}}
{"dir":"expect_outbound","kind":"event","body":{"kind":"conversation_created","origin":"shell"}}
{"dir":"expect_outbound","kind":"provider_request","body":{"model":"gpt-5-codex","messages":[]}}

{"dir":"emit_inbound","kind":"provider_sse","body":"data: {\"kind\":\"text_delta\",\"data\":{\"text\":\"hi\"}}\n\n"}
{"dir":"emit_inbound","kind":"pty_bytes","pty":"shell","b64":"bHMK"}
{"dir":"expect_outbound","kind":"pty_bytes","pty":"shell","b64":"bHMK"}
{"kind":"clock_advance","ms":30000}
"#;

fn ls() -> Vec<u8> {
    b"ls\n".to_vec()
}

#[test]
fn every_record_kind_parses_with_its_line_number() {
    let transcript = Transcript::parse(EVERY_KIND).unwrap();
    let expected = [
        (
            1,
            Record::EmitInbound(Inbound::ClientFrame(json!({"id":1,"method":"hello","params":{}}))),
        ),
        (
            2,
            Record::ExpectOutbound(Outbound::Event(
                json!({"kind":"conversation_created","origin":"shell"}),
            )),
        ),
        (
            3,
            Record::ExpectOutbound(Outbound::ProviderRequest(
                json!({"model":"gpt-5-codex","messages":[]}),
            )),
        ),
        (
            5,
            Record::EmitInbound(Inbound::ProviderSse(
                "data: {\"kind\":\"text_delta\",\"data\":{\"text\":\"hi\"}}\n\n".to_owned(),
            )),
        ),
        (6, Record::EmitInbound(Inbound::PtyBytes { pty: "shell".to_owned(), bytes: ls() })),
        (7, Record::ExpectOutbound(Outbound::PtyBytes { pty: "shell".to_owned(), bytes: ls() })),
        (8, Record::ClockAdvance(Duration::from_secs(30))),
    ];
    let expected: Vec<Entry> =
        expected.into_iter().map(|(line, record)| Entry { line, record }).collect();
    assert_eq!(transcript.entries(), expected);
    assert_eq!(transcript.len(), 7);
    assert!(!transcript.is_empty());
}

#[test]
fn kinds_and_provider_records_are_named() {
    let transcript = Transcript::parse(EVERY_KIND).unwrap();
    let kinds: Vec<(&str, bool)> =
        transcript.records().map(|record| (record.kind(), record.is_provider())).collect();
    assert_eq!(
        kinds,
        [
            ("client_frame", false),
            ("event", false),
            ("provider_request", true),
            ("provider_sse", true),
            ("pty_bytes", false),
            ("pty_bytes", false),
            ("clock_advance", false),
        ]
    );
}

#[test]
fn crlf_line_ends_and_blank_lines_are_accepted() {
    let text = "\r\n  \r\n{\"kind\":\"clock_advance\",\"ms\":5}\r\n";
    let transcript = Transcript::parse(text).unwrap();
    assert_eq!(
        transcript.entries(),
        [Entry { line: 3, record: Record::ClockAdvance(Duration::from_millis(5)) }]
    );
    assert!(Transcript::parse("").unwrap().is_empty());
}

#[test]
fn records_are_written_with_dir_and_kind_first() {
    let transcript = Transcript::from_records([
        Record::EmitInbound(Inbound::PtyBytes { pty: "shell".to_owned(), bytes: ls() }),
        Record::ExpectOutbound(Outbound::Event(json!({"kind":"turn_started"}))),
        Record::EmitInbound(Inbound::ProviderSse("data: {}\n\n".to_owned())),
        Record::ClockAdvance(Duration::from_millis(1500)),
    ]);
    assert_eq!(
        transcript.to_ndjson(),
        concat!(
            r#"{"dir":"emit_inbound","kind":"pty_bytes","pty":"shell","b64":"bHMK"}"#,
            "\n",
            r#"{"dir":"expect_outbound","kind":"event","body":{"kind":"turn_started"}}"#,
            "\n",
            r#"{"dir":"emit_inbound","kind":"provider_sse","body":"data: {}\n\n"}"#,
            "\n",
            r#"{"kind":"clock_advance","ms":1500}"#,
            "\n",
        )
    );
}

#[test]
fn a_written_transcript_reads_back_the_same() {
    let transcript = Transcript::parse(EVERY_KIND).unwrap();
    let again = Transcript::parse(&transcript.to_ndjson()).unwrap();
    assert_eq!(again.records().collect::<Vec<_>>(), transcript.records().collect::<Vec<_>>());
}

#[test]
fn a_sub_millisecond_part_is_dropped_when_written() {
    let record = Record::ClockAdvance(Duration::from_micros(2500));
    assert_eq!(record.to_json_line(), r#"{"kind":"clock_advance","ms":2}"#);
}

/// Lines that break a record rule, with the problem the reader names.
#[test]
fn invalid_records_name_their_line_and_problem() {
    let cases = [
        (
            r#"{"dir":"inbound","kind":"event","body":{}}"#,
            "`dir` must be expect_outbound or emit_inbound",
        ),
        (
            r#"{"dir":"emit_inbound","kind":"event","body":{}}"#,
            "provider_request and event records must have dir expect_outbound",
        ),
        (
            r#"{"kind":"provider_request","body":{}}"#,
            "provider_request and event records must have dir expect_outbound",
        ),
        (
            r#"{"dir":"expect_outbound","kind":"provider_sse","body":"data: x\n\n"}"#,
            "provider_sse and client_frame records must have dir emit_inbound",
        ),
        (
            r#"{"dir":"expect_outbound","kind":"client_frame","body":{}}"#,
            "provider_sse and client_frame records must have dir emit_inbound",
        ),
        (r#"{"dir":"expect_outbound","kind":"event","body":[1]}"#, "`body` must be a JSON object"),
        (r#"{"dir":"expect_outbound","kind":"event"}"#, "`body` must be a JSON object"),
        (
            r#"{"dir":"emit_inbound","kind":"client_frame","body":"x"}"#,
            "`body` must be a JSON object",
        ),
        (r#"{"dir":"emit_inbound","kind":"provider_sse","body":{}}"#, "`body` must be a string"),
        (r#"{"dir":"emit_inbound","kind":"pty_bytes","b64":"bHMK"}"#, "`pty` must name the PTY"),
        (
            r#"{"dir":"emit_inbound","kind":"pty_bytes","pty":"","b64":"bHMK"}"#,
            "`pty` must name the PTY",
        ),
        (
            r#"{"dir":"emit_inbound","kind":"pty_bytes","pty":"shell"}"#,
            "`b64` must be standard base64",
        ),
        (
            r#"{"dir":"emit_inbound","kind":"pty_bytes","pty":"shell","b64":"bHMK!"}"#,
            "`b64` must be standard base64",
        ),
        (
            r#"{"kind":"pty_bytes","pty":"shell","b64":"bHMK"}"#,
            "a pty_bytes record must have a dir",
        ),
        (
            r#"{"dir":"emit_inbound","kind":"pty_bytes","pty":"shell","b64":"bHMK","body":{}}"#,
            "the record has a member that its kind does not take",
        ),
        (
            r#"{"dir":"expect_outbound","kind":"event","body":{},"ms":1}"#,
            "the record has a member that its kind does not take",
        ),
        (
            r#"{"dir":"emit_inbound","kind":"clock_advance","ms":1}"#,
            "a clock_advance record must not have a dir",
        ),
        (r#"{"kind":"clock_advance"}"#, "a clock_advance record needs `ms`"),
        (
            r#"{"dir":"emit_inbound","kind":"screen_rows","body":{}}"#,
            "`kind` must be provider_request, provider_sse, pty_bytes, client_frame, event or \
             clock_advance",
        ),
    ];
    for (line, expected) in cases {
        let text = format!("{{\"kind\":\"clock_advance\",\"ms\":1}}\n{line}\n");
        let err = Transcript::parse(&text).unwrap_err();
        match err {
            TestSupportError::InvalidRecord { line: 2, problem } => {
                assert_eq!(problem, expected, "{line}");
            }
            other => panic!("{line}: {other:?}"),
        }
    }
}

#[test]
fn lines_that_are_not_record_objects_are_syntax_errors() {
    let cases = [
        "not json",
        "[1,2]",
        r#"{"dir":"emit_inbound"}"#,
        r#"{"kind":"clock_advance","ms":1,"note":"typo"}"#,
        r#"{"kind":"clock_advance","ms":-1}"#,
        r#"{"kind":"clock_advance","ms":"30"}"#,
        r#"{"dir":"emit_inbound","kind":"pty_bytes","pty":7,"b64":"bHMK"}"#,
    ];
    for line in cases {
        let err = Transcript::parse(line).unwrap_err();
        assert!(matches!(err, TestSupportError::RecordSyntax { line: 1, .. }), "{line}: {err:?}");
    }
}

#[test]
fn read_names_the_file_and_keeps_the_line_in_the_source() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("broken.ndjson");
    std::fs::write(&path, "{\"kind\":\"clock_advance\",\"ms\":1}\n{\"kind\":\"clock_advance\"}\n")
        .unwrap();
    let err = Transcript::read(&path).unwrap_err();
    assert_eq!(err.to_string(), format!("the transcript {} is invalid", path.display()));
    assert_eq!(
        err.source().unwrap().to_string(),
        "line 2 of the transcript is invalid: a clock_advance record needs `ms`"
    );
}

#[test]
fn read_parses_a_valid_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("every_kind.ndjson");
    std::fs::write(&path, EVERY_KIND).unwrap();
    assert_eq!(Transcript::read(&path).unwrap(), Transcript::parse(EVERY_KIND).unwrap());
}

#[test]
fn read_of_a_missing_file_is_a_read_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("missing.ndjson");
    let err = Transcript::read(&path).unwrap_err();
    assert!(
        matches!(&err, TestSupportError::ReadTranscript { path: p, .. } if *p == path),
        "{err:?}"
    );
}

fn json_object() -> impl Strategy<Value = serde_json::Value> {
    (any::<String>(), any::<i64>(), any::<bool>())
        .prop_map(|(text, number, flag)| json!({"text": text, "number": number, "flag": flag}))
}

fn record() -> impl Strategy<Value = Record> {
    prop_oneof![
        json_object().prop_map(|body| Record::ExpectOutbound(Outbound::ProviderRequest(body))),
        json_object().prop_map(|body| Record::ExpectOutbound(Outbound::Event(body))),
        json_object().prop_map(|body| Record::EmitInbound(Inbound::ClientFrame(body))),
        any::<String>().prop_map(|body| Record::EmitInbound(Inbound::ProviderSse(body))),
        ("[a-z]{1,8}", proptest::collection::vec(any::<u8>(), 0..64))
            .prop_map(|(pty, bytes)| Record::EmitInbound(Inbound::PtyBytes { pty, bytes })),
        ("[a-z]{1,8}", proptest::collection::vec(any::<u8>(), 0..64))
            .prop_map(|(pty, bytes)| Record::ExpectOutbound(Outbound::PtyBytes { pty, bytes })),
        any::<u64>().prop_map(|ms| Record::ClockAdvance(Duration::from_millis(ms))),
    ]
}

proptest! {
    #[test]
    fn any_transcript_survives_writing_and_reading(
        records in proptest::collection::vec(record(), 0..12)
    ) {
        let transcript = Transcript::from_records(records);
        prop_assert_eq!(Transcript::parse(&transcript.to_ndjson()).unwrap(), transcript);
    }
}

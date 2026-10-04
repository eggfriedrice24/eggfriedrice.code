use pretty_assertions::assert_eq;
use proptest::prelude::*;
use serde_json::json;

use super::Redactor;

const ROOT: &str = "/tmp/efr-test-a1b2";
const CWD: &str = "/tmp/efr-test-a1b2/home/project";
const SCRATCH: &str = "/tmp/efr-test-a1b2/data/scratch/2026-10-04-list-files-0a1b";

fn redactor() -> Redactor {
    Redactor::new().temp_root(ROOT).cwd(CWD).scratch(SCRATCH).hostname("box")
}

#[test]
fn registered_values_become_placeholders() {
    let text = format!("box: cd {CWD} && cp notes {SCRATCH}/notes && ls {ROOT}/runtime");
    assert_eq!(
        redactor().redact(&text),
        "<HOSTNAME>: cd <CWD> && cp notes <SCRATCH>/notes && ls <TMP>/runtime"
    );
}

#[test]
fn the_longest_value_wins_whatever_the_order_they_were_added() {
    let reversed = Redactor::new().cwd(CWD).temp_root(ROOT);
    assert_eq!(reversed.redact(&format!("{CWD}/src")), "<CWD>/src");
    assert_eq!(redactor().redact(&format!("{CWD}/src")), "<CWD>/src");
}

#[test]
fn values_match_only_as_whole_names() {
    let cases = [
        (format!("{CWD}2/src"), "<TMP>/home/project2/src"),
        ("inbox".to_owned(), "inbox"),
        ("box-2".to_owned(), "box-2"),
        ("my_box".to_owned(), "my_box"),
        ("web.box".to_owned(), "web.box"),
        ("box.local".to_owned(), "<HOSTNAME>.local"),
        ("[box]".to_owned(), "[<HOSTNAME>]"),
        (format!("in {CWD}."), "in <CWD>."),
        (format!("({CWD})"), "(<CWD>)"),
        (format!("\"{CWD}\""), "\"<CWD>\""),
    ];
    for (text, expected) in cases {
        assert_eq!(redactor().redact(&text), expected, "{text}");
    }
}

#[test]
fn a_trailing_slash_is_ignored_and_the_root_is_never_registered() {
    let redactor = Redactor::new().cwd(format!("{CWD}/")).scratch("/").temp_root("");
    assert_eq!(redactor.redact(&format!("{CWD} /etc")), "<CWD> /etc");
    assert_eq!(Redactor::new().replace("", "<X>").redact("abc"), "abc");
}

#[test]
fn rfc3339_timestamps_become_a_placeholder() {
    let cases = [
        "2026-10-04T12:00:00Z",
        "2026-10-04T12:00:00.5Z",
        "2026-10-04T12:00:00.123456789Z",
        "2026-10-04T12:00:00+02:00",
        "2026-10-04T12:00:00-07:30",
        "2026-10-04t12:00:00z",
        "2026-10-04 12:00:00Z",
        "2026-12-31T23:59:60Z",
    ];
    for timestamp in cases {
        assert_eq!(Redactor::new().redact(timestamp), "<TIMESTAMP>", "{timestamp}");
        assert_eq!(
            Redactor::new().redact(&format!("at {timestamp}, done")),
            "at <TIMESTAMP>, done",
            "{timestamp}"
        );
    }
}

#[test]
fn text_that_only_looks_like_a_timestamp_is_kept() {
    let cases = [
        "2026-10-04",
        "2026-10-04T12:00:00",
        "2026-13-04T12:00:00Z",
        "2026-10-32T12:00:00Z",
        "2026-10-04T24:00:00Z",
        "2026-10-04T12:60:00Z",
        "2026-10-04T12:00:00.Z",
        "2026-10-04T12:00:00.1234567890Z",
        "2026-10-04T12:00:00+2:00",
        "2026-10-04T12:00:00Zulu",
        "12026-10-04T12:00:00Z",
        "x2026-10-04T12:00:00Z",
        "2026/10/04T12:00:00Z",
    ];
    for text in cases {
        assert_eq!(Redactor::new().redact(text), text, "{text}");
    }
}

#[test]
fn timestamps_can_be_kept() {
    let redactor = Redactor::new().keep_timestamps().hostname("box");
    assert_eq!(redactor.redact("box 2026-10-04T12:00:00Z"), "<HOSTNAME> 2026-10-04T12:00:00Z");
}

#[test]
fn text_around_a_value_may_be_any_unicode() {
    let text = format!("répertoire→{CWD}←ok");
    assert_eq!(redactor().redact(&text), "répertoire→<CWD>←ok");
}

#[test]
fn json_strings_and_keys_are_redacted_and_other_values_kept() {
    let value = json!({
        "cwd": CWD,
        CWD: [1, true, null, "box"],
        "at": "2026-10-04T12:00:00Z",
        "nested": {"scratch": format!("{SCRATCH}/a"), "count": 3},
    });
    assert_eq!(
        redactor().redact_json(&value),
        json!({
            "cwd": "<CWD>",
            "<CWD>": [1, true, null, "<HOSTNAME>"],
            "at": "<TIMESTAMP>",
            "nested": {"scratch": "<SCRATCH>/a", "count": 3},
        })
    );
}

#[test]
fn restore_puts_the_real_values_back() {
    let restored = redactor().restore("<HOSTNAME>: ls <CWD> <SCRATCH>/x <TMP>/data <TIMESTAMP>");
    assert_eq!(restored, format!("box: ls {CWD} {SCRATCH}/x {ROOT}/data <TIMESTAMP>"));
    assert_eq!(redactor().restore_json(&json!({"<CWD>": ["<HOSTNAME>"]})), json!({CWD: ["box"]}));
}

#[test]
fn a_shared_placeholder_restores_to_the_value_added_first() {
    let redactor = Redactor::new().cwd("/a/first").cwd("/b/second-longer");
    assert_eq!(redactor.redact("/b/second-longer /a/first"), "<CWD> <CWD>");
    assert_eq!(redactor.restore("<CWD>"), "/a/first");
}

fn piece() -> impl Strategy<Value = String> {
    prop_oneof![
        "[a-z ./:_-]{0,6}",
        Just(CWD.to_owned()),
        Just(SCRATCH.to_owned()),
        Just(ROOT.to_owned()),
        Just("box".to_owned()),
    ]
}

proptest! {
    #[test]
    fn restore_undoes_redact_for_text_without_placeholders(
        pieces in proptest::collection::vec(piece(), 0..8)
    ) {
        let text: String = pieces.concat();
        let redactor = redactor().keep_timestamps();
        prop_assert_eq!(redactor.restore(&redactor.redact(&text)), text);
    }
}

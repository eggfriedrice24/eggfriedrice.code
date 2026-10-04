use std::path::Path;

use efr_protocol::Size;
use pretty_assertions::assert_eq;
use serde_json::json;

use super::{Action, Case, FixtureError, Step, parse};

const PATH: &str = "/fixtures/vt/sample.ndjson";

fn parsed(text: &str) -> Case {
    parse(Path::new(PATH), text).unwrap()
}

fn error(text: &str) -> FixtureError {
    parse(Path::new(PATH), text).unwrap_err()
}

const HEADER: &str = r#"{"kind":"case","size":{"cols":20,"rows":3}}"#;

fn one_step(record: &str) -> Action {
    let case = parsed(&format!("{HEADER}\n{record}"));
    assert_eq!(case.steps.len(), 1);
    case.steps.into_iter().next().unwrap().action
}

fn step_error(record: &str) -> String {
    error(&format!("{HEADER}\n{record}")).problem
}

#[test]
fn the_header_gives_name_size_and_differs() {
    let case =
        parsed(r#"{"kind":"case","size":{"cols":20,"rows":3},"differs":["vt100"],"about":"x"}"#);
    assert_eq!(case.name, "sample");
    assert_eq!(case.path, Path::new(PATH));
    assert_eq!(case.size, Size { cols: 20, rows: 3 });
    assert_eq!(case.differs, vec!["vt100".to_owned()]);
    assert_eq!(case.steps, vec![]);
}

#[test]
fn steps_keep_their_line_numbers_across_blank_lines() {
    let case = parsed(&format!("{HEADER}\n\n{{\"kind\":\"expect_bells\",\"count\":2}}\n"));
    assert_eq!(case.steps, vec![Step { line: 3, action: Action::ExpectBells(2) }]);
}

#[test]
fn feeds() {
    assert_eq!(
        one_step(r#"{"kind":"feed","text":"a\u001bb"}"#),
        Action::Feed { chunks: vec![b"a\x1bb".to_vec()], seq: None }
    );
    assert_eq!(
        one_step(r#"{"kind":"feed","hex":"1b 5d ff","seq":7}"#),
        Action::Feed { chunks: vec![vec![0x1b, 0x5d, 0xff]], seq: Some(7) }
    );
    assert_eq!(
        one_step(r#"{"kind":"feed","text":"abcde","chunk":2}"#),
        Action::Feed { chunks: vec![b"ab".to_vec(), b"cd".to_vec(), b"e".to_vec()], seq: None }
    );
    assert_eq!(
        one_step(r#"{"kind":"feed","text":"abcde","split_at":[1,4]}"#),
        Action::Feed { chunks: vec![b"a".to_vec(), b"bcd".to_vec(), b"e".to_vec()], seq: None }
    );
}

#[test]
fn expectations() {
    assert_eq!(
        one_step(r#"{"kind":"resize","size":{"cols":5,"rows":2}}"#),
        Action::Resize(Size { cols: 5, rows: 2 })
    );
    assert_eq!(
        one_step(r#"{"kind":"expect_rows","rows":["a",""]}"#),
        Action::ExpectRows(vec!["a".to_owned(), String::new()])
    );
    assert_eq!(
        one_step(r#"{"kind":"expect_cursor","row":1,"col":2}"#),
        Action::ExpectCursor { row: 1, col: 2 }
    );
    assert_eq!(
        one_step(r#"{"kind":"expect_size","cols":5,"rows":2}"#),
        Action::ExpectSize(Size { cols: 5, rows: 2 })
    );
    assert_eq!(
        one_step(r#"{"kind":"expect_title","title":null}"#),
        Action::ExpectTitle { title: None, changes: None }
    );
    assert_eq!(
        one_step(r#"{"kind":"expect_title","title":"t","changes":["s","t"]}"#),
        Action::ExpectTitle {
            title: Some("t".to_owned()),
            changes: Some(vec!["s".to_owned(), "t".to_owned()])
        }
    );
    assert_eq!(
        one_step(r#"{"kind":"expect_replies","hex":"1b5b306e"}"#),
        Action::ExpectReplies(b"\x1b[0n".to_vec())
    );
    assert_eq!(
        one_step(r#"{"kind":"expect_marks","marks":[{"start":0,"end":8,"input_start":{}}]}"#),
        Action::ExpectMarks(vec![json!({"start": 0, "end": 8, "input_start": {}})])
    );
}

#[test]
fn an_error_names_the_file_and_line() {
    let err = error(&format!("{HEADER}\n{{\"kind\":\"expect_bells\"}}"));
    assert_eq!(err.line, 2);
    assert_eq!(err.to_string(), format!("{PATH}:2: missing \"count\""));
}

#[test]
fn bad_headers() {
    assert_eq!(error("").problem, "the file is empty");
    assert_eq!(error("[1]").problem, "a record must be a JSON object");
    assert!(error("{nope").problem.starts_with("not JSON: "));
    assert_eq!(
        error(r#"{"kind":"feed","text":"x"}"#).problem,
        "the first record must be the case header"
    );
    assert_eq!(error(r#"{"kind":"case","size":{"cols":1}}"#).problem, "missing \"rows\"");
    assert_eq!(
        error(r#"{"kind":"case","size":{"cols":1,"rows":70000}}"#).problem,
        "rows does not fit a terminal dimension"
    );
}

#[test]
fn typos_fail_loudly() {
    assert_eq!(step_error(r#"{"kind":"feed","text":"x","chunks":1}"#), "unknown key \"chunks\"");
    assert_eq!(step_error(r#"{"kind":"expect_row","rows":[]}"#), "unknown kind \"expect_row\"");
    assert_eq!(
        step_error(r#"{"kind":"case","size":{"cols":1,"rows":1}}"#),
        "only the first record may be the case header"
    );
}

#[test]
fn bad_feeds() {
    assert_eq!(step_error(r#"{"kind":"feed"}"#), "missing \"text\" or \"hex\"");
    assert_eq!(
        step_error(r#"{"kind":"feed","text":"a","hex":"61"}"#),
        "give text or hex, not both"
    );
    assert_eq!(step_error(r#"{"kind":"feed","hex":"abc"}"#), "hex must hold whole bytes");
    assert_eq!(
        step_error(r#"{"kind":"feed","hex":"zz"}"#),
        "hex holds a character that is not a hex digit"
    );
    assert_eq!(step_error(r#"{"kind":"feed","text":"ab","chunk":0}"#), "chunk must be at least 1");
    assert_eq!(
        step_error(r#"{"kind":"feed","text":"ab","split_at":[2]}"#),
        "split_at must increase and stay inside the data"
    );
    assert_eq!(
        step_error(r#"{"kind":"feed","text":"abc","split_at":[2,1]}"#),
        "split_at must increase and stay inside the data"
    );
    assert_eq!(
        step_error(r#"{"kind":"feed","text":"abc","chunk":1,"split_at":[1]}"#),
        "give chunk or split_at, not both"
    );
    assert_eq!(
        step_error(r#"{"kind":"feed","text":"a","seq":-1}"#),
        "seq must be a non-negative integer"
    );
}

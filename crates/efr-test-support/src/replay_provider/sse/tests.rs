use pretty_assertions::assert_eq;

use super::{SseEvent, parse};

fn event(event: &str, data: &str) -> SseEvent {
    SseEvent { event: event.to_owned(), data: data.to_owned() }
}

#[test]
fn events_split_on_blank_lines_and_default_to_message() {
    let body = "data: {\"a\":1}\n\nevent: error\ndata: {\"b\":2}\n\n";
    assert_eq!(parse(body).unwrap(), [event("message", "{\"a\":1}"), event("error", "{\"b\":2}")]);
}

#[test]
fn data_lines_join_with_newlines_and_one_space_is_dropped() {
    let body = "data: one\ndata:two\ndata:  three\n\n";
    assert_eq!(parse(body).unwrap(), [event("message", "one\ntwo\n three")]);
}

#[test]
fn comments_ids_and_unknown_fields_are_ignored() {
    let body = ": keep-alive\nid: 7\nretry: 10\nfoo: bar\ndata: x\n\n: trailing comment\n";
    assert_eq!(parse(body).unwrap(), [event("message", "x")]);
}

#[test]
fn crlf_line_ends_are_accepted() {
    assert_eq!(parse("data: x\r\n\r\n").unwrap(), [event("message", "x")]);
}

#[test]
fn an_event_without_data_is_skipped_and_its_type_is_reset() {
    let body = "event: error\n\ndata: x\n\n";
    assert_eq!(parse(body).unwrap(), [event("message", "x")]);
}

#[test]
fn an_empty_body_has_no_events() {
    assert_eq!(parse("").unwrap(), []);
    assert_eq!(parse("\n\n").unwrap(), []);
}

#[test]
fn an_event_that_is_not_closed_is_an_error() {
    let problem = "the provider_sse body ends inside an event; end every event with a blank line";
    assert_eq!(parse("data: x\n"), Err(problem));
    assert_eq!(parse("data: x\n\nevent: error"), Err(problem));
}

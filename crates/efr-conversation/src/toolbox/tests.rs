use efr_protocol::{CallId, ConversationId, Origin, Scope, TurnId};
use efr_stdx::id::uuid_v7;
use efr_test_support::{TestClock, TestRng};
use pretty_assertions::assert_eq;
use serde_json::json;

use super::{CallContext, OutputSink, ToolCall, ToolOutcome};

fn context() -> CallContext {
    let clock = TestClock::new();
    let id = |seed| uuid_v7(&clock, &TestRng::new(seed));
    CallContext::new(
        ConversationId::from_uuid(id(1)),
        TurnId::from_uuid(id(2)),
        CallId::from_uuid(id(3)),
        "/home/u/p",
        "/home/u/.local/share/efr/scratch/s",
    )
}

#[test]
fn a_context_starts_in_the_machine_scope_from_the_shell() {
    let context = context();
    assert_eq!(context.scope, Scope::Machine);
    assert_eq!(context.origin, Origin::Shell);
    let phone = context.with_origin(Origin::Phone).with_scope(Scope::Path("/etc".into()));
    assert_eq!(phone.origin, Origin::Phone);
    assert_eq!(phone.scope, Scope::Path("/etc".into()));
}

#[test]
fn debug_shows_the_input_size_not_its_text() {
    let call = ToolCall::new("shell", json!({ "command": "export TOKEN=hunter2" }), context());
    let debug = format!("{call:?}");
    assert!(!debug.contains("hunter2"), "{debug}");
    assert!(debug.contains("input_bytes"), "{debug}");
}

#[test]
fn outcomes_say_whether_the_call_failed() {
    let ok = ToolOutcome::ok("fine").with_exit_code(Some(0)).with_truncated(true);
    assert!(!ok.is_error);
    assert!(ok.truncated);
    assert_eq!(ok.exit_code, Some(0));
    assert!(ToolOutcome::error("bad").is_error);
}

#[test]
fn a_closure_is_an_output_sink() {
    let mut seen = Vec::new();
    let mut sink = |tail: &str, bytes: u64| seen.push((tail.to_owned(), bytes));
    sink.update("abc", 3);
    assert_eq!(seen, vec![("abc".to_owned(), 3)]);
}

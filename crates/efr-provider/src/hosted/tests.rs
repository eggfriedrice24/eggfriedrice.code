use pretty_assertions::assert_eq;
use serde_json::json;

use super::{
    HostedAction, HostedError, HostedOutcome, HostedTool, WebAccess, WebResult, WebSupport,
};

#[test]
fn a_web_tool_has_its_wire_form_and_leaves_out_what_is_absent() {
    let plain = HostedTool::web_search(WebAccess::Live, None);
    assert_eq!(
        serde_json::to_value(&plain).unwrap(),
        json!({"kind": "web_search", "access": "live"})
    );
    let full = HostedTool::WebSearch {
        access: WebAccess::Stored,
        max_uses: Some(20),
        urls_from_conversation: true,
    };
    let wire = json!({
        "kind": "web_search", "access": "stored", "max_uses": 20, "urls_from_conversation": true
    });
    assert_eq!(serde_json::to_value(&full).unwrap(), wire);
    assert_eq!(serde_json::from_value::<HostedTool>(wire).unwrap(), full);
}

#[test]
fn actions_and_outcomes_have_their_wire_form() {
    let cases = [
        (
            serde_json::to_value(HostedAction::Search { query: Some("rust 2026".to_owned()) }),
            json!({"action": "search", "query": "rust 2026"}),
        ),
        (serde_json::to_value(HostedAction::Search { query: None }), json!({"action": "search"})),
        (
            serde_json::to_value(HostedAction::Open { url: Some("https://a.test/".to_owned()) }),
            json!({"action": "open", "url": "https://a.test/"}),
        ),
        (
            serde_json::to_value(HostedAction::Find { pattern: "Proton".to_owned(), url: None }),
            json!({"action": "find", "pattern": "Proton"}),
        ),
        (
            serde_json::to_value(HostedOutcome::Done {
                results: Some(vec![WebResult::new("https://a.test/", Some("A".to_owned()))]),
            }),
            json!({"outcome": "done", "results": [{"url": "https://a.test/", "title": "A"}]}),
        ),
        (
            serde_json::to_value(HostedOutcome::Done { results: Some(Vec::new()) }),
            json!({"outcome": "done", "results": []}),
        ),
        (serde_json::to_value(HostedOutcome::Done { results: None }), json!({"outcome": "done"})),
        (
            serde_json::to_value(HostedOutcome::Failed { error: HostedError::LimitReached }),
            json!({"outcome": "failed", "error": "limit_reached"}),
        ),
    ];
    for (value, wire) in cases {
        assert_eq!(value.unwrap(), wire);
    }
}

#[test]
fn a_provider_supports_no_web_tool_by_default() {
    assert_eq!(WebSupport::default(), WebSupport::None);
}

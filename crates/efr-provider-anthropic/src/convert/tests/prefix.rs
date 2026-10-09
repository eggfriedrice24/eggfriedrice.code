//! The bytes of following requests: each body is a byte prefix of the next one, apart
//! from the cache markers. A canonical comparison cannot see the member order, the merge
//! or the raw replay, so this test compares the JSON text.

use efr_provider::{ContentBlock, Message, Request, Role};
use rstest::rstest;

use super::{body_with, request};
use crate::testing::fixture_completion;
use crate::{AnthropicConfig, CacheTtl};

/// The text of `value` without any cache marker.
fn unmarked(value: &impl serde::Serialize) -> String {
    let text = serde_json::to_string(value).unwrap();
    text.replace(r#","cache_control":{"type":"ephemeral","ttl":"1h"}"#, "")
        .replace(r#","cache_control":{"type":"ephemeral","ttl":"5m"}"#, "")
}

/// The answer of the stream fixture `name`, as the conversation stores it.
fn answer(name: &str) -> Message {
    let message = fixture_completion(name).message;
    assert_eq!(message.role, Role::Assistant);
    assert!(message.provider_raw.is_some(), "{name}");
    message
}

fn result(call_id: &str, output: &str) -> Message {
    Message::new(
        Role::User,
        vec![ContentBlock::ToolResult {
            call_id: call_id.to_owned(),
            output: output.to_owned(),
            is_error: false,
        }],
    )
}

/// The requests of `steps`: each request is the one before plus the answer of a stream
/// fixture, or no answer when that request failed, and the next message.
fn requests_of(steps: &[(Option<&str>, Message)]) -> Vec<Request> {
    let mut history = Vec::new();
    let mut requests = Vec::new();
    for (answered, next) in steps {
        if let Some(fixture) = answered {
            history.push(answer(fixture));
        }
        history.push(next.clone());
        requests.push(request(history.clone()));
    }
    requests
}

/// The requests of four turns, the first with a tool loop, the others answered with
/// thinking, redacted thinking or text.
fn requests() -> Vec<Request> {
    requests_of(&[
        (None, Message::user("Which files are here?")),
        (Some("tool_use.sse"), result("toolu_01T1x5fJ9mB7sQe3RkVw8ZpN", "Cargo.toml\nsrc")),
        (Some("text.sse"), Message::user("Why does the build fail?")),
        (Some("thinking.sse"), Message::user("Check the path first.")),
        (Some("redacted_thinking.sse"), Message::user("Thanks.")),
    ])
}

/// The requests of turns that end on a user message: a call whose request ended on its
/// tool results and failed, then a new prompt; a turn whose request ended on its prompt
/// and failed before an answer, then a new prompt.
fn requests_after_failures() -> Vec<Request> {
    requests_of(&[
        (None, Message::user("Which files are here?")),
        (Some("tool_use.sse"), result("toolu_01T1x5fJ9mB7sQe3RkVw8ZpN", "Cargo.toml\nsrc")),
        (None, Message::user("Try again.")),
        (Some("text.sse"), Message::user("Why does the build fail?")),
        (None, Message::user("Answer, please.")),
    ])
}

#[rstest]
#[case::auto(CacheTtl::Auto)]
#[case::five_minutes(CacheTtl::FiveMinutes)]
#[case::one_hour(CacheTtl::OneHour)]
fn each_body_is_a_byte_prefix_of_the_next(#[case] ttl: CacheTtl) {
    assert_each_is_a_prefix_of_the_next(&requests(), ttl);
}

#[rstest]
#[case::auto(CacheTtl::Auto)]
#[case::five_minutes(CacheTtl::FiveMinutes)]
#[case::one_hour(CacheTtl::OneHour)]
fn a_prompt_after_a_turn_that_ended_on_a_user_message_keeps_that_message(#[case] ttl: CacheTtl) {
    // The new prompt is a message of its own: the user message that ended the request
    // before keeps its bytes. The API joins the two, efr does not.
    assert_each_is_a_prefix_of_the_next(&requests_after_failures(), ttl);
    let body = body_with(requests_after_failures().last().unwrap(), &AnthropicConfig::new());
    let roles: Vec<&str> = body.messages.iter().map(|message| message.role).collect();
    assert_eq!(roles, ["user", "assistant", "user", "user", "assistant", "user", "user"]);
}

fn assert_each_is_a_prefix_of_the_next(requests: &[Request], ttl: CacheTtl) {
    let config = AnthropicConfig::new().with_cache_ttl(ttl);
    let bodies: Vec<_> = requests.iter().map(|request| body_with(request, &config)).collect();
    for pair in bodies.windows(2) {
        let (before, after) = (&pair[0], &pair[1]);
        assert!(serde_json::to_string(before).unwrap().contains("cache_control"));
        assert_eq!(unmarked(&before.tools), unmarked(&after.tools));
        assert_eq!(unmarked(&before.system), unmarked(&after.system));
        let earlier = unmarked(&before.messages);
        let later = unmarked(&after.messages);
        let open = earlier.strip_suffix(']').unwrap();
        assert!(later.starts_with(open), "\n{earlier}\nis not a prefix of\n{later}");
        assert!(later.len() > earlier.len());
    }
}

#[test]
fn the_raw_content_of_each_answer_is_in_every_later_body() {
    let requests = requests();
    let last = requests.last().unwrap();
    let text = serde_json::to_string(&body_with(last, &AnthropicConfig::new())).unwrap();
    for message in last.messages.iter().filter(|message| message.role == Role::Assistant) {
        let raw = message.provider_raw.as_ref().and_then(|raw| raw.as_str()).unwrap();
        assert!(text.contains(&format!(r#"{{"role":"assistant","content":{raw}}}"#)), "{raw}");
    }
}

// Each prompt that opens a turn is an anchor; the call of the tool loop is not. A
// prompt after a turn that ended on a user message opens a turn too.
#[rstest]
#[case::four_turns(requests(), &[0, 4, 6, 8])]
#[case::after_failures(requests_after_failures(), &[0, 3, 5, 6])]
fn a_loop_call_reads_its_anchor_from_an_entry_that_a_call_wrote_for_one_hour(
    #[case] requests: Vec<Request>,
    #[case] expected: &[usize],
) {
    // Under `auto`, S is one hour on every call. A call that marks an anchor writes
    // its tail for one hour, with every marker one hour; any other call has a one-hour
    // marker on a message only where an earlier call wrote its tail for one hour.
    let config = AnthropicConfig::new();
    let bodies: Vec<_> = requests.iter().map(|request| body_with(request, &config)).collect();
    let system: Vec<String> =
        bodies.iter().map(|body| serde_json::to_string(&body.system).unwrap()).collect();
    assert!(system.windows(2).all(|pair| pair[0] == pair[1]), "{system:?}");
    let marker = |text: &str, ttl: &str| text.contains(&format!(r#""ttl":"{ttl}""#));
    let mut anchors = Vec::new();
    for body in &bodies {
        let texts: Vec<String> =
            body.messages.iter().map(|message| serde_json::to_string(message).unwrap()).collect();
        let tail = texts.len() - 1;
        if marker(&texts[tail], "1h") {
            assert!(!texts.iter().any(|text| marker(text, "5m")), "{texts:?}");
            anchors.push(tail);
            continue;
        }
        for (index, _) in texts.iter().enumerate().filter(|(_, text)| marker(text, "1h")) {
            assert!(anchors.contains(&index), "message {index}, anchors {anchors:?}");
        }
    }
    assert_eq!(anchors, expected);
}

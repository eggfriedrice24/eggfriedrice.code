//! The history only grows: over a long conversation, each request starts with the
//! messages of the request before it, across turns, tool calls, a daemon restart, past
//! 60 turns and past the 4096-event page. The one exception is the first request after
//! a compaction. The README, "The history only grows", is the contract.

use efr_protocol::{ConversationCompact, Event, Origin};
use efr_provider::{ContentBlock, Request, Role};
use pretty_assertions::assert_eq;

use crate::compaction::{SUMMARY_OPEN, is_results};
use crate::fresh::FRESH_OPEN;
use crate::testing::{Recorder, Setup, big_text};

/// The turns of the conversation.
const TURNS: u64 = 64;

/// The manual compaction runs after this turn.
const COMPACT_AFTER: u64 = 10;

/// The actor restarts after this turn.
const RESTART_AFTER: u64 = 30;

/// The text deltas of each answer: each is one event in the log, so the log passes the
/// page of 4096 events while the turns after the compaction are still in the history.
const WORDS: usize = 70;

/// The first message that `after` changes of the messages of `before`, or `None` when
/// `after` starts with all of them.
fn first_edit(before: &Request, after: &Request) -> Option<usize> {
    let edited = before.messages.iter().zip(&after.messages).position(|(old, new)| old != new);
    match edited {
        Some(at) => Some(at),
        None if after.messages.len() < before.messages.len() => Some(after.messages.len()),
        None => None,
    }
}

/// What a request keeps of the request before it, as one line for a failure.
fn check(number: usize, before: &Request, after: &Request) -> Option<String> {
    if (&before.model, &before.system, &before.tools) != (&after.model, &after.system, &after.tools)
    {
        return Some(format!("request {number} changes the model, the system or the tools"));
    }
    first_edit(before, after).map(|at| {
        format!("request {number} edits message {at} of the {} before it", before.messages.len())
    })
}

/// The text of the first block of a user prompt.
fn first_text(message: &efr_provider::Message) -> &str {
    match message.content.first() {
        Some(ContentBlock::Text { text }) => text,
        _ => "",
    }
}

#[tokio::test]
async fn each_request_starts_with_the_request_before_it() {
    let setup = Setup::new();
    let model = Recorder::new(WORDS);
    let mut h = setup.start_model(model.clone()).await;
    let notes = h.cwd.join("notes.txt");
    let mut compacted = None;

    for n in 1..=TURNS {
        if n == COMPACT_AFTER + 1 {
            let params = ConversationCompact {
                command_id: h.command_id(),
                conversation_id: h.conversation_id,
                focus: None,
            };
            h.handle.compact(params).await.expect("compacted");
            compacted = Some(model.requests().iter().filter(|r| !r.side_call).count());
        }
        if n == RESTART_AFTER + 1 {
            h = h.restart_model(model.clone()).await;
        }
        // NOTE: a long second prompt, so the compaction has something before its tail.
        let text = match n {
            2 => format!("prompt 2\n{}", big_text(100_000)),
            n if n % 10 == 0 => format!("read {}", notes.display()),
            n => format!("prompt {n}"),
        };
        let sent = h.prompt(&text).await;
        let end = h.wait_end(sent.turn_id).await;
        assert!(matches!(end, Event::TurnCompleted { .. }), "turn {n}: {end:?}");
    }

    let events = h.events().await;
    let paged = events.iter().filter(|event| event.kind() != "tool_call_output_updated").count();
    assert!(paged > 4096, "the log has {paged} events");
    let requests = model.requests();
    let (summaries, turns): (Vec<&Request>, Vec<&Request>) =
        requests.iter().partition(|request| request.side_call);
    let compacted = compacted.expect("the compaction ran");
    let [summary] = summaries.as_slice() else {
        panic!("one summary request: {}", summaries.len());
    };
    // NOTE: the summary request is the history as the last request sent it, so it hits
    // the prompt cache too.
    assert_eq!(first_edit(turns[compacted - 1], summary), None);
    let failures: Vec<String> = turns
        .windows(2)
        .enumerate()
        .filter(|(at, _)| at + 1 != compacted)
        .filter_map(|(at, pair)| check(at + 2, pair[0], pair[1]))
        .collect();
    assert_eq!(failures, Vec::<String>::new());
    let after = &turns[compacted].messages;
    assert!(first_text(&after[0]).starts_with(FRESH_OPEN), "{:?}", after[0]);
    assert!(first_text(&after[1]).starts_with(SUMMARY_OPEN), "{:?}", after[1]);

    // Every prompt of the last request keeps its preamble, and the turns since the
    // compaction are more than the 50 that the history once kept.
    let last = turns.last().expect("requests");
    let prompts: Vec<_> = last.messages[2..]
        .iter()
        .filter(|message| message.role == Role::User && !is_results(message))
        .collect();
    assert!(prompts.len() > 50, "{} prompts", prompts.len());
    for prompt in prompts {
        assert!(first_text(prompt).starts_with("<live_state>\n"), "{prompt:?}");
    }
    assert!(last.messages.iter().any(|message| message.provider_raw.is_some()));
    h.finish();
}

/// The rule of the live state, as each block starts with it.
const CURRENT: &str = "<live_state>\nThis block shows the state when the user sent this \
                       prompt. Only the newest live_state block is current; an older one \
                       shows the state at its own prompt.\n";

#[tokio::test]
async fn a_custom_system_prompt_keeps_the_rule_of_the_live_state_and_old_blocks_keep_their_bytes() {
    let mut setup = Setup::new();
    setup.config.system_prompt = Some("You are a terse helper.".to_owned());
    let model = Recorder::new(3);
    let mut h = setup.start_model(model.clone()).await;
    let other = h.dirs.create_dir("home/other").expect("another directory");

    for (n, cwd) in [(1, h.cwd.clone()), (2, other.clone())] {
        let sent = h.prompt_in(&cwd, &format!("prompt {n}")).await;
        let end = h.wait_end(sent.turn_id).await;
        assert!(matches!(end, Event::TurnCompleted { .. }), "turn {n}: {end:?}");
    }
    h = h.restart_model(model.clone()).await;
    let sent = h.prompt_in(&other, "prompt 3").await;
    h.wait_end(sent.turn_id).await;

    let requests = model.requests();
    let [one, two, three] = requests.as_slice() else {
        panic!("three requests: {}", requests.len());
    };
    for request in &requests {
        let system = request.system.as_deref().unwrap_or_default();
        assert!(!system.contains("live_state"), "the custom prompt says nothing of it");
        let newest = request.messages.last().expect("a prompt");
        assert!(first_text(newest).starts_with(CURRENT), "{:?}", first_text(newest));
    }
    // The block of the first prompt names its own directory, and every later request,
    // also after the restart, sends it with the same bytes.
    let first = serde_json::to_string(&one.messages[0]).expect("json");
    assert!(first_text(&one.messages[0]).contains(&h.cwd.display().to_string()));
    for later in [two, three] {
        assert_eq!(serde_json::to_string(&later.messages[0]).expect("json"), first);
    }
    assert_eq!(check(2, one, two), None);
    assert_eq!(check(3, two, three), None);
    h.finish();
}

/// A last command with a secret in each form that the preamble redacts.
const SECRET_COMMAND: &str = "mysql -u root -phunter2 db && git clone https://me:t0ken@example.com/r \
                              && curl -H 'Authorization: Bearer abc123' x";

/// The secrets in [`SECRET_COMMAND`].
const SECRETS: &[&str] = &["hunter2", "t0ken", "abc123"];

/// The redaction runs when the preamble is rendered, so the prompt that the store keeps
/// is the prompt that the model read: after a restart, the next request sends the first
/// prompt again byte for byte, from the store, and no request holds a secret.
#[tokio::test]
async fn the_kept_preamble_is_redacted_once_and_sent_again_as_it_was() {
    let setup = Setup::new();
    let model = Recorder::new(2);
    let mut h = setup.start_model(model.clone()).await;
    let cwd = h.cwd.clone();
    let mut params = h.prompt_params(&cwd, "why did it fail");
    params.last_command = Some(SECRET_COMMAND.to_owned());
    let sent = h.handle.send_prompt(params, Origin::Shell).await.expect("prompt accepted");
    h.wait_end(sent.turn_id).await;
    h = h.restart_model(model.clone()).await;
    let sent = h.prompt("and now?").await;
    h.wait_end(sent.turn_id).await;

    let requests = model.requests();
    let [first, second] = requests.as_slice() else {
        panic!("two requests: {}", requests.len());
    };
    assert_eq!(first_edit(first, second), None, "the stored prompt is the sent prompt");
    let prompt = first_text(&first.messages[0]);
    let redacted = "mysql -u root -p[redacted] db && git clone https://me:[redacted]@example.com/r \
                    && curl -H 'Authorization: Bearer [redacted]' x";
    assert!(prompt.contains(redacted), "{prompt}");
    let sent = format!("{requests:?}");
    for secret in SECRETS {
        assert!(!sent.contains(secret), "{secret} reached a request");
    }
    h.finish();
}

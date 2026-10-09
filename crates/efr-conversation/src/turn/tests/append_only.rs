//! The history only grows: over a long conversation, each request starts with the
//! messages of the request before it, across turns, tool calls, a daemon restart, past
//! 60 turns and past the 4096-event page. The one exception is the first request after
//! a compaction. The README, "The history only grows", is the contract.

use efr_protocol::{ConversationCompact, Event};
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

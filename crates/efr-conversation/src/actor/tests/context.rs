//! What fills the context (`conversation.context`): the actor answers at any time, also
//! while a turn runs, with the parts of the next request, the newest real count, the
//! compactions and the cache use, and never with the text of a message.

use efr_protocol::{
    CompactionTrigger, ContextPartKind, ConversationContext, ConversationContextResult,
    ConversationId, Event, Usage,
};
use efr_provider::{Message, ProviderEvent, StopReason, TokenUsage};
use efr_stdx::id::uuid_v7;
use efr_test_support::{TestClock, TestRng};
use pretty_assertions::assert_eq;
use serde_json::json;

use crate::ConversationError;
use crate::compaction::{LeftOut, summary_request};
use crate::context::DEFAULT_CONTEXT_WINDOW;
use crate::testing::{
    Harness, SUMMARY, Setup, WINDOW, answer, compacting, done, expect_request, hold, request,
    result_message, run_two_big_turns, text_answer, tool_answer, tool_message, two_big_turns,
};

fn params(h: &Harness, model: Option<&str>) -> ConversationContext {
    ConversationContext { conversation_id: h.conversation_id, model: model.map(str::to_owned) }
}

async fn context(h: &Harness) -> ConversationContextResult {
    h.handle.context(params(h, None)).await.expect("the context")
}

fn count(result: &ConversationContextResult, kind: ContextPartKind) -> u32 {
    result.parts.iter().find(|part| part.kind == kind).map_or(0, |part| part.count)
}

#[tokio::test]
async fn context_answers_while_a_turn_runs() {
    let setup = Setup::new();
    let state = setup.live_state(&setup.cwd, "first");
    let records = vec![
        expect_request(request(vec![setup.prompt(&state, "first")])),
        answer(&[ProviderEvent::TextDelta { text: "One.".to_owned() }]),
        hold(),
        answer(&[done(StopReason::EndTurn, None)]),
    ];
    let mut h = setup.start(records).await;
    let first = h.prompt("first").await.turn_id;
    // NOTE: the provider holds the end of the answer until the test releases it, so
    // the turn runs while the context is measured.
    h.wait_for(|e| matches!(e, Event::AssistantMessageUpdated { .. })).await;

    let during = context(&h).await;

    assert!(during.running);
    assert_eq!(count(&during, ContextPartKind::Prompts), 1, "the running turn's prompt counts");
    assert_eq!(
        count(&during, ContextPartKind::Preambles),
        0,
        "the log does not hold the running turn's preamble"
    );
    assert_eq!(during.counted, None);
    assert_eq!(during.estimate, during.parts.iter().map(|part| part.tokens).sum::<u64>());

    h.provider.handled_through(3);
    h.wait_end(first).await;
    let after = context(&h).await;

    assert!(!after.running);
    assert_eq!(count(&after, ContextPartKind::Preambles), 1, "the ended turn as the model read it");
    assert_eq!(count(&after, ContextPartKind::Answers), 1);
    h.finish();
}

#[tokio::test]
async fn context_output_holds_no_message_text() {
    let setup = Setup::new();
    let state = setup.live_state(&setup.cwd, "PROMPT-SECRET-1 what is in the notes");
    let path = setup.home().join("notes.txt");
    let read = json!({ "path": path });
    let prompt = "PROMPT-SECRET-1 what is in the notes";
    let records = vec![
        expect_request(request(vec![setup.prompt(&state, prompt)])),
        answer(&tool_answer("call_1", "read_file", &read)),
        expect_request(request(vec![
            setup.prompt(&state, prompt),
            tool_message("call_1", "read_file", &read),
            result_message("call_1", "OUTPUT-SECRET-2", false),
        ])),
        answer(&text_answer("ANSWER-SECRET-3")),
    ];
    let mut h = setup.start(records).await;
    h.toolbox
        .outcomes
        .lock()
        .expect("outcomes")
        .insert(path.display().to_string(), crate::ToolOutcome::ok("OUTPUT-SECRET-2"));
    let turn = h.prompt(prompt).await.turn_id;
    h.wait_end(turn).await;

    let result = context(&h).await;

    let wire = serde_json::to_string(&result).expect("the result serializes");
    for secret in ["PROMPT-SECRET-1", "OUTPUT-SECRET-2", "ANSWER-SECRET-3"] {
        assert!(!wire.contains(secret), "{secret} is in {wire}");
    }
    let output = result
        .largest
        .iter()
        .find(|item| item.kind == ContextPartKind::ToolOutputs)
        .expect("the output is an item");
    assert_eq!(output.tool.as_deref(), Some("read_file"));
    assert_eq!(output.label, Some(path.display().to_string()), "only the label of the call");
    h.finish();
}

#[tokio::test]
async fn summary_after_a_compaction() {
    let setup = compacting();
    let (mut records, history) = two_big_turns(&setup);
    records.push(expect_request(summary_request(
        &request(Vec::new()),
        history,
        None,
        LeftOut::default(),
    )));
    records.push(answer(&text_answer(SUMMARY)));
    let mut h = setup.start(records).await;
    run_two_big_turns(&mut h).await;
    let before = context(&h).await;
    let compact = efr_protocol::ConversationCompact {
        command_id: h.command_id(),
        conversation_id: h.conversation_id,
        focus: None,
    };
    let compacted = h.handle.compact(compact).await.expect("compacted").compaction;

    let after = context(&h).await;

    assert_eq!(count(&before, ContextPartKind::Summary), 0);
    assert_eq!(before.compactions.count, 0);
    assert_eq!(count(&after, ContextPartKind::Summary), 2, "the fresh block and the summary");
    assert_eq!(count(&after, ContextPartKind::Prompts), 1, "only the tail keeps its prompt");
    assert_eq!(
        (before.estimate, after.estimate),
        (compacted.tokens_before, compacted.tokens_after),
        "the same request that the compaction measured"
    );
    assert_eq!(after.counted, None, "the count is from before the compaction");
    assert_eq!(after.compactions.count, 1);
    let newest = after.compactions.newest.expect("the newest compaction");
    assert_eq!(
        (newest.trigger, newest.tokens_before, newest.tokens_after, newest.summarized),
        (CompactionTrigger::Manual, compacted.tokens_before, compacted.tokens_after, true)
    );
    h.finish();
}

#[tokio::test]
async fn unknown_model_says_window_unknown() {
    let setup = compacting();
    let h = setup.start(Vec::new()).await;

    let known = context(&h).await;
    let unknown = h.handle.context(params(&h, Some("mystery-model"))).await.expect("context");

    assert_eq!((known.window, known.window_known), (WINDOW, true));
    assert_eq!(known.model, crate::testing::MODEL);
    assert_eq!((unknown.window, unknown.window_known), (DEFAULT_CONTEXT_WINDOW, false));
    assert_eq!(unknown.model, "mystery-model");
    assert_eq!(unknown.hard_cap, DEFAULT_CONTEXT_WINDOW * 95 / 100);
    assert!(known.parts.iter().any(|part| part.kind == ContextPartKind::System));
    h.finish();
}

#[tokio::test]
async fn cache_block_sums_the_conversation() {
    let setup = Setup::new();
    let state = setup.live_state(&setup.cwd, "one");
    let usage = |input: u64, cached: u64, written: u64| TokenUsage {
        input_tokens: input,
        output_tokens: 100,
        cached_input_tokens: cached,
        reasoning_tokens: 0,
        cache_write_tokens: written,
        cache_write_1h_tokens: written,
    };
    let records = vec![
        expect_request(request(vec![setup.prompt(&state, "one")])),
        answer(&[
            ProviderEvent::TextDelta { text: "ok 1".to_owned() },
            ProviderEvent::Usage(usage(3_000, 0, 3_000)),
            done(StopReason::EndTurn, None),
        ]),
        expect_request(request(vec![
            setup.prompt(&state, "one"),
            Message::assistant("ok 1"),
            setup.prompt(&state, "two"),
        ])),
        answer(&[
            ProviderEvent::TextDelta { text: "ok 2".to_owned() },
            ProviderEvent::Usage(usage(3_200, 3_000, 200)),
            done(StopReason::EndTurn, None),
        ]),
    ];
    let mut h = setup.start(records).await;
    let one = h.prompt("one").await.turn_id;
    h.wait_end(one).await;
    let two = h.prompt("two").await.turn_id;
    h.wait_end(two).await;

    let result = context(&h).await;

    let cache = result.cache;
    assert_eq!(cache.turns, 2);
    let last = cache.last_turn.expect("the last turn's usage");
    assert_eq!((last.input_tokens, last.cached_input_tokens), (3_200, 3_000));
    assert_eq!(
        cache.conversation,
        Usage {
            input_tokens: 6_200,
            output_tokens: 200,
            cached_input_tokens: 3_000,
            reasoning_tokens: 0,
            cache_write_tokens: 3_200,
            cache_write_1h_tokens: 3_200,
            context_tokens: 0,
        }
    );
    let counted = result.counted.expect("a real count");
    assert_eq!((counted.turn_id, counted.tokens), (two, 3_300));
    h.finish();
}

#[tokio::test]
async fn another_conversation_is_refused() {
    let setup = Setup::new();
    let h = setup.start(Vec::new()).await;
    let other = ConversationId::from_uuid(uuid_v7(&TestClock::new(), &TestRng::new(99)));

    let refused =
        h.handle.context(ConversationContext { conversation_id: other, model: None }).await;

    assert!(matches!(refused, Err(ConversationError::WrongConversation { .. })), "{refused:?}");
    h.finish();
}

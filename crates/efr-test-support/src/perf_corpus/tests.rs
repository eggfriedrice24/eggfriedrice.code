use std::collections::HashSet;

use efr_store::{compactions, conversations, turn_messages};
use pretty_assertions::assert_eq;

use super::*;
use crate::TestStore;

fn count(events: &[Event], kind: &str) -> usize {
    events.iter().filter(|event| event.kind() == kind).count()
}

#[test]
fn the_sizes_have_their_shapes() {
    assert_eq!(PerfSize::S.shape().all_turns(), 20);
    assert_eq!(PerfSize::M.shape().all_turns(), 200);
    assert_eq!(PerfSize::L.shape().compacted_through, Some(1_990));
    assert_eq!(PerfSize::Xl.shape().all_turns(), 40_000);
    assert_eq!(PerfSize::Xl.shape().name(), "c20-t2000-k1990");
    assert_eq!(PerfSize::Xl.name(), "xl");
}

#[test]
fn a_turn_has_about_fifty_events_and_ten_messages() {
    let turns: Vec<PerfTurn> = PerfCorpus::new(PerfSize::S, 7).collect();

    assert_eq!(turns.len(), 20);
    assert_eq!(turns[0].events[0].kind(), "conversation_created");
    for turn in &turns[1..] {
        assert_eq!(turn.events.len(), 48);
        assert_eq!(count(&turn.events, "prompt_queued"), 1);
        assert_eq!(count(&turn.events, "assistant_message_completed"), 5);
        assert_eq!(count(&turn.events, "tool_call_output_updated"), 16);
        assert_eq!(count(&turn.events, "tool_call_completed"), 4);
        assert_eq!(turn.events.last().map(Event::kind), Some("turn_completed"));
        assert_eq!(turn.messages.len(), 10);
    }
    let numbers: Vec<u32> = turns.iter().map(|turn| turn.number).collect();
    assert_eq!(numbers, (1..=20).collect::<Vec<_>>());
}

#[test]
fn the_same_seed_gives_the_same_turns() {
    let first: Vec<PerfTurn> = PerfCorpus::new(PerfSize::S, 3).take(3).collect();
    let again: Vec<PerfTurn> = PerfCorpus::new(PerfSize::S, 3).take(3).collect();
    let other: Vec<PerfTurn> = PerfCorpus::new(PerfSize::S, 4).take(3).collect();

    assert_eq!(first, again);
    assert_ne!(first, other);
}

#[test]
fn conversations_are_interleaved_by_turn() {
    let shape = PerfShape { conversations: 3, turns: 2, compacted_through: None };
    let corpus = PerfCorpus::of_shape(shape, 1);
    let ids = corpus.conversation_ids();
    let order: Vec<(ConversationId, u32)> =
        corpus.map(|turn| (turn.conversation_id, turn.number)).collect();

    assert_eq!(ids.iter().collect::<HashSet<_>>().len(), 3);
    assert_eq!(
        order,
        vec![(ids[0], 1), (ids[1], 1), (ids[2], 1), (ids[0], 2), (ids[1], 2), (ids[2], 2)]
    );
}

#[tokio::test]
async fn the_compaction_drops_the_saved_messages_before_its_cut() {
    let shape = PerfShape { conversations: 1, turns: 10, compacted_through: Some(3) };
    let clock = TestClock::new();
    let store = TestStore::open(clock.shared()).await.unwrap();
    let mut corpus = PerfCorpus::of_shape(shape, 5);
    let id = corpus.conversation_ids()[0];
    let turn_ids: Vec<TurnId> = PerfCorpus::of_shape(shape, 5).map(|turn| turn.turn_id).collect();

    let written = corpus.write(store.writer(), &clock).await.unwrap();

    assert_eq!(written.turns, 10);
    assert_eq!(written.events, 10 * 48 + 2);
    let (turns, saved, latest) = store
        .readers()
        .with(move |conn| {
            Ok((
                conversations::turns(conn, id)?,
                turn_messages::of_conversation(conn, id)?,
                compactions::latest(conn, id)?,
            ))
        })
        .await
        .unwrap();
    assert_eq!(turns.len(), 10);
    let saved: Vec<TurnId> = saved.iter().map(|turn| turn.turn_id).collect();
    assert_eq!(saved, turn_ids[3..].to_vec(), "turns 1 to 3 are covered by the summary");
    let summary = latest.summary.unwrap().compaction;
    assert_eq!(summary.through_turn, turn_ids[2]);
    assert_eq!(summary.turn_id, Some(turn_ids[8]));
    store.close().await;
}

use efr_conversation::{ConversationDraft, LiveToolInput, LiveTurn};
use efr_protocol::{CompactionTrigger, ContextUse, ConversationId, DraftPart, Seq, TurnId};
use pretty_assertions::assert_eq;
use proptest::prelude::*;

use super::PendingDrafts;

const CONVERSATION: &str = "019a9b1c-3d00-7a10-8b20-000000000001";
const TURN: &str = "019a9b1c-3d00-7a10-8b20-000000000002";
const NEXT_TURN: &str = "019a9b1c-3d00-7a10-8b20-000000000003";

fn conversation() -> ConversationId {
    CONVERSATION.parse().unwrap()
}

fn turn() -> TurnId {
    TURN.parse().unwrap()
}

fn draft(after_seq: u64, part: DraftPart) -> ConversationDraft {
    ConversationDraft {
        conversation_id: conversation(),
        turn_id: turn(),
        after_seq: Seq::new(after_seq),
        part,
    }
}

fn text(index: u32, offset: u64, delta: &str) -> DraftPart {
    DraftPart::Text { index, offset, delta: delta.to_owned() }
}

fn reasoning(offset: u64, delta: &str, title: Option<&str>) -> DraftPart {
    DraftPart::Reasoning { offset, delta: delta.to_owned(), title: title.map(str::to_owned) }
}

fn input(call: u32, bytes: u64) -> DraftPart {
    DraftPart::ToolInput { call, tool: "shell".to_owned(), bytes }
}

fn context(used: u64) -> DraftPart {
    DraftPart::Context(ContextUse { tokens: used, limit: 900, window: 1000 })
}

/// Every held draft, oldest first, as `(after_seq, part)`.
fn drain(pending: &mut PendingDrafts) -> Vec<(u64, DraftPart)> {
    std::iter::from_fn(|| pending.pop()).map(|draft| (draft.after_seq.get(), draft.part)).collect()
}

#[test]
fn text_drafts_of_one_message_merge_into_one() {
    let mut pending = PendingDrafts::default();
    pending.push(draft(3, text(0, 0, "Hello")));
    pending.push(draft(3, text(0, 5, ", ")));
    pending.push(draft(4, text(0, 7, "wörld")));
    assert_eq!(drain(&mut pending), vec![(4, text(0, 0, "Hello, wörld"))]);
    assert!(pending.is_empty());
}

#[test]
fn bytes_that_went_already_are_cut_off() {
    let mut pending = PendingDrafts::default();
    pending.push(draft(1, text(0, 0, "Grüße")));
    assert_eq!(drain(&mut pending), vec![(1, text(0, 0, "Grüße"))]);
    // A draft that starts inside what went keeps only its new bytes.
    pending.push(draft(2, text(0, 2, "üße, 世界")));
    pending.push(draft(2, text(0, 0, "Grüße")));
    assert_eq!(drain(&mut pending), vec![(2, text(0, "Grüße".len() as u64, ", 世界"))]);
}

#[test]
fn a_gap_and_a_new_message_hold_drafts_of_their_own() {
    let mut pending = PendingDrafts::default();
    pending.push(draft(1, text(0, 0, "ab")));
    pending.push(draft(1, text(0, 5, "fg")));
    pending.push(draft(1, text(0, 7, "h")));
    pending.push(draft(2, text(1, 0, "next")));
    assert_eq!(
        drain(&mut pending),
        vec![(1, text(0, 0, "ab")), (1, text(0, 5, "fgh")), (2, text(1, 0, "next"))]
    );
}

#[test]
fn a_new_turn_starts_its_text_and_reasoning_again() {
    let mut pending = PendingDrafts::default();
    pending.push(draft(1, text(0, 0, "old")));
    pending.push(draft(1, reasoning(0, "old", None)));
    drain(&mut pending);
    let next =
        ConversationDraft { turn_id: NEXT_TURN.parse().unwrap(), ..draft(5, text(0, 0, "o")) };
    pending.push(next.clone());
    let thought = ConversationDraft { part: reasoning(0, "x", None), ..next.clone() };
    pending.push(thought.clone());
    let held: Vec<ConversationDraft> = std::iter::from_fn(|| pending.pop()).collect();
    assert_eq!(held, vec![next, thought], "nothing of the new turn went yet");
}

#[test]
fn reasoning_merges_and_keeps_the_newest_title() {
    let mut pending = PendingDrafts::default();
    pending.push(draft(1, reasoning(0, "**Plan**\n", Some("Plan"))));
    pending.push(draft(1, reasoning(9, "a", Some("Plan"))));
    pending.push(draft(2, reasoning(10, "\n**Fix**", Some("Fix"))));
    assert_eq!(drain(&mut pending), vec![(2, reasoning(0, "**Plan**\na\n**Fix**", Some("Fix")))]);
}

#[test]
fn tool_input_keeps_the_newest_count_of_each_call() {
    let mut pending = PendingDrafts::default();
    pending.push(draft(1, input(0, 2)));
    pending.push(draft(1, input(1, 0)));
    pending.push(draft(2, input(0, 40)));
    assert_eq!(drain(&mut pending), vec![(2, input(0, 40)), (1, input(1, 0))]);
}

#[test]
fn a_context_draft_ends_a_held_compaction_and_goes_last() {
    let compacting = DraftPart::Compacting { trigger: CompactionTrigger::Auto };
    let mut pending = PendingDrafts::default();
    pending.push(draft(1, context(10)));
    pending.push(draft(1, compacting.clone()));
    pending.push(draft(1, text(0, 0, "a")));
    pending.push(draft(2, context(20)));
    assert_eq!(drain(&mut pending), vec![(1, text(0, 0, "a")), (2, context(20))]);

    pending.push(draft(3, context(30)));
    pending.push(draft(3, compacting.clone()));
    pending.push(draft(4, compacting.clone()));
    assert_eq!(drain(&mut pending), vec![(3, context(30)), (4, compacting)]);
}

#[test]
fn drafts_before_a_boundary_go() {
    let mut pending = PendingDrafts::default();
    pending.push(draft(1, text(0, 0, "a")));
    pending.push(draft(5, reasoning(0, "b", None)));
    pending.drop_before(Seq::new(5));
    assert_eq!(drain(&mut pending), vec![(5, reasoning(0, "b", None))]);
}

fn live(text: &str, reasoning: &str) -> LiveTurn {
    LiveTurn {
        turn_id: turn(),
        after_seq: Seq::new(9),
        index: 0,
        text: text.to_owned(),
        reasoning: reasoning.to_owned(),
        title: Some("Plan".to_owned()),
        tools: vec![LiveToolInput { call: 0, tool: "shell".to_owned(), bytes: 12 }],
    }
}

#[test]
fn a_rebuild_holds_what_did_not_go_from_the_live_parts() {
    let mut pending = PendingDrafts::default();
    pending.push(draft(2, text(0, 0, "Hello")));
    pending.push(draft(2, reasoning(0, "**Plan**", Some("Plan"))));
    drain(&mut pending);
    pending.push(draft(3, text(0, 5, ", w")));

    pending.rebuild(
        conversation(),
        Some(live("Hello, world", "**Plan**\nmore")),
        vec![draft(8, context(50))],
    );
    assert_eq!(
        drain(&mut pending),
        vec![
            (9, text(0, 5, ", world")),
            (8, context(50)),
            (9, reasoning(8, "\nmore", Some("Plan"))),
            (9, input(0, 12)),
        ]
    );
    // Drafts of the channel after the rebuild bring nothing new.
    pending.push(draft(4, text(0, 8, "orld")));
    assert!(pending.is_empty());
}

#[test]
fn a_rebuild_with_nothing_sent_holds_the_whole_parts() {
    let mut pending = PendingDrafts::default();
    pending.rebuild(conversation(), Some(live("Hi", "")), Vec::new());
    assert_eq!(drain(&mut pending), vec![(9, text(0, 0, "Hi")), (9, input(0, 12))]);
    pending.rebuild(conversation(), None, Vec::new());
    assert!(pending.is_empty(), "no running turn, nothing to build");
}

/// What happens to one text draft of the channel on its way to the subscriber.
#[derive(Debug, Clone, Copy)]
enum Fate {
    /// It reaches the live side.
    Arrives,
    /// The channel lost it; the live side rebuilds from the live parts at once.
    Lost,
    /// It reaches the live side, and then the room has a place for one draft.
    ArrivesAndOneGoes,
}

proptest! {
    /// Any split of a text into drafts, any of them lost and rebuilt, and any pace of
    /// the room give drafts that join, at their offsets, to the whole text with no
    /// gap and no byte twice.
    #[test]
    fn any_loss_and_any_pace_give_the_whole_text_once(
        pieces in proptest::collection::vec(
            proptest::sample::select(vec!["a", "bc", "é", "世界", "😀", "\n", " "]),
            1..40,
        ),
        fates in proptest::collection::vec(
            prop_oneof![
                3 => Just(Fate::Arrives),
                1 => Just(Fate::Lost),
                2 => Just(Fate::ArrivesAndOneGoes),
            ],
            40,
        ),
    ) {
        let mut pending = PendingDrafts::default();
        let mut whole = String::new();
        let mut sent: Vec<(u64, String)> = Vec::new();
        let take = |pending: &mut PendingDrafts, sent: &mut Vec<(u64, String)>| {
            if let Some(ConversationDraft { part: DraftPart::Text { offset, delta, .. }, .. }) =
                pending.pop()
            {
                sent.push((offset, delta));
            }
        };
        for (n, piece) in pieces.iter().enumerate() {
            let offset = whole.len() as u64;
            whole.push_str(piece);
            let made = draft(n as u64, text(0, offset, piece));
            match fates[n] {
                Fate::Arrives => pending.push(made),
                Fate::Lost => {
                    pending.rebuild(conversation(), Some(live(&whole, "")), Vec::new());
                }
                Fate::ArrivesAndOneGoes => {
                    pending.push(made);
                    take(&mut pending, &mut sent);
                }
            }
        }
        while !pending.is_empty() {
            take(&mut pending, &mut sent);
        }
        let mut joined = String::new();
        for (offset, delta) in sent {
            prop_assert_eq!(offset, joined.len() as u64, "no gap and no byte twice");
            joined.push_str(&delta);
        }
        prop_assert_eq!(joined, whole);
    }
}

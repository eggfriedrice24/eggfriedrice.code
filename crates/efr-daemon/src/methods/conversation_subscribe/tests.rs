//! The live side of a subscription with drafts, without a daemon: a channel that left
//! it behind and a room that is full.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64};

use efr_conversation::{ConversationDraft, LiveToolInput, LiveTurn, draft_channel};
use efr_protocol::{ConversationId, ConversationSubscribeItem, DraftPart, Seq, TurnId};
use efr_store::Committed;
use efr_transport::{Delivery, LOSSY_QUEUE_FRAMES, SubscriptionReceiver};
use pretty_assertions::assert_eq;
use tokio::sync::{broadcast, oneshot};
use tokio_util::sync::CancellationToken;

use super::pending::{Boundary, PendingDrafts};
use super::{DraftFeed, Live, LiveStart, Progress, Rebuild};

const CONVERSATION: &str = "019a9b1c-3d00-7a10-8b20-000000000001";
const TURN: &str = "019a9b1c-3d00-7a10-8b20-000000000002";

fn conversation() -> ConversationId {
    CONVERSATION.parse().unwrap()
}

fn turn() -> TurnId {
    TURN.parse().unwrap()
}

/// A live side with no commits, fed by `drafts` and `rebuild`, whose start did
/// `at_start` with the live parts; returns the receiver of the subscription and the parts the
/// test keeps alive.
fn start(
    drafts: broadcast::Receiver<ConversationDraft>,
    rebuild: Rebuild,
    at_start: LiveStart,
) -> (
    SubscriptionReceiver<ConversationSubscribeItem>,
    broadcast::Sender<Committed>,
    CancellationToken,
) {
    let (commits, committed) = broadcast::channel(8);
    let (sender, receiver) = efr_transport::subscription();
    let live = Live {
        committed,
        sender,
        conversation_id: conversation(),
        progress: Arc::new(Progress {
            last: Arc::new(AtomicU64::new(0)),
            lagged: AtomicBool::new(false),
        }),
        drafts: None,
        boundary: Boundary::default(),
        pending: PendingDrafts::default(),
        rebuild: None,
    };
    let (feed, fed) = oneshot::channel();
    let _ = feed.send(DraftFeed { receiver: drafts, held: Vec::new(), rebuild, live: at_start });
    let cancelled = CancellationToken::new();
    tokio::spawn(live.produce(fed, cancelled.clone()));
    (receiver, commits, cancelled)
}

/// Sends `count` text drafts of one message on `channel`; returns the whole text.
fn send_words(channel: &broadcast::Sender<ConversationDraft>, count: usize) -> String {
    let mut whole = String::new();
    for n in 0..count {
        let delta = format!("wörd{n} ");
        let part = DraftPart::Text { index: 0, offset: whole.len() as u64, delta: delta.clone() };
        whole.push_str(&delta);
        let draft = ConversationDraft {
            conversation_id: conversation(),
            turn_id: turn(),
            after_seq: Seq::new(1),
            part,
        };
        channel.send(draft).unwrap();
    }
    whole
}

/// Reads text drafts until they join to `whole`; returns how many came. Each one
/// starts where the one before it ended.
async fn read_text(
    receiver: &mut SubscriptionReceiver<ConversationSubscribeItem>,
    whole: &str,
) -> usize {
    let mut joined = String::new();
    let mut count = 0;
    while joined.len() < whole.len() {
        let Some(Delivery::Lossy { item: ConversationSubscribeItem::Draft(draft) }) =
            receiver.recv().await
        else {
            panic!("only drafts");
        };
        let DraftPart::Text { offset, delta, .. } = draft.draft else {
            panic!("only text: {:?}", draft.draft);
        };
        assert_eq!(offset, joined.len() as u64, "no gap and no byte twice");
        joined.push_str(&delta);
        count += 1;
    }
    assert_eq!(joined, whole);
    count
}

#[tokio::test]
async fn a_subscriber_that_the_channel_left_behind_builds_the_text_again() {
    let channel = draft_channel();
    let drafts = channel.subscribe();
    // NOTE: the turn sends more drafts than the channel keeps before the live side
    // reads one, so the oldest ones are lost to it.
    let whole = send_words(&channel, 300);
    let text = whole.clone();
    let rebuild = Rebuild(Box::new(move || {
        let live = LiveTurn {
            turn_id: turn(),
            after_seq: Seq::new(1),
            index: 0,
            text: text.clone(),
            reasoning: String::new(),
            title: None,
            tools: Vec::new(),
        };
        (Some(live), Vec::new())
    }));
    let (mut receiver, _commits, cancelled) = start(drafts, rebuild, LiveStart::Nothing);
    let count = read_text(&mut receiver, &whole).await;
    assert_eq!(count, 1, "the live parts held the whole text");
    cancelled.cancel();
}

#[tokio::test]
async fn a_full_room_merges_the_drafts_and_sends_them_at_the_next_free_place() {
    let channel = draft_channel();
    let drafts = channel.subscribe();
    let whole = send_words(&channel, 100);
    let nothing = Rebuild(Box::new(|| (None, Vec::new())));
    let (mut receiver, _commits, cancelled) = start(drafts, nothing, LiveStart::Nothing);
    let count = read_text(&mut receiver, &whole).await;
    assert!(count <= LOSSY_QUEUE_FRAMES + 2, "{count} drafts for 100: the rest merged");
    cancelled.cancel();
}

/// The live parts of a turn whose message 0 holds `text` and whose reasoning is
/// `reasoning`, with one tool call that wrote 12 bytes.
fn live_parts(text: &str, reasoning: &str) -> LiveTurn {
    LiveTurn {
        turn_id: turn(),
        after_seq: Seq::new(1),
        index: 0,
        text: text.to_owned(),
        reasoning: reasoning.to_owned(),
        title: Some("Plan".to_owned()),
        tools: vec![LiveToolInput { call: 0, tool: "shell".to_owned(), bytes: 12 }],
    }
}

/// The next draft that `receiver` gets.
async fn next_draft(receiver: &mut SubscriptionReceiver<ConversationSubscribeItem>) -> DraftPart {
    match receiver.recv().await {
        Some(Delivery::Lossy { item: ConversationSubscribeItem::Draft(draft) }) => draft.draft,
        other => panic!("only drafts: {other:?}"),
    }
}

/// A text draft of message 0 at `offset`.
fn text_draft(offset: u64, delta: &str) -> ConversationDraft {
    ConversationDraft {
        conversation_id: conversation(),
        turn_id: turn(),
        after_seq: Seq::new(1),
        part: DraftPart::Text { index: 0, offset, delta: delta.to_owned() },
    }
}

#[tokio::test]
async fn drafts_after_a_snapshot_with_the_live_parts_go_on_past_them() {
    let channel = draft_channel();
    let drafts = channel.subscribe();
    // NOTE: drafts that the turn sent between the subscription and the read of the live
    // parts repeat what the snapshot shows.
    channel.send(text_draft(0, "Hëllo")).unwrap();
    channel.send(text_draft(6, ", wor")).unwrap();
    channel.send(text_draft(11, "ld")).unwrap();
    let nothing = Rebuild(Box::new(|| (None, Vec::new())));
    let shown = LiveStart::Shown(live_parts("Hëllo, wor", "thinking"));
    let (mut receiver, _commits, cancelled) = start(drafts, nothing, shown);

    assert_eq!(
        next_draft(&mut receiver).await,
        DraftPart::ToolInput { call: 0, tool: "shell".to_owned(), bytes: 12 },
        "the snapshot does not show the input of the tool calls"
    );
    let text = DraftPart::Text { index: 0, offset: 11, delta: "ld".to_owned() };
    assert_eq!(next_draft(&mut receiver).await, text, "only the bytes past the snapshot");
    cancelled.cancel();
}

#[tokio::test]
async fn a_replay_start_gets_the_live_parts_as_its_first_drafts() {
    let channel = draft_channel();
    let drafts = channel.subscribe();
    let nothing = Rebuild(Box::new(|| (None, Vec::new())));
    let seeded = LiveStart::Draft(live_parts("Hëllo", "**Plan**\nthinking"));
    let (mut receiver, _commits, cancelled) = start(drafts, nothing, seeded);

    let reasoning = DraftPart::Reasoning {
        offset: 0,
        delta: "**Plan**\nthinking".to_owned(),
        title: Some("Plan".to_owned()),
    };
    assert_eq!(next_draft(&mut receiver).await, reasoning);
    let text = DraftPart::Text { index: 0, offset: 0, delta: "Hëllo".to_owned() };
    assert_eq!(next_draft(&mut receiver).await, text);
    assert!(matches!(next_draft(&mut receiver).await, DraftPart::ToolInput { call: 0, .. }));
    // A draft of the channel that repeats the seeded text is cut to its new bytes.
    channel.send(text_draft(0, "Hëllo!")).unwrap();
    let rest = DraftPart::Text { index: 0, offset: 6, delta: "!".to_owned() };
    assert_eq!(next_draft(&mut receiver).await, rest);
    cancelled.cancel();
}

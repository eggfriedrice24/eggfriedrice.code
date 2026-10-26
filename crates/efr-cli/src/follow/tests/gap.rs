//! A snapshot that efrd cut, against a fake daemon: the view pages the events between
//! the last event that it showed and the snapshot with `conversation.history`, and
//! shows each event once and in order.

use efr_protocol::{
    ConversationHistory, ConversationHistoryResult, ConversationSnapshot, ConversationStatus,
    ConversationSubscribeItem, ConversationSummary, ErrorBody, ErrorCode, Event, EventEnvelope,
    Method, PageCursor, Seq,
};
use pretty_assertions::assert_eq;

use super::{run, subscribed, turn_completed};
use crate::testing::{Conn, TestEnv, conversation, envelope, item, now, turn};

/// The followed turn's message `n`: one line of its own on the screen.
pub(super) fn message(n: u64) -> Event {
    Event::AssistantMessageCompleted {
        turn_id: turn(),
        index: u32::try_from(n).unwrap(),
        text: format!("message {n}."),
    }
}

/// The log of the conversation, oldest first: messages from 10, the event that the
/// view starts after, to `last`.
pub(super) fn log(last: u64) -> Vec<EventEnvelope> {
    (10..=last).map(|n| envelope(n, message(n))).collect()
}

/// The cursor that efrd makes for the events before `seq`.
pub(super) fn before(seq: u64) -> PageCursor {
    PageCursor::new(format!("s:{seq}"))
}

/// A snapshot of `events` (the newest of the log) with a cursor before `cursor` and the
/// high-water mark `hwm`.
pub(super) fn snapshot(
    events: Vec<EventEnvelope>,
    cursor: u64,
    hwm: u64,
) -> ConversationSubscribeItem {
    ConversationSubscribeItem::Snapshot(ConversationSnapshot {
        conversation: ConversationSummary {
            id: conversation(),
            title: None,
            status: ConversationStatus::Running,
            created_at: now(),
            updated_at: now(),
            last_seq: Seq::new(hwm),
            cwd: None,
            scope: None,
            tty: None,
        },
        events,
        history_cursor: Some(before(cursor)),
        hwm: Seq::new(hwm),
        live: None,
    })
}

/// What efrd's `conversation.history` answers from `log`: the newest `limit` events
/// before the cursor, oldest first, and the cursor of the older page.
pub(super) fn page(
    log: &[EventEnvelope],
    params: &ConversationHistory,
) -> ConversationHistoryResult {
    assert_eq!(params.conversation_id, conversation());
    let before = params
        .cursor
        .as_ref()
        .and_then(|cursor| cursor.as_str().strip_prefix("s:"))
        .map_or(u64::MAX, |seq| seq.parse().unwrap());
    let limit = usize::try_from(params.limit.unwrap_or(100)).unwrap();
    let older: Vec<&EventEnvelope> =
        log.iter().filter(|envelope| envelope.seq.get() < before).collect();
    let start = older.len().saturating_sub(limit);
    let events: Vec<EventEnvelope> =
        older[start..].iter().map(|&envelope| envelope.clone()).collect();
    let next_cursor = (start > 0).then(|| self::before(events[0].seq.get()));
    ConversationHistoryResult { events, next_cursor }
}

/// Answers `conversation.history` from `log` until the view has a page that reaches
/// `seen`, the last event that it showed, or the first event; gives the limits that the
/// view asked for. A limit over `frame_room` fails as a page too large for one frame.
pub(super) async fn serve_history(
    conn: &mut Conn,
    log: &[EventEnvelope],
    seen: u64,
    frame_room: u32,
) -> Vec<u32> {
    let mut limits = Vec::new();
    loop {
        let (id, method) = conn.request().await;
        let Method::ConversationHistory(params) = method else {
            panic!("expected conversation.history, got {}", method.name());
        };
        let limit = params.limit.unwrap();
        limits.push(limit);
        if limit > frame_room {
            let body =
                ErrorBody::new(ErrorCode::Internal, "an answer could not be sent to the client");
            conn.fail(id, body).await;
            continue;
        }
        let page = page(log, &params);
        conn.reply(id, &page).await;
        let reached = page.next_cursor.is_none()
            || page.events.first().is_some_and(|first| first.seq.get() <= seen);
        if reached {
            return limits;
        }
    }
}

/// The numbers of the messages on the screen, in their order.
pub(super) fn shown(out: &str) -> Vec<u64> {
    out.lines()
        .filter_map(|line| line.trim().strip_prefix("message ")?.strip_suffix('.')?.parse().ok())
        .collect()
}

/// The review finding: after an overflow, a gap of more than 128 events came as a
/// snapshot of the newest 128, and the 300 events before them never showed.
#[tokio::test]
async fn a_gap_of_300_events_after_an_overflow_shows_every_event_once_in_order() {
    let env = TestEnv::new();
    let ctx = env.context();
    let log = log(439);
    let (result, out, _) = run(&env, &ctx, |mut conn, _| async move {
        let first = subscribed(&mut conn, 10).await;
        conn.item(first, &ConversationSubscribeItem::Event(log[1].clone())).await;
        conn.fail(first, ErrorBody::overflow(Seq::new(11))).await;
        let second = subscribed(&mut conn, 11).await;
        // 300 events after 11, then the newest 128 in the snapshot.
        let newest = log.iter().filter(|envelope| envelope.seq.get() >= 312).cloned().collect();
        conn.item(second, &snapshot(newest, 312, 439)).await;
        // The live events go on while the view reads the gap.
        conn.item(second, &item(440, turn_completed())).await;
        let limits = serve_history(&mut conn, &log, 11, u32::MAX).await;
        // Three pages of at most 128 reach event 11, and none goes past it.
        assert_eq!(limits, [128, 128, 128]);
        conn.until_closed().await;
    })
    .await;
    result.unwrap();
    assert_eq!(shown(&out), (11..=439).collect::<Vec<_>>());
}

/// The review finding: an event over 1 MiB is never in a snapshot, so the first
/// subscription of `efr send` lost it and the events before it.
#[tokio::test]
async fn a_gap_with_one_event_of_1_5_mib_shows_it_once_in_order() {
    let env = TestEnv::new();
    let ctx = env.context();
    let big = format!("big {}end", "word ".repeat(1536 * 1024 / 5));
    let mut log = log(11);
    let large = Event::AssistantMessageCompleted { turn_id: turn(), index: 12, text: big };
    log.push(envelope(12, large));
    log.push(envelope(13, message(13)));
    let (result, out, _) = run(&env, &ctx, |mut conn, _| async move {
        let id = subscribed(&mut conn, 10).await;
        conn.item(id, &snapshot(vec![log[3].clone()], 13, 13)).await;
        conn.item(id, &item(14, turn_completed())).await;
        let limits = serve_history(&mut conn, &log, 10, u32::MAX).await;
        assert_eq!(limits, [128]);
        conn.until_closed().await;
    })
    .await;
    result.unwrap();
    assert_eq!(shown(&out), [11, 13]);
    assert_eq!(out.matches("big word").count(), 1, "the large event shows once");
    let big_at = out.find("big word").unwrap();
    assert!(out.find("message 11.").unwrap() < big_at && big_at < out.find("message 13.").unwrap());
    assert!(out.contains("word end"), "the large event shows whole");
}

/// A page of `conversation.history` larger than a frame fails in efrd, so the view
/// asks again for half as many events, and keeps that size for the rest of the gap.
#[tokio::test]
async fn a_history_page_too_large_for_a_frame_is_asked_again_with_fewer_events() {
    let env = TestEnv::new();
    let ctx = env.context();
    let log = log(300);
    let (result, out, _) = run(&env, &ctx, |mut conn, _| async move {
        let id = subscribed(&mut conn, 10).await;
        let newest = log.iter().filter(|envelope| envelope.seq.get() >= 200).cloned().collect();
        conn.item(id, &snapshot(newest, 200, 300)).await;
        conn.item(id, &item(301, turn_completed())).await;
        let limits = serve_history(&mut conn, &log, 10, 32).await;
        // 128 and 64 fail; the rest of the gap comes in pages of 32, back to event 10.
        assert_eq!(limits, [128, 64, 32, 32, 32, 32, 32, 32]);
        conn.until_closed().await;
    })
    .await;
    result.unwrap();
    assert_eq!(shown(&out), (11..=300).collect::<Vec<_>>());
}

/// A refusal of `conversation.history` that is not a lost connection leaves a note and
/// the snapshot's events: the turn goes on.
#[tokio::test]
async fn a_refused_history_page_leaves_a_note_and_the_snapshot() {
    let env = TestEnv::new();
    let ctx = env.context();
    let log = log(200);
    let (result, out, err) = run(&env, &ctx, |mut conn, _| async move {
        let id = subscribed(&mut conn, 10).await;
        let newest = log.iter().filter(|envelope| envelope.seq.get() >= 150).cloned().collect();
        conn.item(id, &snapshot(newest, 150, 200)).await;
        conn.item(id, &item(201, turn_completed())).await;
        let (history, method) = conn.request().await;
        assert!(matches!(method, Method::ConversationHistory(_)), "got {}", method.name());
        conn.fail(history, ErrorBody::new(ErrorCode::Busy, "the store is busy")).await;
        conn.until_closed().await;
    })
    .await;
    result.unwrap();
    assert_eq!(shown(&out), (150..=200).collect::<Vec<_>>());
    let shown_note = format!("{out}{err}");
    assert!(
        shown_note.contains("efr could not read the events before these: the store is busy"),
        "{shown_note}"
    );
}

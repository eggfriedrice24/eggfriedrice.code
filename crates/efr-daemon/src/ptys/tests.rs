use bytes::Bytes;
use efr_protocol::{ConversationId, PtyId, Size};
use pretty_assertions::assert_eq;

use crate::ptys::{ATTACH_QUEUE, AttachMark, PtyActivity, PtyDelivery, PtyLive, Ptys};

fn pty(n: u128) -> PtyId {
    PtyId::from_uuid(uuid::Uuid::from_u128(n))
}

fn conversation(n: u128) -> ConversationId {
    ConversationId::from_uuid(uuid::Uuid::from_u128(n))
}

#[tokio::test]
async fn attached_clients_get_output_and_resizes_in_order() {
    let ptys = Ptys::default();
    ptys.started(pty(1), conversation(1), None);
    let mut client = ptys.attach(pty(1)).unwrap();

    ptys.recorded(pty(1), 0, Bytes::from_static(b"$ "));
    ptys.resized(pty(1), 2, Size { cols: 80, rows: 24 });
    ptys.recorded(pty(1), 2, Bytes::from_static(b"ls"));

    assert_eq!(
        client.recv().await,
        Some(PtyDelivery::Live(PtyLive::Output { start: 0, data: Bytes::from_static(b"$ ") }))
    );
    assert_eq!(
        client.recv().await,
        Some(PtyDelivery::Live(PtyLive::Resized { at: 2, size: Size { cols: 80, rows: 24 } }))
    );
    assert_eq!(
        client.recv().await,
        Some(PtyDelivery::Live(PtyLive::Output { start: 2, data: Bytes::from_static(b"ls") }))
    );
    assert_eq!(ptys.attach(pty(1)).unwrap().mark().end, 4);
}

#[test]
fn the_mark_of_an_attach_counts_the_output_and_the_sizes_at_its_end() {
    let ptys = Ptys::default();
    ptys.started(pty(1), conversation(1), None);
    let size = Size { cols: 80, rows: 24 };
    let mark = |ptys: &Ptys| ptys.attach(pty(1)).unwrap().mark();

    assert_eq!(mark(&ptys), AttachMark { end: 0, sizes_at_end: 0 });
    ptys.resized(pty(1), 0, size);
    assert_eq!(mark(&ptys), AttachMark { end: 0, sizes_at_end: 1 });
    ptys.recorded(pty(1), 0, Bytes::from_static(b"$ "));
    ptys.resized(pty(1), 2, size);
    ptys.resized(pty(1), 2, size);
    assert_eq!(mark(&ptys), AttachMark { end: 2, sizes_at_end: 2 });
    ptys.recorded(pty(1), 2, Bytes::from_static(b"ls"));
    assert_eq!(mark(&ptys), AttachMark { end: 4, sizes_at_end: 0 });
}

#[tokio::test]
async fn a_client_that_falls_behind_is_closed_with_overflow_and_the_others_continue() {
    let ptys = Ptys::default();
    ptys.started(pty(1), conversation(1), None);
    let mut slow = ptys.attach(pty(1)).unwrap();
    let mut fast = ptys.attach(pty(1)).unwrap();

    for n in 0..=ATTACH_QUEUE as u64 {
        ptys.recorded(pty(1), n, Bytes::from_static(b"x"));
        assert!(matches!(fast.recv().await, Some(PtyDelivery::Live(_))));
    }

    for _ in 0..ATTACH_QUEUE {
        assert!(matches!(slow.recv().await, Some(PtyDelivery::Live(_))));
    }
    assert_eq!(slow.recv().await, Some(PtyDelivery::Overflowed));
    ptys.recorded(pty(1), 999, Bytes::from_static(b"y"));
    assert!(matches!(fast.recv().await, Some(PtyDelivery::Live(_))));
}

#[tokio::test]
async fn attached_streams_end_when_the_shell_exits() {
    let ptys = Ptys::default();
    ptys.started(pty(1), conversation(1), None);
    let mut client = ptys.attach(pty(1)).unwrap();
    ptys.recorded(pty(1), 0, Bytes::from_static(b"bye"));

    ptys.exited(pty(1));

    assert!(matches!(client.recv().await, Some(PtyDelivery::Live(_))));
    assert_eq!(client.recv().await, None);
    assert!(ptys.attach(pty(1)).is_none());
    assert_eq!(ptys.conversation(pty(1)), None);
}

#[test]
fn activity_marks_count_output_and_input_and_see_attached_clients() {
    let ptys = Ptys::default();
    ptys.started(pty(1), conversation(7), None);
    ptys.recorded(pty(1), 0, Bytes::from_static(b"12345"));
    ptys.written(pty(1), 3);

    assert_eq!(
        ptys.activity(),
        [PtyActivity { pty_id: pty(1), conversation: conversation(7), mark: 8, attached: false }]
    );

    let client = ptys.attach(pty(1)).unwrap();
    assert!(ptys.activity()[0].attached);
    drop(client);
    assert!(!ptys.activity()[0].attached, "a dropped client no longer counts");
}

#[test]
fn pty_write_to_adopted_pty_is_not_found() {
    let ptys = Ptys::default();
    ptys.started(pty(1), conversation(7), Some(41));

    ptys.adopt(pty(1));

    // NOTE: `pty.write` and `pty.resize` map the PTY to its conversation first; `None`
    // is `PtyNotFound`, so the old id never reaches the conversation's new shell.
    assert_eq!(ptys.conversation(pty(1)), None);
    assert_eq!(ptys.shell_pids(), [41], "the adopted shell stays a model-side peer");
    assert!(ptys.attach(pty(1)).is_some(), "its output can still be read");
}

#[test]
fn gc_skips_an_adopted_pty_of_a_conversation_with_a_new_shell() {
    let ptys = Ptys::default();
    ptys.started(pty(1), conversation(7), Some(41));
    ptys.adopt(pty(1));
    ptys.started(pty(2), conversation(7), Some(42));

    assert_eq!(
        ptys.activity(),
        [PtyActivity { pty_id: pty(2), conversation: conversation(7), mark: 0, attached: false }]
    );
    assert_eq!(ptys.conversation(pty(2)), Some(conversation(7)));
    ptys.exited(pty(1));
    assert_eq!(ptys.shell_pids(), [42]);
}

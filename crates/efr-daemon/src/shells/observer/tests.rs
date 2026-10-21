use std::sync::Arc;

use efr_protocol::{ConversationId, PtyId};
use efr_shell::{ShellNotice, ShellObserver as _};
use pretty_assertions::assert_eq;

use super::ShellNotices;
use crate::ptys::Ptys;

fn pty(n: u128) -> PtyId {
    PtyId::from_uuid(uuid::Uuid::from_u128(n))
}

fn conversation(n: u128) -> ConversationId {
    ConversationId::from_uuid(uuid::Uuid::from_u128(n))
}

#[test]
fn adopted_notice_marks_the_pty_at_the_hand_over() {
    let ptys = Arc::new(Ptys::default());
    let (observer, mut queue) = ShellNotices::new(Arc::clone(&ptys));
    ptys.started(pty(1), conversation(7), Some(41));

    observer.notice(ShellNotice::Adopted { conversation: conversation(7), pty_id: pty(1) });

    // NOTE: with no answer of `call.background` at all: `pty.write` and `pty.resize`
    // with the old id never reach the conversation's next shell (DS56).
    assert_eq!(ptys.conversation(pty(1)), None);
    ptys.started(pty(2), conversation(7), Some(42));
    assert_eq!(ptys.conversation(pty(2)), Some(conversation(7)));
    assert!(matches!(queue.try_recv(), Ok(ShellNotice::Adopted { .. })));
}

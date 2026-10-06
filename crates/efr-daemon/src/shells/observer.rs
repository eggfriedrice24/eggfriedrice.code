//! The shells' lifecycle notices, recorded as `shell_started`, `cwd_changed` and
//! `shell_exited` events.
//!
//! `ShellObserver::notice` runs on a shell's own task and must not block, so it only
//! updates the PTY table and queues the notice; [`follow_notices`] writes the events.

use std::sync::Arc;

use efr_holder::ChildStatus;
use efr_protocol::{ConversationId, Event};
use efr_shell::{ShellNotice, ShellObserver};
use efr_store::{Batch, WriterHandle};
use tokio::sync::mpsc;

use crate::ptys::Ptys;
use crate::shells::StoreRecording;

/// The observer handed to `ShellSessions`.
#[derive(Debug)]
pub(crate) struct ShellNotices {
    ptys: Arc<Ptys>,
    queue: mpsc::UnboundedSender<ShellNotice>,
}

impl ShellNotices {
    /// The observer, and the queue that [`follow_notices`] reads.
    pub(crate) fn new(ptys: Arc<Ptys>) -> (Arc<Self>, mpsc::UnboundedReceiver<ShellNotice>) {
        // NOTE: unbounded because `notice` must never wait, and notices are rare: one per
        // shell start, exit and directory change.
        let (queue, notices) = mpsc::unbounded_channel();
        (Arc::new(ShellNotices { ptys, queue }), notices)
    }
}

impl ShellObserver for ShellNotices {
    fn notice(&self, notice: ShellNotice) {
        match &notice {
            ShellNotice::Started { conversation, pty_id, pid, .. } => {
                self.ptys.started(*pty_id, *conversation, Some(*pid));
            }
            ShellNotice::Exited { pty_id, .. } => self.ptys.exited(*pty_id),
            _ => {}
        }
        // The queue closes only at shutdown, when nothing records events any more.
        let _ = self.queue.send(notice);
    }
}

/// Records each notice as an event until the queue closes.
pub(crate) async fn follow_notices(
    mut notices: mpsc::UnboundedReceiver<ShellNotice>,
    writer: WriterHandle,
    recording: Arc<StoreRecording>,
) {
    while let Some(notice) = notices.recv().await {
        let Some((conversation, event)) = event_of(notice, &recording) else {
            continue;
        };
        if let Err(error) = writer.append(Batch::new().event(conversation, event)).await {
            tracing::warn!(error = %error, %conversation, "a shell event could not be recorded");
        }
    }
}

/// The event of `notice`, closing the recording of a shell that exited.
fn event_of(notice: ShellNotice, recording: &StoreRecording) -> Option<(ConversationId, Event)> {
    match notice {
        ShellNotice::Started { conversation, pty_id, pid, cwd } => {
            Some((conversation, Event::ShellStarted { pty_id, cwd, pid: Some(pid) }))
        }
        ShellNotice::CwdChanged { conversation, pty_id, cwd, host } => {
            Some((conversation, Event::CwdChanged { pty_id, cwd, host }))
        }
        ShellNotice::Exited { conversation, pty_id, status } => {
            recording.close(pty_id);
            let exit_code = status.and_then(ChildStatus::exit_code);
            Some((conversation, Event::ShellExited { pty_id, exit_code }))
        }
        _ => None,
    }
}

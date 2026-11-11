//! The wait of a model call for its answer: what the provider says about a connect and
//! a retry goes to live clients at once, a client that attaches during the wait sees
//! it, and none of it goes to the event log.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use efr_protocol::DraftPart;
use efr_provider::{
    CallNotice, CallNotices, Provider, ProviderError, ProviderEvent, ProviderId, ProviderStream,
    Request, StopReason,
};
use pretty_assertions::assert_eq;
use tokio::sync::{Notify, broadcast};

use crate::ConversationDraft;
use crate::testing::{Setup, done};

/// A model whose first attempt gets a 503: it says so, waits until the test lets it go
/// on, then connects again and answers.
#[derive(Debug)]
struct Retried {
    id: ProviderId,
    /// The test lets the wait end.
    go: Arc<Notify>,
}

#[async_trait]
impl Provider for Retried {
    fn id(&self) -> &ProviderId {
        &self.id
    }

    async fn stream(&self, request: Request) -> Result<ProviderStream, ProviderError> {
        self.stream_with_notices(request, &efr_provider::NoNotices).await
    }

    async fn stream_with_notices(
        &self,
        _request: Request,
        notices: &dyn CallNotices,
    ) -> Result<ProviderStream, ProviderError> {
        notices.notice(CallNotice::Connecting);
        let wait = Duration::from_millis(1_500);
        notices.notice(CallNotice::Retrying { attempt: 1, wait, status: Some(503) });
        self.go.notified().await;
        notices.notice(CallNotice::Connecting);
        notices.notice(CallNotice::Connected);
        let events = vec![
            ProviderEvent::TextDelta { text: "It is zsh.".to_owned() },
            done(StopReason::EndTurn, None),
        ];
        Ok(Box::pin(futures::stream::iter(events.into_iter().map(Ok))))
    }
}

/// The parts of the drafts that `receiver` holds now, without the `context` ones.
fn parts(receiver: &mut broadcast::Receiver<ConversationDraft>) -> Vec<DraftPart> {
    let mut parts = Vec::new();
    while let Ok(draft) = receiver.try_recv() {
        if !matches!(draft.part, DraftPart::Context(_)) {
            parts.push(draft.part);
        }
    }
    parts
}

fn retrying() -> DraftPart {
    DraftPart::Retrying {
        attempt: 1,
        wait_ms: 1_500,
        status: Some(503),
        reason: "the provider answered 503".to_owned(),
    }
}

#[tokio::test]
async fn a_retry_and_a_connect_go_to_live_clients_at_once_and_to_no_event() {
    let mut setup = Setup::new();
    setup.config.draft_interval = Duration::from_millis(50);
    let mut receiver = setup.drafts.subscribe();
    let go = Arc::new(Notify::new());
    let model = Arc::new(Retried { id: ProviderId::new("replay").unwrap(), go: Arc::clone(&go) });
    let mut h = setup.start_model(model).await;

    let sent = h.prompt("which shell?").await;
    // NOTE: the provider waits for the test, so these drafts went before any answer,
    // and the draft interval held none of them back.
    let held = loop {
        let held: Vec<DraftPart> = h.handle.live_drafts().into_iter().map(|d| d.part).collect();
        if held.iter().any(|part| matches!(part, DraftPart::Retrying { .. })) {
            break held;
        }
        tokio::task::yield_now().await;
    };
    // A client that attaches during the wait sees it, after the context estimate.
    assert!(
        matches!(held.as_slice(), [DraftPart::Context(_), part] if *part == retrying()),
        "{held:?}"
    );
    assert_eq!(parts(&mut receiver), [DraftPart::Connecting, retrying()]);

    go.notify_one();
    h.wait_end(sent.turn_id).await;
    let after = parts(&mut receiver);
    assert_eq!(after[..2], [DraftPart::Connecting, DraftPart::Connected]);
    assert!(matches!(after[2], DraftPart::Text { .. }), "{after:?}");
    assert!(h.handle.live_drafts().is_empty(), "an ended turn has no status");
    // The waits are no events: the log is the same as for a call that went at once.
    let kinds = h.kinds().await;
    assert_eq!(
        kinds,
        [
            "conversation_created",
            "prompt_queued",
            "turn_started",
            "assistant_message_updated",
            "assistant_message_completed",
            "turn_completed"
        ]
    );
}

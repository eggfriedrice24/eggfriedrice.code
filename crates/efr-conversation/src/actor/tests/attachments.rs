//! The queue step of a prompt with attachments: the scratch claim before the bind, the
//! ids, a bind that fails and the files of a withdrawn prompt.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;
use efr_protocol::{
    Attachment, AttachmentId, AttachmentKind, AttachmentOrigin, ConversationId, Event, Origin,
    PromptSendResult, UploadId, WithdrawTarget,
};
use efr_provider::{
    ContentBlock, EncodedBytes, Provider, ProviderError, ProviderId, ProviderStream, Request,
};
use efr_stdx::id::uuid_v7;
use efr_test_support::{TestClock, TestRng};
use pretty_assertions::assert_eq;
use serde_json::json;

use crate::scratch::scratch_dirs;
use crate::testing::{Harness, Setup, text_answer, tool_answer};
use crate::{Attachments, ConversationError};

/// A store that binds each upload as a text file in `$SCRATCH/attachments` and keeps
/// the scratch dir of each bind and the ids of each forget.
#[derive(Debug, Default)]
struct Binder {
    scratch: Mutex<Vec<PathBuf>>,
    forgotten: Mutex<Vec<AttachmentId>>,
    /// The upload that does not bind.
    refused: Option<UploadId>,
}

impl Binder {
    fn scratch(&self) -> Vec<PathBuf> {
        self.scratch.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    fn forgotten(&self) -> Vec<AttachmentId> {
        self.forgotten.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

#[async_trait]
impl Attachments for Binder {
    async fn bind(
        &self,
        _conversation_id: ConversationId,
        id: AttachmentId,
        upload: UploadId,
        scratch: &Path,
    ) -> Option<Attachment> {
        if self.refused == Some(upload) {
            return None;
        }
        self.scratch.lock().unwrap_or_else(PoisonError::into_inner).push(scratch.to_path_buf());
        Some(Attachment {
            name: "notes.txt".to_owned(),
            media_type: "text/plain".to_owned(),
            bytes: 5,
            sha256: "ab".repeat(32),
            kind: AttachmentKind::Text,
            origin: AttachmentOrigin::Clipboard,
            copy: Some(scratch.join("attachments").join(format!("{id}-notes.txt"))),
            sent: None,
            id,
        })
    }

    async fn keep(
        &self,
        _conversation_id: ConversationId,
        _id: AttachmentId,
        _path: &Path,
        _bytes: Vec<u8>,
    ) -> Option<Attachment> {
        None
    }

    async fn load(
        &self,
        _conversation_id: ConversationId,
        _attachment: &Attachment,
    ) -> Option<EncodedBytes> {
        None
    }

    async fn forget(&self, _conversation_id: ConversationId, attachments: &[Attachment]) {
        let ids = attachments.iter().map(|attachment| attachment.id.clone());
        self.forgotten.lock().unwrap_or_else(PoisonError::into_inner).extend(ids);
    }
}

/// A model that calls the `hang` tool for a prompt that says `hang`, and answers each
/// other request with text.
#[derive(Debug)]
struct Model {
    id: ProviderId,
}

fn model() -> Arc<Model> {
    Arc::new(Model { id: ProviderId::new("replay").expect("provider id") })
}

#[async_trait]
impl Provider for Model {
    fn id(&self) -> &ProviderId {
        &self.id
    }

    async fn stream(&self, request: Request) -> Result<ProviderStream, ProviderError> {
        let hangs = request.messages.last().is_some_and(|message| {
            message
                .content
                .iter()
                .any(|block| matches!(block, ContentBlock::Text { text } if text == HANG))
        });
        let events =
            if hangs { tool_answer("call_1", "hang", &json!({})) } else { text_answer("Seen.") };
        Ok(Box::pin(futures::stream::iter(events.into_iter().map(Ok))))
    }
}

/// The prompt whose turn runs until a test stops it.
const HANG: &str = "hang";

fn upload(n: u64) -> UploadId {
    UploadId::from_uuid(uuid_v7(&TestClock::new(), &TestRng::new(n)))
}

/// Sends `text` with the uploads `uploads`.
async fn send(
    h: &mut Harness,
    text: &str,
    uploads: Vec<UploadId>,
) -> Result<PromptSendResult, ConversationError> {
    let cwd = h.cwd.clone();
    let mut params = h.prompt_params(&cwd, text);
    params.attachments = uploads;
    h.handle.send_prompt(params, Origin::Shell).await
}

/// The ids of the attachments that each `prompt_queued` of the conversation recorded.
async fn queued(h: &Harness) -> Vec<Vec<AttachmentId>> {
    h.events()
        .await
        .into_iter()
        .filter_map(|event| match event {
            Event::PromptQueued { attachments, .. } => {
                Some(attachments.into_iter().map(|attachment| attachment.id).collect())
            }
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn the_first_prompt_of_a_new_conversation_binds_into_the_one_scratch_dir_of_its_turn() {
    let binder = Arc::new(Binder::default());
    let mut setup = Setup::new();
    setup.attachments = binder.clone();
    let expected = setup.scratch("what is in these");
    let root = setup.config.scratch_root.clone();
    let mut h = setup.start_model(model()).await;

    let sent = send(&mut h, "what is in these", vec![upload(1), upload(2)]).await.unwrap();
    h.wait_end(sent.turn_id).await;

    assert_eq!(scratch_dirs(&root, h.conversation_id), std::slice::from_ref(&expected));
    assert_eq!(binder.scratch(), [expected.clone(), expected]);
    assert_eq!(queued(&h).await, [vec![AttachmentId::nth(1), AttachmentId::nth(2)]]);
    h.finish();
}

#[tokio::test]
async fn a_prompt_behind_a_running_turn_and_after_a_restart_binds_into_the_same_dir() {
    let binder = Arc::new(Binder::default());
    let mut setup = Setup::new();
    setup.attachments = binder.clone();
    let expected = setup.scratch(HANG);
    let root = setup.config.scratch_root.clone();
    let mut h = setup.start_model(model()).await;
    let running = h.prompt(HANG).await;
    h.toolbox.hang_started.notified().await;

    let behind = send(&mut h, "second", vec![upload(1)]).await.unwrap();
    assert!(behind.queued);
    let more = send(&mut h, "third", vec![upload(2), upload(3)]).await.unwrap();
    assert!(more.queued);
    let interrupt = h.interrupt_params(Some(running.turn_id), Vec::new(), Vec::new());
    h.handle.interrupt(interrupt, Origin::Shell).await.expect("interrupted");
    h.wait_end(more.turn_id).await;
    let mut h = h.restart_model(model()).await;
    let after = send(&mut h, "fourth", vec![upload(4)]).await.unwrap();
    h.wait_end(after.turn_id).await;

    assert_eq!(scratch_dirs(&root, h.conversation_id), std::slice::from_ref(&expected));
    assert_eq!(binder.scratch(), vec![expected; 4]);
    let ids: Vec<Vec<AttachmentId>> = [vec![], vec![1], vec![2, 3], vec![4]]
        .into_iter()
        .map(|numbers| numbers.into_iter().map(AttachmentId::nth).collect())
        .collect();
    assert_eq!(queued(&h).await, ids, "the ids go on after a restart");
    h.finish();
}

#[tokio::test]
async fn a_prompt_whose_upload_does_not_bind_records_nothing_and_forgets_the_others() {
    let binder = Arc::new(Binder { refused: Some(upload(2)), ..Binder::default() });
    let mut setup = Setup::new();
    setup.attachments = binder.clone();
    let mut h = setup.start(Vec::new()).await;

    let refused = send(&mut h, "what is in these", vec![upload(1), upload(2)]).await;

    assert!(
        matches!(refused, Err(ConversationError::UploadNotBound { upload: refused }) if refused == upload(2)),
        "{refused:?}"
    );
    assert_eq!(binder.forgotten(), [AttachmentId::nth(1)]);
    assert_eq!(h.events().await, Vec::<Event>::new());
    h.finish();
}

#[tokio::test]
async fn a_withdrawn_or_interrupted_prompt_forgets_its_attachments() {
    let binder = Arc::new(Binder::default());
    let mut setup = Setup::new();
    setup.attachments = binder.clone();
    let mut h = setup.start_model(model()).await;
    let running = h.prompt(HANG).await;
    h.toolbox.hang_started.notified().await;
    let second = send(&mut h, "second", vec![upload(1)]).await.unwrap();
    let third = send(&mut h, "third", vec![upload(2), upload(3)]).await.unwrap();

    let by_turn = h.withdraw_params(WithdrawTarget::Turn { turn_id: second.turn_id });
    h.handle.withdraw(by_turn, Origin::Shell).await.expect("withdrawn");
    assert_eq!(binder.forgotten(), [AttachmentId::nth(1)]);
    let interrupt = h.interrupt_params(Some(running.turn_id), Vec::new(), vec![third.turn_id]);
    h.handle.interrupt(interrupt, Origin::Shell).await.expect("interrupted");
    assert_eq!(binder.forgotten(), [1, 2, 3].map(AttachmentId::nth));
    h.finish();
}

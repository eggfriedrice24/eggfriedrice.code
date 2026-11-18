use std::collections::VecDeque;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;
use efr_protocol::{
    Attachment, AttachmentId, AttachmentKind, AttachmentOrigin, ConversationId, Event, SentForm,
    UploadId, WithheldReason,
};
use efr_provider::{
    AttachmentPart, ContentBlock, EncodedBytes, InputKinds, Message, ModelInfo, Provider,
    ProviderError, ProviderEvent, ProviderId, ProviderStream, Request, Role, StopReason,
};
use efr_store::attachments::Summary;
use pretty_assertions::assert_eq;
use serde_json::json;

use super::*;
use crate::context::{PRUNED_OUTPUT_STUB, message_tokens};
use crate::testing::{MODEL, Setup, done, text_answer};
use crate::{ToolImage, ToolOutcome};

/// The image `n` that a tool read, as efrd keeps it: a blob of 64 x 32 pixels.
fn image(n: u32) -> Attachment {
    Attachment {
        id: AttachmentId::nth(n),
        name: format!("img-{n}.png"),
        media_type: "image/png".to_owned(),
        bytes: 9_000,
        sha256: "ab".repeat(32),
        kind: AttachmentKind::Image { width: 64, height: 32 },
        origin: AttachmentOrigin::Tool { path: format!("/home/u/img-{n}.png").into() },
        copy: None,
        sent: Some(SentForm {
            media_type: "image/png".to_owned(),
            bytes: 9_000,
            width: Some(64),
            height: Some(32),
        }),
    }
}

fn both() -> InputKinds {
    InputKinds { image: true, pdf: true }
}

#[test]
fn a_model_reads_only_its_kinds_and_never_a_withheld_part_or_one_with_no_blob() {
    let summary = Summary::new(4, vec![AttachmentId::nth(2)]);
    let turn = TurnAttachments::new(&summary, InputKinds { image: true, pdf: false });
    let pdf = Attachment { kind: AttachmentKind::Pdf { pages: Some(2) }, ..image(3) };
    let no_blob = Attachment { sent: None, ..image(4) };
    assert!(turn.reads(&image(1)));
    assert!(!turn.reads(&image(2)), "withheld");
    assert!(!turn.reads(&pdf), "the model reads no PDF");
    assert!(!turn.reads(&no_blob));
    assert_eq!(
        turn.unread(&[image(1), image(2), pdf, no_blob]),
        [AttachmentId::nth(2), AttachmentId::nth(3), AttachmentId::nth(4)]
    );
}

#[test]
fn the_next_id_follows_the_highest_one_of_the_conversation() {
    let mut turn = TurnAttachments::new(&Summary::new(2, Vec::new()), both());
    turn.seen([&AttachmentId::nth(5), &AttachmentId::nth(3)]);
    assert_eq!(turn.next_id(), AttachmentId::nth(6));
    assert!(turn.any());
    assert!(!TurnAttachments::new(&Summary::default(), both()).any());
}

#[test]
fn a_refusal_withholds_the_new_parts_first_and_then_all() {
    let mut turn = TurnAttachments::new(&Summary::default(), both());
    turn.carried(vec![AttachmentId::nth(1)]);
    let sent = [AttachmentId::nth(1), AttachmentId::nth(2)];
    assert_eq!(turn.refused(&sent, true), [AttachmentId::nth(2)]);
    assert_eq!(turn.refused(&sent, false), sent);
    assert_eq!(turn.refused(&[AttachmentId::nth(1)], true), [AttachmentId::nth(1)], "no new part");
}

#[test]
fn only_a_400_or_a_413_of_the_api_can_be_a_refused_attachment() {
    let api =
        |status| ProviderError::Api { status: Some(status), code: None, message: "no".into() };
    assert!(refused(&api(400)));
    assert!(refused(&api(413)));
    assert!(!refused(&api(500)));
    assert!(!refused(&ProviderError::api(Some(413), None, "prompt is too long".into())));
    assert!(!refused(&ProviderError::NotLoggedIn));
}

#[test]
fn a_tool_note_says_why_the_model_does_not_see_an_image() {
    assert_eq!(tool_note(&image(1)), None);
    let large = Attachment {
        kind: AttachmentKind::Image { width: 9000, height: 8000 },
        sent: None,
        ..image(1)
    };
    assert_eq!(tool_note(&large).as_deref(), Some("[efr: too large to show; 9000x8000]"));
    let broken = Attachment { kind: AttachmentKind::Other, sent: None, ..image(1) };
    assert_eq!(tool_note(&broken).as_deref(), Some(BROKEN_IMAGE));
}

#[test]
fn the_user_messages_of_the_reviewer_and_the_naming_rule_do_not_read_an_attachment() {
    let turn: efr_protocol::TurnId =
        "00000002-0000-7000-8000-000000000001".parse().expect("turn id");
    let page = |attachments: Vec<Attachment>| {
        let queued = Event::PromptQueued {
            turn_id: turn,
            command_id: "00000003-0000-7000-8000-000000000001".parse().expect("command id"),
            text: "fix the build".to_owned(),
            origin: efr_protocol::Origin::Shell,
            context: None,
            settings: efr_protocol::TurnSettings::default(),
            steers: Vec::new(),
            attachments,
        };
        vec![efr_protocol::EventEnvelope {
            seq: efr_protocol::Seq::new(1),
            conversation_id: None,
            at: jiff::Timestamp::UNIX_EPOCH,
            event: queued,
        }]
    };
    let named = Attachment { name: "rm -rf ~ now.png".to_owned(), ..image(1) };
    assert_eq!(
        crate::exit::UserMessages::of_turn(&page(vec![named]), turn),
        crate::exit::UserMessages::of_turn(&page(Vec::new()), turn)
    );
}

fn with_data(attachment: Attachment) -> AttachmentPart {
    AttachmentPart { attachment, data: Some(EncodedBytes::new("AAAA")) }
}

fn result(output: &str, parts: Vec<AttachmentPart>) -> ContentBlock {
    ContentBlock::ToolResult {
        call_id: "call_1".to_owned(),
        output: output.to_owned(),
        is_error: false,
        attachments: parts,
    }
}

#[test]
fn a_pruned_result_has_no_part_that_can_go_as_content_and_strip_drops_every_data() {
    let mut message = Message::new(
        Role::User,
        vec![
            ContentBlock::Attachment(with_data(image(1))),
            result("img-2.png: PNG image, 9 KB", vec![with_data(image(2))]),
            result(PRUNED_OUTPUT_STUB, vec![with_data(image(3))]),
        ],
    );
    let ids: Vec<AttachmentId> =
        parts_mut(&mut message).into_iter().map(|part| part.attachment.id.clone()).collect();
    assert_eq!(ids, [AttachmentId::nth(1), AttachmentId::nth(2)]);

    clear(std::iter::once(&mut message), &[AttachmentId::nth(2)]);
    assert_eq!(super::with_data(std::iter::once(&mut message)), [AttachmentId::nth(1)]);
    strip(&mut message);
    let mut all = message.clone();
    let none = all.content.iter_mut().all(|block| match block {
        ContentBlock::Attachment(part) => part.data.is_none(),
        ContentBlock::ToolResult { attachments, .. } => {
            attachments.iter().all(|p| p.data.is_none())
        }
        _ => true,
    });
    assert!(none, "{message:?}");
}

#[test]
fn a_3_mb_image_counts_under_5000_tokens() {
    let large = Attachment {
        bytes: 3_000_000,
        kind: AttachmentKind::Image { width: 4000, height: 2250 },
        sent: Some(SentForm {
            media_type: "image/jpeg".to_owned(),
            bytes: 700_000,
            width: Some(2000),
            height: Some(1125),
        }),
        ..image(1)
    };
    let part =
        AttachmentPart { attachment: large, data: Some(EncodedBytes::new("A".repeat(4_000_000))) };
    let message = Message::new(Role::User, vec![ContentBlock::Attachment(part.clone())]);
    let tokens = message_tokens(&message);
    assert!((2_000..5_000).contains(&tokens), "{tokens}");
    let label = Message::new(
        Role::User,
        vec![ContentBlock::Attachment(AttachmentPart::new(part.attachment))],
    );
    assert!(message_tokens(&label) < 200, "a label counts its text only");
}

/// The store of a test: it keeps each image of a tool as a blob of 64 x 32 pixels, but
/// one whose first byte is `B` (broken) or `L` (too large), and gives `AAAA` as the
/// bytes of every blob.
#[derive(Debug, Default)]
pub(crate) struct FakeAttachments {
    loads: AtomicUsize,
}

#[async_trait]
impl Attachments for FakeAttachments {
    async fn bind(
        &self,
        _conversation_id: ConversationId,
        _id: AttachmentId,
        _upload: UploadId,
        _scratch: &Path,
    ) -> Option<Attachment> {
        None
    }

    async fn keep(
        &self,
        _conversation_id: ConversationId,
        id: AttachmentId,
        path: &Path,
        bytes: Vec<u8>,
    ) -> Option<Attachment> {
        let n = id.number()?;
        let origin = AttachmentOrigin::Tool { path: path.to_path_buf() };
        let kept = Attachment { id, origin, ..image(n) };
        Some(match bytes.first() {
            Some(b'B') => Attachment { kind: AttachmentKind::Other, sent: None, ..kept },
            Some(b'L') => Attachment {
                kind: AttachmentKind::Image { width: 9000, height: 9000 },
                sent: None,
                ..kept
            },
            _ => kept,
        })
    }

    async fn load(
        &self,
        _conversation_id: ConversationId,
        _attachment: &Attachment,
    ) -> Option<EncodedBytes> {
        self.loads.fetch_add(1, Ordering::Relaxed);
        Some(EncodedBytes::new("AAAA"))
    }
}

/// A model that reads `inputs`, answers each request with the next answer of its
/// script, and keeps every request.
#[derive(Debug)]
struct Scripted {
    id: ProviderId,
    inputs: InputKinds,
    script: Mutex<VecDeque<Result<Vec<ProviderEvent>, ProviderError>>>,
    requests: Mutex<Vec<Request>>,
}

impl Scripted {
    fn new(
        inputs: InputKinds,
        script: Vec<Result<Vec<ProviderEvent>, ProviderError>>,
    ) -> Arc<Self> {
        Arc::new(Scripted {
            id: ProviderId::new("replay").expect("provider id"),
            inputs,
            script: Mutex::new(script.into()),
            requests: Mutex::new(Vec::new()),
        })
    }

    fn requests(&self) -> Vec<Request> {
        self.requests.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

#[async_trait]
impl Provider for Scripted {
    fn id(&self) -> &ProviderId {
        &self.id
    }

    fn models(&self) -> Vec<ModelInfo> {
        vec![ModelInfo::new(MODEL).with_inputs(self.inputs)]
    }

    async fn stream(&self, request: Request) -> Result<ProviderStream, ProviderError> {
        self.requests.lock().unwrap_or_else(PoisonError::into_inner).push(request);
        let next = self.script.lock().unwrap_or_else(PoisonError::into_inner).pop_front();
        let events = next.unwrap_or_else(|| Ok(text_answer("done")))?;
        Ok(Box::pin(futures::stream::iter(events.into_iter().map(Ok))))
    }
}

/// An answer that reads each of `paths` with `read_file`.
fn reads(paths: &[String]) -> Vec<ProviderEvent> {
    let mut events = Vec::new();
    for (n, path) in paths.iter().enumerate() {
        let call_id = format!("call_{n}");
        events.push(ProviderEvent::ToolCallStart {
            call_id: call_id.clone(),
            name: "read_file".to_owned(),
            freeform: false,
        });
        events.push(ProviderEvent::ToolCallEnd {
            call_id,
            arguments: json!({ "path": path }).to_string(),
        });
    }
    events.push(done(StopReason::ToolUse, None));
    events
}

fn bad_request() -> ProviderError {
    ProviderError::Api { status: Some(400), code: None, message: "invalid image".to_owned() }
}

/// The parts of the tool results of `request`: each id, and whether it has data.
fn result_parts(request: &Request) -> Vec<(u32, bool)> {
    request
        .messages
        .iter()
        .flat_map(|message| &message.content)
        .flat_map(|block| match block {
            ContentBlock::ToolResult { attachments, .. } => attachments.as_slice(),
            _ => &[],
        })
        .map(|part| (part.attachment.id.number().unwrap_or(0), part.data.is_some()))
        .collect()
}

/// A setup with the fake store whose toolbox reads each of `names` in the project as an
/// image whose bytes are the name.
fn images(names: &[&str]) -> (Setup, Vec<String>) {
    let mut setup = Setup::new();
    setup.attachments = Arc::new(FakeAttachments::default());
    let paths = names.iter().map(|name| setup.cwd.join(name).display().to_string()).collect();
    (setup, paths)
}

/// Makes the toolbox of `h` answer a read of each of `paths` with the image `bytes`.
fn serve(h: &crate::testing::Harness, paths: &[String], bytes: &[u8]) {
    let mut outcomes = h.toolbox.outcomes.lock().unwrap_or_else(PoisonError::into_inner);
    for path in paths {
        let outcome = ToolOutcome::ok("img.png: PNG image, 9 B")
            .with_media(vec![ToolImage { path: path.into(), bytes: bytes.to_vec() }]);
        outcomes.insert(path.clone(), outcome);
    }
}

async fn withheld_events(h: &crate::testing::Harness) -> Vec<(Vec<AttachmentId>, WithheldReason)> {
    h.events()
        .await
        .into_iter()
        .filter_map(|event| match event {
            Event::AttachmentsWithheld { attachments, reason, .. } => Some((attachments, reason)),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn an_image_that_read_file_read_goes_to_the_model_as_content_and_into_the_log() {
    let (setup, paths) = images(&["shot.png"]);
    let model = Scripted::new(both(), vec![Ok(reads(&paths))]);
    let mut h = setup.start_model(model.clone()).await;
    serve(&h, &paths, b"\x89PNG");
    let sent = h.prompt("look at the screenshot").await;
    let end = h.wait_end(sent.turn_id).await;
    assert!(matches!(end, Event::TurnCompleted { .. }), "{end:?}");

    let requests = model.requests();
    assert_eq!(result_parts(&requests[1]), [(1, true)]);
    let recorded = h.events().await.into_iter().find_map(|event| match event {
        Event::ToolCallCompleted { attachments, .. } => Some(attachments),
        _ => None,
    });
    assert_eq!(recorded.map(|list| list.len()), Some(1));
}

#[tokio::test]
async fn a_model_that_reads_no_image_gets_the_label() {
    let (setup, paths) = images(&["shot.png"]);
    let model = Scripted::new(InputKinds::default(), vec![Ok(reads(&paths))]);
    let mut h = setup.start_model(model.clone()).await;
    serve(&h, &paths, b"\x89PNG");
    let sent = h.prompt("look").await;
    h.wait_end(sent.turn_id).await;
    assert_eq!(result_parts(&model.requests()[1]), [(1, false)]);
}

#[tokio::test]
async fn a_broken_or_too_large_image_says_so_in_the_result() {
    let (setup, paths) = images(&["a.png", "b.png"]);
    let model = Scripted::new(both(), vec![Ok(reads(&paths))]);
    let mut h = setup.start_model(model.clone()).await;
    serve(&h, &paths[..1], b"Broken");
    serve(&h, &paths[1..], b"Large");
    let sent = h.prompt("look").await;
    h.wait_end(sent.turn_id).await;
    let outputs: Vec<String> = model.requests()[1]
        .messages
        .iter()
        .flat_map(|message| &message.content)
        .filter_map(|block| match block {
            ContentBlock::ToolResult { output, .. } => Some(output.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        outputs,
        [
            format!("img.png: PNG image, 9 B\n{BROKEN_IMAGE}"),
            "img.png: PNG image, 9 B\n[efr: too large to show; 9000x9000]".to_owned(),
        ]
    );
}

#[tokio::test]
async fn a_refused_image_goes_as_a_label_and_the_turn_and_the_next_prompt_work() {
    let (setup, paths) = images(&["shot.png"]);
    let model = Scripted::new(both(), vec![Ok(reads(&paths)), Err(bad_request())]);
    let mut h = setup.start_model(model.clone()).await;
    serve(&h, &paths, b"\x89PNG");
    let sent = h.prompt("look").await;
    let end = h.wait_end(sent.turn_id).await;
    assert!(matches!(end, Event::TurnCompleted { .. }), "{end:?}");

    let requests = model.requests();
    assert_eq!(result_parts(&requests[1]), [(1, true)]);
    assert_eq!(result_parts(&requests[2]), [(1, false)], "sent once more with the label");
    assert_eq!(withheld_events(&h).await, [(vec![AttachmentId::nth(1)], WithheldReason::Refused)]);

    let next = h.prompt("and now?").await;
    h.wait_end(next.turn_id).await;
    let requests = model.requests();
    assert_eq!(
        result_parts(requests.last().unwrap()),
        [(1, false)],
        "the next prompt sends the label"
    );
}

#[tokio::test]
async fn a_second_refusal_withholds_all_and_a_third_fails_the_turn() {
    let (setup, paths) = images(&["one.png", "two.png"]);
    let first = reads(&paths[..1]);
    let second = reads(&paths[1..]);
    let script =
        vec![Ok(first), Ok(second), Err(bad_request()), Err(bad_request()), Err(bad_request())];
    let model = Scripted::new(both(), script);
    let mut h = setup.start_model(model.clone()).await;
    serve(&h, &paths, b"\x89PNG");
    let sent = h.prompt("look").await;
    let end = h.wait_end(sent.turn_id).await;
    assert!(matches!(end, Event::TurnFailed { .. }), "{end:?}");

    assert_eq!(
        withheld_events(&h).await,
        [
            (vec![AttachmentId::nth(2)], WithheldReason::Refused),
            (vec![AttachmentId::nth(1)], WithheldReason::Refused),
        ],
        "the image that a good call carried goes last"
    );
    assert_eq!(model.requests().len(), 5);
}

#[tokio::test]
async fn an_overflow_a_rate_limit_and_a_login_error_start_no_recovery() {
    let errors = [
        ProviderError::NotLoggedIn,
        ProviderError::RateLimited { retry_after: None, limit: None },
        ProviderError::Api { status: Some(500), code: None, message: "down".to_owned() },
    ];
    for error in errors {
        let (setup, paths) = images(&["shot.png"]);
        let model = Scripted::new(both(), vec![Ok(reads(&paths)), Err(error)]);
        let mut h = setup.start_model(model.clone()).await;
        serve(&h, &paths, b"\x89PNG");
        let sent = h.prompt("look").await;
        h.wait_end(sent.turn_id).await;
        assert_eq!(withheld_events(&h).await, []);
    }
}

#[tokio::test]
async fn the_budget_withholds_the_oldest_images_once_for_91_and_not_again_for_92() {
    let names: Vec<String> = (1..=92).map(|n| format!("img-{n}.png")).collect();
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    let (setup, paths) = images(&names);
    let model = Scripted::new(both(), vec![Ok(reads(&paths[..91])), Ok(reads(&paths[91..]))]);
    let mut h = setup.start_model(model.clone()).await;
    serve(&h, &paths, b"\x89PNG");
    let sent = h.prompt("look at all").await;
    h.wait_end(sent.turn_id).await;

    let requests = model.requests();
    let with_data =
        |request: &Request| result_parts(request).iter().filter(|(_, data)| *data).count();
    assert_eq!(with_data(&requests[1]), 45);
    assert_eq!(with_data(&requests[2]), 46);
    let withheld = withheld_events(&h).await;
    assert_eq!(withheld.len(), 1, "{withheld:?}");
    assert_eq!(withheld[0].0, (1..=46).map(AttachmentId::nth).collect::<Vec<_>>());
    assert_eq!(withheld[0].1, WithheldReason::Budget);
}

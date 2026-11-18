use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use efr_config::Settings;
use efr_conversation::Attachments as _;
use efr_protocol::{
    AttachmentId, AttachmentKind, AttachmentOrigin, CommandId, ConversationId, Event, Origin,
    TurnId, TurnSettings, UploadId,
};
use efr_sandbox::ATTACH_DIR;
use efr_store::Batch;
use efr_test_support::{TestClock, TestStore};
use efr_transport::ConnId;
use pretty_assertions::assert_eq;
use tokio::sync::{Semaphore, watch};
use uuid::Uuid;

use super::DaemonAttachments;
use super::store::ATTACHMENTS_DIR;
use super::uploads::{Chunk, Owner, Uploads};

/// A PNG of 1 x 1 pixel with alpha, the smallest whole image.
pub(crate) const PIXEL_PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0\0\x01\0\0\0\x01\x08\x06\0\0\0\x1f\x15\xc4\x89\0\0\0\nIDATx\x9cc\0\x01\0\0\x05\0\x01\r\n-\xb4\0\0\0\0IEND\xaeB`\x82";

fn conversation() -> ConversationId {
    ConversationId::from_uuid(Uuid::from_u128(7))
}

/// The blob dir of [`conversation`] below the data dir of `root`.
fn blob_dir(root: &Path) -> PathBuf {
    root.join("data").join(ATTACHMENTS_DIR).join(conversation().to_string())
}

/// The attachments of efrd with its data and state dirs below `root`, and the store
/// whose rows name the blobs.
struct Daemon {
    attachments: DaemonAttachments,
    uploads: Uploads,
    rows: TestStore,
}

async fn daemon(root: &Path, enabled: bool) -> Daemon {
    let mut settings = Settings::default();
    settings.attachments.enabled = enabled;
    let clock = TestClock::new();
    let rows = TestStore::open(clock.shared()).await.unwrap();
    let decode = Arc::new(Semaphore::new(1));
    let uploads = Uploads::new(&root.join("state"), clock.shared(), Arc::clone(&decode));
    let attachments = DaemonAttachments::new(
        &root.join("data"),
        watch::channel(Arc::new(settings)).1,
        uploads.clone(),
        rows.readers().clone(),
        decode,
    );
    Daemon { attachments, uploads, rows }
}

async fn store(root: &Path, enabled: bool) -> DaemonAttachments {
    daemon(root, enabled).await.attachments
}

/// The finished upload `upload` of `bytes` from the `@path` `/home/u/<name>`.
async fn upload(uploads: &Uploads, upload: UploadId, name: &str, bytes: &[u8]) {
    let chunk = Chunk {
        upload_id: upload,
        name: name.to_owned(),
        total_bytes: bytes.len() as u64,
        offset: 0,
        data: bytes.to_vec(),
        origin: AttachmentOrigin::Path { path: Path::new("/home/u").join(name) },
    };
    let owner = Owner::Connection(ConnId::new(1));
    let put = uploads.put(owner, chunk, 2000).await.unwrap();
    assert!(put.upload.is_some(), "finished");
}

#[tokio::test]
async fn a_bound_upload_gets_a_blob_and_a_copy_with_the_original_bytes() {
    let root = tempfile::tempdir().unwrap();
    let daemon = daemon(root.path(), true).await;
    let scratch = root.path().join("scratch");
    std::fs::create_dir(&scratch).unwrap();
    let id = UploadId::from_uuid(Uuid::from_u128(1));
    upload(&daemon.uploads, id, "shot.png", PIXEL_PNG).await;

    let bound = daemon.attachments.bind(conversation(), AttachmentId::nth(3), id, &scratch).await;
    let bound = bound.expect("bound");
    let copy = scratch.join(ATTACH_DIR).join("a3-shot.png");
    assert_eq!(bound.copy.as_deref(), Some(copy.as_path()));
    assert_eq!(std::fs::read(&copy).unwrap(), PIXEL_PNG);
    assert_eq!(bound.kind, AttachmentKind::Image { width: 1, height: 1 });
    assert_eq!(bound.origin, AttachmentOrigin::Path { path: "/home/u/shot.png".into() });
    assert!(blob_dir(root.path()).join(&bound.sha256).is_file());
    assert!(daemon.attachments.load(conversation(), &bound).await.is_some());
    // NOTE: the upload stays until the prompt that took it is queued.
    assert!(daemon.uploads.finished(id).is_some());
    let unknown = UploadId::from_uuid(Uuid::from_u128(2));
    assert_eq!(
        daemon.attachments.bind(conversation(), AttachmentId::nth(4), unknown, &scratch).await,
        None
    );
}

#[tokio::test]
async fn a_forgotten_prompt_leaves_no_copy_and_no_blob_that_no_row_names() {
    let root = tempfile::tempdir().unwrap();
    let daemon = daemon(root.path(), true).await;
    let scratch = root.path().join("scratch");
    std::fs::create_dir(&scratch).unwrap();
    let id = UploadId::from_uuid(Uuid::from_u128(1));
    upload(&daemon.uploads, id, "shot.png", PIXEL_PNG).await;
    let bind = |n| daemon.attachments.bind(conversation(), AttachmentId::nth(n), id, &scratch);
    let (first, kept) = (bind(1).await.unwrap(), bind(2).await.unwrap());
    let blob = blob_dir(root.path()).join(&first.sha256);
    let created = Event::ConversationCreated { origin: Origin::Shell, tty: None, route: None };
    let queued = Event::PromptQueued {
        turn_id: TurnId::from_uuid(Uuid::from_u128(3)),
        command_id: CommandId::from_uuid(Uuid::from_u128(4)),
        text: "what is this".to_owned(),
        origin: Origin::Shell,
        context: None,
        settings: TurnSettings::default(),
        steers: Vec::new(),
        attachments: vec![kept.clone()],
    };
    let batch = Batch::new().event(conversation(), created).event(conversation(), queued);
    daemon.rows.writer().append(batch).await.unwrap();

    daemon.attachments.forget(conversation(), std::slice::from_ref(&first)).await;
    assert!(!first.copy.as_deref().unwrap().exists());
    assert!(kept.copy.as_deref().unwrap().is_file());
    assert!(blob.is_file(), "the row of a2 names the blob");

    daemon.rows.writer().append(Batch::new().event(conversation(), withdrawn())).await.unwrap();
    daemon.attachments.forget(conversation(), std::slice::from_ref(&kept)).await;
    assert!(!kept.copy.as_deref().unwrap().exists());
    assert!(!blob.exists());
}

/// The `prompt_withdrawn` of the prompt of turn 3.
fn withdrawn() -> Event {
    Event::PromptWithdrawn { turn_id: TurnId::from_uuid(Uuid::from_u128(3)), origin: Origin::Shell }
}

#[tokio::test]
async fn an_image_of_a_tool_gets_a_private_blob_whose_base64_a_model_gets() {
    let data = tempfile::tempdir().unwrap();
    let store = store(data.path(), true).await;
    let path = Path::new("/home/u/shot.png");
    let kept = store.keep(conversation(), AttachmentId::nth(1), path, PIXEL_PNG.to_vec()).await;
    let kept = kept.expect("kept");
    assert_eq!(kept.id, AttachmentId::nth(1));
    assert_eq!(kept.name, "shot.png");
    assert_eq!(kept.kind, AttachmentKind::Image { width: 1, height: 1 });
    assert_eq!(kept.origin, AttachmentOrigin::Tool { path: path.to_path_buf() });
    assert_eq!(kept.bytes, PIXEL_PNG.len() as u64);
    assert_eq!(kept.copy, None);

    let dir = blob_dir(data.path());
    let blob = dir.join(&kept.sha256);
    assert_eq!(std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
    assert_eq!(std::fs::metadata(&blob).unwrap().permissions().mode() & 0o777, 0o600);
    let loaded = store.load(conversation(), &kept).await.expect("loaded");
    assert_eq!(loaded.as_str(), STANDARD.encode(std::fs::read(&blob).unwrap()));

    // NOTE: the cache answers a second load: the blob is not read again.
    std::fs::remove_file(&blob).unwrap();
    assert_eq!(store.load(conversation(), &kept).await, Some(loaded));
}

#[tokio::test]
async fn the_same_bytes_twice_give_two_ids_and_one_blob() {
    let data = tempfile::tempdir().unwrap();
    let store = store(data.path(), true).await;
    let path = Path::new("/home/u/shot.png");
    let first = store.keep(conversation(), AttachmentId::nth(1), path, PIXEL_PNG.to_vec()).await;
    let second = store.keep(conversation(), AttachmentId::nth(2), path, PIXEL_PNG.to_vec()).await;
    let (first, second) = (first.unwrap(), second.unwrap());
    assert_eq!(first.sha256, second.sha256);
    assert_ne!(first.id, second.id);
    assert_eq!(std::fs::read_dir(blob_dir(data.path())).unwrap().count(), 1);
}

#[tokio::test]
async fn a_broken_image_gets_no_blob_and_loads_no_bytes() {
    let data = tempfile::tempdir().unwrap();
    let store = store(data.path(), true).await;
    let path = Path::new("/home/u/cut.png");
    let kept =
        store.keep(conversation(), AttachmentId::nth(1), path, PIXEL_PNG[..40].to_vec()).await;
    let kept = kept.expect("kept as a path");
    assert_eq!((kept.kind, kept.sent.clone()), (AttachmentKind::Other, None));
    assert!(!blob_dir(data.path()).exists());
    assert_eq!(store.load(conversation(), &kept).await, None);
}

#[tokio::test]
async fn with_attachments_off_no_image_is_kept() {
    let data = tempfile::tempdir().unwrap();
    let store = store(data.path(), false).await;
    let kept = store
        .keep(conversation(), AttachmentId::nth(1), Path::new("/x.png"), PIXEL_PNG.to_vec())
        .await;
    assert_eq!(kept, None);
}

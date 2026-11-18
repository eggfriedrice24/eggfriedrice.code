use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;
use std::sync::Arc;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use efr_config::Settings;
use efr_conversation::Attachments as _;
use efr_protocol::{AttachmentId, AttachmentKind, AttachmentOrigin, ConversationId};
use pretty_assertions::assert_eq;
use tokio::sync::watch;
use uuid::Uuid;

use super::DaemonAttachments;
use super::store::ATTACHMENTS_DIR;

/// A PNG of 1 x 1 pixel with alpha, the smallest whole image.
pub(crate) const PIXEL_PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0\0\x01\0\0\0\x01\x08\x06\0\0\0\x1f\x15\xc4\x89\0\0\0\nIDATx\x9cc\0\x01\0\0\x05\0\x01\r\n-\xb4\0\0\0\0IEND\xaeB`\x82";

fn conversation() -> ConversationId {
    ConversationId::from_uuid(Uuid::from_u128(7))
}

fn store(data: &Path, enabled: bool) -> DaemonAttachments {
    let mut settings = Settings::default();
    settings.attachments.enabled = enabled;
    DaemonAttachments::new(data, watch::channel(Arc::new(settings)).1)
}

#[tokio::test]
async fn an_image_of_a_tool_gets_a_private_blob_whose_base64_a_model_gets() {
    let data = tempfile::tempdir().unwrap();
    let store = store(data.path(), true);
    let path = Path::new("/home/u/shot.png");
    let kept = store.keep(conversation(), AttachmentId::nth(1), path, PIXEL_PNG.to_vec()).await;
    let kept = kept.expect("kept");
    assert_eq!(kept.id, AttachmentId::nth(1));
    assert_eq!(kept.name, "shot.png");
    assert_eq!(kept.kind, AttachmentKind::Image { width: 1, height: 1 });
    assert_eq!(kept.origin, AttachmentOrigin::Tool { path: path.to_path_buf() });
    assert_eq!(kept.bytes, PIXEL_PNG.len() as u64);
    assert_eq!(kept.copy, None);

    let dir = data.path().join(ATTACHMENTS_DIR).join(conversation().to_string());
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
    let store = store(data.path(), true);
    let path = Path::new("/home/u/shot.png");
    let first = store.keep(conversation(), AttachmentId::nth(1), path, PIXEL_PNG.to_vec()).await;
    let second = store.keep(conversation(), AttachmentId::nth(2), path, PIXEL_PNG.to_vec()).await;
    let (first, second) = (first.unwrap(), second.unwrap());
    assert_eq!(first.sha256, second.sha256);
    assert_ne!(first.id, second.id);
    let dir = data.path().join(ATTACHMENTS_DIR).join(conversation().to_string());
    assert_eq!(std::fs::read_dir(dir).unwrap().count(), 1);
}

#[tokio::test]
async fn a_broken_image_gets_no_blob_and_loads_no_bytes() {
    let data = tempfile::tempdir().unwrap();
    let store = store(data.path(), true);
    let path = Path::new("/home/u/cut.png");
    let kept =
        store.keep(conversation(), AttachmentId::nth(1), path, PIXEL_PNG[..40].to_vec()).await;
    let kept = kept.expect("kept as a path");
    assert_eq!((kept.kind, kept.sent.clone()), (AttachmentKind::Other, None));
    assert!(!data.path().join(ATTACHMENTS_DIR).exists());
    assert_eq!(store.load(conversation(), &kept).await, None);
}

#[tokio::test]
async fn with_attachments_off_no_image_is_kept() {
    let data = tempfile::tempdir().unwrap();
    let store = store(data.path(), false);
    let kept = store
        .keep(conversation(), AttachmentId::nth(1), Path::new("/x.png"), PIXEL_PNG.to_vec())
        .await;
    assert_eq!(kept, None);
}

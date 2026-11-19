use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use efr_protocol::{
    AttachmentKind, AttachmentOrigin, DeviceId, MAX_OPEN_UPLOADS, UPLOAD_TTL, UploadId,
};
use efr_test_support::{TestClock, Wait};
use efr_transport::ConnId;
use pretty_assertions::assert_eq;
use tokio::sync::Semaphore;
use uuid::Uuid;

use super::{Chunk, Owner, UPLOADS_DIR, UploadError, Uploads};
use crate::attachments::tests::PIXEL_PNG;

const LOCAL: Owner = Owner::Connection(ConnId::new(1));

fn phone() -> Owner {
    Owner::Device(DeviceId::from_uuid(Uuid::from_u128(9)))
}

fn upload(n: u128) -> UploadId {
    UploadId::from_uuid(Uuid::from_u128(n))
}

fn uploads(state: &Path, clock: &TestClock) -> Uploads {
    Uploads::new(state, clock.shared(), Arc::new(Semaphore::new(1)))
}

/// The chunk of `file` at `offset` with `len` bytes, of the upload `n`.
fn chunk(n: u128, file: &[u8], offset: usize, len: usize) -> Chunk {
    Chunk {
        upload_id: upload(n),
        name: "notes.txt".to_owned(),
        total_bytes: file.len() as u64,
        offset: offset as u64,
        data: file[offset..offset + len].to_vec(),
        origin: AttachmentOrigin::Clipboard,
    }
}

fn part(state: &Path, n: u128) -> PathBuf {
    state.join(UPLOADS_DIR).join(format!("{}.part", upload(n)))
}

/// Waits until the blocking task that removes the files of an upload removed `path`.
async fn gone(path: &Path) {
    Wait::new("the file of an upload goes").until(|| !path.exists()).await.unwrap();
}

#[tokio::test]
async fn a_repeat_a_gap_and_a_resume_all_go_on_at_received() {
    let state = tempfile::tempdir().unwrap();
    let uploads = uploads(state.path(), &TestClock::new());
    let file = b"0123456789";
    let put = |offset, len| uploads.put(phone(), chunk(1, file, offset, len), 2000);

    assert_eq!(put(0, 4).await.unwrap().received, 4);
    assert_eq!(put(0, 4).await.unwrap().received, 4, "a repeated chunk writes nothing");
    assert_eq!(put(6, 4).await.unwrap().received, 4, "a gap writes nothing");
    assert_eq!(put(4, 0).await.unwrap().received, 4, "an empty chunk asks for received");
    let done = put(4, 6).await.unwrap();
    assert_eq!(done.received, 10);
    let info = done.upload.clone().expect("finished");
    assert_eq!((info.media_type.as_str(), info.kind), ("text/plain", AttachmentKind::Text));
    assert_eq!((info.bytes, info.sent), (10, None));
    assert_eq!(std::fs::read(part(state.path(), 1)).unwrap(), file);
    // NOTE: a lost answer of the last chunk: the repeat gives the same result.
    assert_eq!(put(4, 6).await.unwrap(), done);
}

#[tokio::test]
async fn an_image_gets_what_a_model_reads_and_waits_for_a_prompt() {
    let state = tempfile::tempdir().unwrap();
    let uploads = uploads(state.path(), &TestClock::new());
    let done = uploads.put(LOCAL, chunk(1, PIXEL_PNG, 0, PIXEL_PNG.len()), 2000).await.unwrap();
    let info = done.upload.expect("finished");
    assert_eq!(info.kind, AttachmentKind::Image { width: 1, height: 1 });
    assert!(info.sent.is_some());
    let finished = uploads.finished(upload(1)).expect("finished");
    assert!(finished.sent_file.as_deref().is_some_and(Path::is_file));
    assert_eq!(finished.sha256, crate::judge::sha256_hex(PIXEL_PNG));
}

#[tokio::test]
async fn a_pdf_waits_in_its_part_with_no_sent_file() {
    let state = tempfile::tempdir().unwrap();
    let uploads = uploads(state.path(), &TestClock::new());
    let pdf = b"%PDF-1.4\n2 0 obj\n<< /Type /Pages /Kids [] /Count 1 >>\nendobj\n%%EOF\n";
    let done = uploads.put(LOCAL, chunk(1, pdf, 0, pdf.len()), 2000).await.unwrap();
    assert_eq!(done.upload.expect("finished").kind, AttachmentKind::Pdf { pages: Some(1) });
    let finished = uploads.finished(upload(1)).expect("finished");
    assert_eq!(finished.sent_file.as_deref(), Some(part(state.path(), 1).as_path()));
    assert!(!state.path().join(UPLOADS_DIR).join(format!("{}.sent", upload(1))).exists());
}

#[tokio::test]
async fn a_text_file_longer_than_the_head_is_hashed_whole() {
    let state = tempfile::tempdir().unwrap();
    let uploads = uploads(state.path(), &TestClock::new());
    let text = "0123456789abcdef\n".repeat(20_000).into_bytes();
    let done = uploads.put(LOCAL, chunk(1, &text, 0, text.len()), 2000).await.unwrap();
    let info = done.upload.expect("finished");
    assert_eq!((info.kind, info.bytes), (AttachmentKind::Text, text.len() as u64));
    let finished = uploads.finished(upload(1)).expect("finished");
    assert_eq!(finished.sha256, crate::judge::sha256_hex(&text));
    assert_eq!(finished.sent_file, None);
}

#[tokio::test]
async fn the_intake_of_a_whole_upload_waits_for_the_one_decode() {
    let state = tempfile::tempdir().unwrap();
    let decode = Arc::new(Semaphore::new(1));
    let uploads = Uploads::new(state.path(), TestClock::new().shared(), Arc::clone(&decode));
    let other = decode.acquire().await.unwrap();

    let put = uploads.put(LOCAL, chunk(1, PIXEL_PNG, 0, PIXEL_PNG.len()), 2000);
    tokio::pin!(put);
    // NOTE: real time; while another decode runs, the intake never ends.
    let waited = tokio::time::timeout(Duration::from_millis(200), &mut put).await;
    assert!(waited.is_err(), "the intake waits while another decode runs");
    assert!(uploads.finished(upload(1)).is_none());

    drop(other);
    let done = put.await.unwrap();
    assert!(done.upload.is_some(), "the intake ends after the other decode");
    assert_eq!(decode.available_permits(), 1, "the intake gives the decode back");
}

#[tokio::test]
async fn an_empty_file_is_whole_at_its_first_chunk() {
    let state = tempfile::tempdir().unwrap();
    let uploads = uploads(state.path(), &TestClock::new());
    let done = uploads.put(LOCAL, chunk(1, b"", 0, 0), 2000).await.unwrap();
    assert_eq!(done.upload.map(|info| info.bytes), Some(0));
}

#[tokio::test]
async fn only_the_owner_adds_drops_or_names_an_upload() {
    let state = tempfile::tempdir().unwrap();
    let uploads = uploads(state.path(), &TestClock::new());
    uploads.put(phone(), chunk(1, b"abc", 0, 3), 2000).await.unwrap();
    let other = uploads.put(LOCAL, chunk(1, b"abc", 0, 3), 2000).await;
    assert!(matches!(other, Err(UploadError::Forbidden)), "{other:?}");
    assert!(matches!(uploads.discard(LOCAL, upload(1)), Err(UploadError::Forbidden)));
    assert!(matches!(uploads.missing(LOCAL, &[upload(1)]), Err(UploadError::Forbidden)));
    assert_eq!(uploads.missing(phone(), &[upload(2), upload(1)]).unwrap(), [upload(2)]);
    uploads.discard(phone(), upload(1)).unwrap();
    uploads.discard(phone(), upload(1)).expect("an unknown upload is no error");
    assert_eq!(uploads.missing(phone(), &[upload(1)]).unwrap(), [upload(1)]);
    gone(&part(state.path(), 1)).await;
}

#[tokio::test]
async fn an_upload_keeps_its_first_name_size_and_origin_and_ends_at_its_size() {
    let state = tempfile::tempdir().unwrap();
    let uploads = uploads(state.path(), &TestClock::new());
    uploads.put(LOCAL, chunk(1, b"abcd", 0, 2), 2000).await.unwrap();
    let mut renamed = chunk(1, b"abcd", 2, 2);
    renamed.name = "other.txt".to_owned();
    assert!(matches!(uploads.put(LOCAL, renamed, 2000).await, Err(UploadError::Changed)));
    let mut longer = chunk(1, b"abcd", 2, 2);
    longer.data = b"cdef".to_vec();
    assert!(matches!(uploads.put(LOCAL, longer, 2000).await, Err(UploadError::PastEnd)));
    assert_eq!(uploads.put(LOCAL, chunk(1, b"abcd", 2, 2), 2000).await.unwrap().received, 4);
}

#[tokio::test]
async fn each_owner_has_its_own_places() {
    let state = tempfile::tempdir().unwrap();
    let uploads = uploads(state.path(), &TestClock::new());
    let open = MAX_OPEN_UPLOADS as u128;
    for n in 0..open {
        uploads.put(LOCAL, chunk(n, b"ab", 0, 1), 2000).await.unwrap();
    }
    let over = uploads.put(LOCAL, chunk(open, b"ab", 0, 1), 2000).await;
    assert!(matches!(over, Err(UploadError::TooMany)), "{over:?}");
    uploads.put(phone(), chunk(open, b"ab", 0, 1), 2000).await.expect("another owner");
    uploads.put(LOCAL, chunk(0, b"ab", 1, 1), 2000).await.expect("an open one goes on");
}

#[tokio::test]
async fn a_closed_connection_leaves_no_part_and_a_device_keeps_its_uploads() {
    let state = tempfile::tempdir().unwrap();
    let uploads = uploads(state.path(), &TestClock::new());
    uploads.put(LOCAL, chunk(1, b"abc", 0, 1), 2000).await.unwrap();
    uploads.put(phone(), chunk(2, b"abc", 0, 1), 2000).await.unwrap();
    uploads.closed(ConnId::new(1));
    gone(&part(state.path(), 1)).await;
    assert_eq!(uploads.put(phone(), chunk(2, b"abc", 1, 2), 2000).await.unwrap().received, 3);
}

#[tokio::test]
async fn an_upload_of_a_device_that_nothing_named_for_its_life_goes() {
    let state = tempfile::tempdir().unwrap();
    let clock = TestClock::new();
    let uploads = uploads(state.path(), &clock);
    uploads.put(phone(), chunk(1, b"abc", 0, 3), 2000).await.unwrap();
    uploads.put(LOCAL, chunk(2, b"abc", 0, 1), 2000).await.unwrap();
    clock.advance(UPLOAD_TTL - Duration::from_secs(1));
    assert_eq!(uploads.missing(phone(), &[upload(1)]).unwrap(), Vec::<UploadId>::new());
    clock.advance(UPLOAD_TTL - Duration::from_secs(1));
    assert!(uploads.finished(upload(1)).is_some(), "the prompt named it");
    clock.advance(Duration::from_secs(2));
    uploads.put(phone(), chunk(3, b"x", 0, 0), 2000).await.unwrap();
    assert!(uploads.finished(upload(1)).is_none(), "the next put swept it");
    gone(&part(state.path(), 1)).await;
    assert_eq!(uploads.put(LOCAL, chunk(2, b"abc", 1, 2), 2000).await.unwrap().received, 3);
}

#[tokio::test]
async fn an_expired_upload_goes_with_no_put_and_a_revoke_removes_those_of_its_device() {
    let state = tempfile::tempdir().unwrap();
    let clock = TestClock::new();
    let uploads = uploads(state.path(), &clock);
    uploads.put(phone(), chunk(1, b"abc", 0, 1), 2000).await.unwrap();
    clock.advance(UPLOAD_TTL);
    uploads.expire();
    gone(&part(state.path(), 1)).await;

    let other = Owner::Device(DeviceId::from_uuid(Uuid::from_u128(8)));
    uploads.put(phone(), chunk(2, b"abc", 0, 1), 2000).await.unwrap();
    uploads.put(other, chunk(3, b"abc", 0, 1), 2000).await.unwrap();
    uploads.revoked(DeviceId::from_uuid(Uuid::from_u128(9)));
    gone(&part(state.path(), 2)).await;
    assert_eq!(uploads.put(other, chunk(3, b"abc", 1, 2), 2000).await.unwrap().received, 3);
}

#[tokio::test]
async fn a_start_removes_every_part_and_a_prompt_takes_its_uploads() {
    let state = tempfile::tempdir().unwrap();
    let uploads = uploads(state.path(), &TestClock::new());
    uploads.put(LOCAL, chunk(1, b"abc", 0, 3), 2000).await.unwrap();
    uploads.remove(&[upload(1)]);
    assert!(uploads.finished(upload(1)).is_none());
    gone(&part(state.path(), 1)).await;

    let left = state.path().join(UPLOADS_DIR).join("old.part");
    std::fs::write(&left, "of an earlier efrd").unwrap();
    uploads.sweep().await;
    assert!(!left.exists());
    uploads.sweep().await;
}

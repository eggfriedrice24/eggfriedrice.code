//! Attachments over a `TestDaemon`: the capability of `hello` and the uploads of
//! `attachment.put` and `attachment.discard`.

use efr_protocol::{
    AttachmentDiscard, AttachmentDiscardResult, AttachmentPut, AttachmentPutResult, Base64Bytes,
    ErrorCode, Method, UploadId,
};
use efr_test_daemon::{ClientError, TestDaemon};

/// The code of the daemon's refusal of `result`.
fn refusal<T: std::fmt::Debug>(result: Result<T, ClientError>) -> ErrorCode {
    match result {
        Err(ClientError::Server { body }) => body.code,
        other => panic!("expected a refusal from the daemon, got {other:?}"),
    }
}

#[tokio::test]
async fn a_daemon_that_takes_no_attachments_says_so_and_refuses_an_upload() {
    let daemon = TestDaemon::start().await.unwrap();
    let client = daemon.client().await.unwrap();
    assert_eq!(client.hello().capabilities.attachments, None);

    let upload_id = UploadId::from_uuid(uuid::Uuid::from_u128(1));
    let put = client
        .call::<AttachmentPutResult>(Method::AttachmentPut(AttachmentPut {
            upload_id,
            name: "shot.png".to_owned(),
            total_bytes: 4,
            offset: 0,
            data: Base64Bytes::new(*b"\x89PNG"),
            origin: None,
        }))
        .await;
    assert_eq!(refusal(put), ErrorCode::Invalid);
    let discard = client
        .call::<AttachmentDiscardResult>(Method::AttachmentDiscard(AttachmentDiscard { upload_id }))
        .await;
    assert_eq!(refusal(discard), ErrorCode::Invalid);

    drop(client);
    daemon.stop().await.unwrap();
}

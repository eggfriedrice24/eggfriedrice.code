//! The uploads of `attachment.put` and the prompts that take them, from `efr` on the
//! Unix socket and from a paired phone on the tailnet listener: the result rule of a
//! chunk, the owners, the origins, the limits, the parts that a closed connection or a
//! restart leaves, and a missing upload that the client sends again.

use efr_protocol::{
    AttachmentDiscard, AttachmentDiscardResult, AttachmentId, AttachmentOrigin, AttachmentPut,
    AttachmentPutResult, Base64Bytes, CHUNK_BYTES, ErrorCode, Event, MAX_OPEN_UPLOADS, Method,
    MissingUploads, PromptSend, PromptSendResult, ShellContext, TurnSettings, UploadId,
};
use efr_test_daemon::{
    Client, ClientError, MessagesAnswer, MessagesServer, PairedPhone, TTY, TestDaemon,
    TestDaemonError, command_id, events_until,
};
use pretty_assertions::assert_eq;
use serde_json::json;

use super::{PIXEL_PNG, claude, pixel, refusal};

fn upload(n: u128) -> UploadId {
    UploadId::from_uuid(uuid::Uuid::from_u128(n))
}

/// The chunk of `file` from `offset` with `len` bytes, of the upload `n` named
/// `shot.png` from `origin`.
fn put(
    n: u128,
    file: &[u8],
    offset: usize,
    len: usize,
    origin: Option<AttachmentOrigin>,
) -> Method {
    Method::AttachmentPut(AttachmentPut {
        upload_id: upload(n),
        name: "shot.png".to_owned(),
        total_bytes: file.len() as u64,
        offset: offset as u64,
        data: Base64Bytes::new(&file[offset..offset + len]),
        origin,
    })
}

/// The origin of an `@shot.png` in the working directory of `daemon`.
fn at_path(daemon: &TestDaemon) -> Option<AttachmentOrigin> {
    Some(AttachmentOrigin::Path { path: daemon.cwd().join("shot.png") })
}

/// Prompt `n` with `text` and the uploads `uploads`, from the terminal when `tty`.
fn prompt(daemon: &TestDaemon, n: u128, text: &str, uploads: Vec<UploadId>, tty: bool) -> Method {
    let mut context = ShellContext::new(daemon.cwd());
    context.tty = tty.then(|| TTY.to_owned());
    Method::PromptSend(PromptSend {
        command_id: command_id(n),
        conversation_id: None,
        new_conversation: false,
        text: text.to_owned(),
        context: Some(context),
        last_command: None,
        recent_commands: Vec::new(),
        settings: TurnSettings::default(),
        attachments: uploads,
    })
}

/// The uploads of a refused `prompt.send` that efrd does not have.
fn missing(result: Result<PromptSendResult, ClientError>) -> Vec<UploadId> {
    let Err(ClientError::Server { body }) = result else { panic!("not refused: {result:?}") };
    assert_eq!(body.code, ErrorCode::NotFound);
    let data: MissingUploads = serde_json::from_value(body.data.expect("data")).unwrap();
    data.missing_uploads
}

/// Follows the turn of `sent` on `client` to its end, and gives the events of the
/// conversation up to there.
async fn ended(daemon: &TestDaemon, client: &Client, sent: &PromptSendResult) -> Vec<Event> {
    let mut follow = daemon.follow(client, sent.conversation_id).await.unwrap();
    let events = events_until(&mut follow, |event| {
        matches!(
            event,
            Event::TurnCompleted { turn_id, .. } | Event::TurnFailed { turn_id, .. }
                if *turn_id == sent.turn_id
        )
    })
    .await
    .unwrap();
    events.into_iter().map(|envelope| envelope.event).collect()
}

/// True when the uploads dir of `daemon` holds no part.
fn no_part(daemon: &TestDaemon) -> bool {
    let dir = daemon.dirs().dirs().state().join("uploads");
    std::fs::read_dir(dir).map_or(true, |mut entries| entries.next().is_none())
}

async fn no_part_left(daemon: &TestDaemon) {
    efr_test_support::Wait::new("the parts go").until(|| no_part(daemon)).await.unwrap();
}

#[tokio::test]
async fn an_upload_in_chunks_goes_with_its_prompt_as_an_image_and_a_copy_of_the_file() {
    let server = MessagesServer::start().await;
    server.set_models(vec![claude(true)]);
    server.push(MessagesAnswer::text("A dot."));
    let daemon = TestDaemon::builder().messages(&server).start().await.unwrap();
    let client = daemon.client_for_tty(TTY).await.unwrap();

    let first: AttachmentPutResult =
        client.call(put(1, PIXEL_PNG, 0, 30, at_path(&daemon))).await.unwrap();
    assert_eq!((first.received, first.upload), (30, None));
    let rest = PIXEL_PNG.len() - 30;
    let done: AttachmentPutResult =
        client.call(put(1, PIXEL_PNG, 30, rest, at_path(&daemon))).await.unwrap();
    assert_eq!(done.upload.map(|info| info.media_type), Some("image/png".to_owned()));
    let sent: PromptSendResult =
        client.call(prompt(&daemon, 1, "what is this", vec![upload(1)], true)).await.unwrap();
    let events = ended(&daemon, &client, &sent).await;

    let attachments = events.iter().find_map(|event| match event {
        Event::PromptQueued { attachments, .. } => Some(attachments.clone()),
        _ => None,
    });
    let [attachment] = attachments.unwrap().try_into().unwrap();
    assert_eq!(attachment.id, AttachmentId::nth(1));
    assert_eq!(attachment.origin, at_path(&daemon).unwrap());
    let copy = attachment.copy.expect("a copy in scratch");
    assert!(copy.ends_with("attachments/a1-shot.png"), "{}", copy.display());
    assert_eq!(std::fs::read(&copy).unwrap(), PIXEL_PNG);
    let content = server.received()[0].body["messages"][0]["content"].clone();
    let image = json!({"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": pixel()}});
    assert!(content.as_array().unwrap().contains(&image), "{content:#}");
    no_part_left(&daemon).await;
    drop(client);
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn a_link_that_a_command_planted_with_the_next_name_is_not_followed() {
    let server = MessagesServer::start().await;
    server.set_models(vec![claude(true)]);
    server.push(MessagesAnswer::text("A dot."));
    server.push(MessagesAnswer::text("The same dot."));
    let daemon = TestDaemon::builder().messages(&server).start().await.unwrap();
    let client = daemon.client_for_tty(TTY).await.unwrap();
    let all = PIXEL_PNG.len();
    let _: AttachmentPutResult =
        client.call(put(1, PIXEL_PNG, 0, all, at_path(&daemon))).await.unwrap();
    let sent: PromptSendResult =
        client.call(prompt(&daemon, 1, "what is this", vec![upload(1)], true)).await.unwrap();
    let events = ended(&daemon, &client, &sent).await;
    let copies = events.iter().find_map(|event| match event {
        Event::PromptQueued { attachments, .. } => attachments[0].copy.clone(),
        _ => None,
    });
    let dir = copies.unwrap().parent().unwrap().to_path_buf();
    // NOTE: what a command in a mode with no sandbox can do: the dir is read-only only
    // in contained calls, and the next id is known.
    let target = daemon.cwd().join("kept.txt");
    std::fs::write(&target, "the user's file").unwrap();
    std::os::unix::fs::symlink(&target, dir.join("a2-shot.png")).unwrap();

    let _: AttachmentPutResult =
        client.call(put(2, PIXEL_PNG, 0, all, at_path(&daemon))).await.unwrap();
    let again: PromptSendResult =
        client.call(prompt(&daemon, 2, "and this?", vec![upload(2)], true)).await.unwrap();
    assert_eq!(again.conversation_id, sent.conversation_id);
    ended(&daemon, &client, &again).await;

    assert_eq!(std::fs::read_to_string(&target).unwrap(), "the user's file");
    assert!(dir.join("a2-shot.png.moved-1").is_symlink());
    assert_eq!(std::fs::read(dir.join("a2-shot.png")).unwrap(), PIXEL_PNG);
    drop(client);
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn a_missing_upload_writes_no_receipt_and_the_same_command_works_once_after_it() {
    let server = MessagesServer::start().await;
    server.set_models(vec![claude(true)]);
    server.push(MessagesAnswer::text("A dot."));
    let daemon = TestDaemon::builder().messages(&server).start().await.unwrap();
    let client = daemon.client_for_tty(TTY).await.unwrap();
    let send = || {
        client.call::<PromptSendResult>(prompt(
            &daemon,
            1,
            "what is this",
            vec![upload(1), upload(2)],
            true,
        ))
    };

    let half: AttachmentPutResult =
        client.call(put(1, PIXEL_PNG, 0, 10, at_path(&daemon))).await.unwrap();
    assert_eq!(half.received, 10);
    let blank = client.call::<PromptSendResult>(prompt(&daemon, 2, " ", vec![upload(1)], true));
    assert_eq!(refusal(blank.await), ErrorCode::Invalid, "a prompt with attachments needs text");
    assert_eq!(missing(send().await), [upload(1), upload(2)]);
    for n in [1, 2] {
        let done: AttachmentPutResult =
            client.call(put(n, PIXEL_PNG, 0, PIXEL_PNG.len(), at_path(&daemon))).await.unwrap();
        assert_eq!(done.received, if n == 1 { 10 } else { PIXEL_PNG.len() as u64 });
    }
    let rest = PIXEL_PNG.len() - 10;
    let _: AttachmentPutResult =
        client.call(put(1, PIXEL_PNG, 10, rest, at_path(&daemon))).await.unwrap();
    let sent = send().await.unwrap();
    let again = send().await.unwrap();
    assert_eq!(again, sent, "a retry after a good send answers from the receipt");
    ended(&daemon, &client, &sent).await;
    assert_eq!(server.received().len(), 1, "one turn");
    drop(client);
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn a_local_upload_names_an_absolute_path_or_a_paste_and_fits_the_limits() {
    let daemon = TestDaemon::start().await.unwrap();
    let client = daemon.client().await.unwrap();
    let home = daemon.dirs().home().to_path_buf();
    let origins = [
        None,
        Some(AttachmentOrigin::Phone),
        Some(AttachmentOrigin::Tool { path: daemon.cwd().join("shot.png") }),
        Some(AttachmentOrigin::Path { path: "shot.png".into() }),
    ];
    for origin in origins {
        let refused =
            client.call::<AttachmentPutResult>(put(1, b"png", 0, 3, origin.clone())).await;
        assert_eq!(refusal(refused), ErrorCode::Invalid, "{origin:?}");
    }
    let secret = Some(AttachmentOrigin::Path { path: home.join(".ssh/id_ed25519") });
    let refused = client.call::<AttachmentPutResult>(put(1, b"key", 0, 3, secret)).await;
    let Err(ClientError::Server { body }) = refused else { panic!("{refused:?}") };
    assert_eq!(body.message, "the request is invalid: the file of the @path is a secret file");
    let big = vec![0_u8; CHUNK_BYTES as usize + 1];
    let over =
        client.call::<AttachmentPutResult>(put(1, &big, 0, big.len(), at_path(&daemon))).await;
    assert_eq!(refusal(over), ErrorCode::Invalid, "a chunk over the limit");
    let mut huge = put(1, b"x", 0, 1, at_path(&daemon));
    if let Method::AttachmentPut(params) = &mut huge {
        params.total_bytes = 21 * 1024 * 1024;
    }
    assert_eq!(refusal(client.call::<AttachmentPutResult>(huge).await), ErrorCode::Invalid);
    let pasted: AttachmentPutResult =
        client.call(put(1, b"png", 0, 3, Some(AttachmentOrigin::Clipboard))).await.unwrap();
    assert_eq!(pasted.received, 3);
    drop(client);
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn a_closed_connection_leaves_no_part_and_no_other_connection_takes_its_upload() {
    let daemon = TestDaemon::start().await.unwrap();
    let owner = daemon.client_for_tty(TTY).await.unwrap();
    let other = daemon.client_for_tty(TTY).await.unwrap();
    let _: AttachmentPutResult =
        owner.call(put(1, PIXEL_PNG, 0, 10, at_path(&daemon))).await.unwrap();
    assert!(!no_part(&daemon));

    let taken = other
        .call::<PromptSendResult>(prompt(&daemon, 1, "what is this", vec![upload(1)], true))
        .await;
    assert_eq!(refusal(taken), ErrorCode::Forbidden);
    let added =
        other.call::<AttachmentPutResult>(put(1, PIXEL_PNG, 10, 10, at_path(&daemon))).await;
    assert_eq!(refusal(added), ErrorCode::Forbidden);
    drop(owner);
    no_part_left(&daemon).await;
    drop(other);
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn three_connections_upload_ten_files_each_at_once() {
    let daemon = TestDaemon::start().await.unwrap();
    let clients = [
        daemon.client().await.unwrap(),
        daemon.client().await.unwrap(),
        daemon.client().await.unwrap(),
    ];
    let puts = clients.iter().enumerate().flat_map(|(c, client)| {
        let daemon = &daemon;
        (0..10).map(move |n| async move {
            let n = (c * 10 + n) as u128;
            client
                .call::<AttachmentPutResult>(put(n, PIXEL_PNG, 0, PIXEL_PNG.len(), at_path(daemon)))
                .await
        })
    });
    let done = futures::future::join_all(puts).await;
    assert!(done.iter().all(|put| put.as_ref().is_ok_and(|put| put.upload.is_some())), "{done:?}");
    drop(clients);
    no_part_left(&daemon).await;
    daemon.stop().await.unwrap();
}

/// The code of the daemon's refusal of a phone's `result`.
fn phone_refusal<T: std::fmt::Debug>(result: Result<T, TestDaemonError>) -> ErrorCode {
    match result {
        Err(TestDaemonError::PhoneServer { body }) => body.code,
        other => panic!("the daemon did not refuse the phone's request: {other:?}"),
    }
}

#[tokio::test]
async fn a_phone_resumes_its_upload_over_a_new_connection_and_another_device_cannot_touch_it() {
    let daemon = TestDaemon::builder().remote().start().await.unwrap();
    let mut paired = PairedPhone::pair(&daemon).await.unwrap();
    let file = PIXEL_PNG;

    let first: AttachmentPutResult = paired.phone.call(put(1, file, 0, 20, None)).await.unwrap();
    assert_eq!(first.received, 20);
    let named =
        paired.phone.call::<AttachmentPutResult>(put(1, file, 0, 20, at_path(&daemon))).await;
    assert_eq!(phone_refusal(named), ErrorCode::Invalid, "a phone names no origin");
    let resumed = paired.reconnect(&daemon).await.unwrap();
    paired.phone = resumed;
    let repeat: AttachmentPutResult = paired.phone.call(put(1, file, 0, 20, None)).await.unwrap();
    assert_eq!((repeat.received, repeat.upload), (20, None), "a repeated chunk writes nothing");
    let rest = file.len() - 20;
    let done: AttachmentPutResult = paired.phone.call(put(1, file, 20, rest, None)).await.unwrap();
    assert!(done.upload.is_some());

    let mut other = PairedPhone::pair(&daemon).await.unwrap();
    let theirs = other.phone.call::<AttachmentPutResult>(put(1, file, 0, 1, None)).await;
    assert_eq!(phone_refusal(theirs), ErrorCode::Forbidden);
    let discard = Method::AttachmentDiscard(AttachmentDiscard { upload_id: upload(1) });
    let theirs = other.phone.call::<AttachmentDiscardResult>(discard.clone()).await;
    assert_eq!(phone_refusal(theirs), ErrorCode::Forbidden);
    let _: AttachmentDiscardResult = paired.phone.call(discard).await.unwrap();
    let gone =
        paired.phone.call::<AttachmentPutResult>(put(1, file, 20, rest, None)).await.unwrap();
    assert_eq!(gone.received, 0, "the discard dropped the bytes");
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn a_phone_has_twenty_open_uploads() {
    let daemon = TestDaemon::builder().remote().start().await.unwrap();
    let mut paired = PairedPhone::pair(&daemon).await.unwrap();
    for n in 0..MAX_OPEN_UPLOADS as u128 {
        let _: AttachmentPutResult = paired.phone.call(put(n, b"abc", 0, 1, None)).await.unwrap();
    }
    let over = paired.phone.call::<AttachmentPutResult>(put(99, b"abc", 0, 1, None)).await;
    assert_eq!(phone_refusal(over), ErrorCode::Invalid);
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn after_a_restart_a_phone_uploads_again_and_its_prompt_runs_once() {
    let server = MessagesServer::start().await;
    server.set_models(vec![claude(true)]);
    server.push(MessagesAnswer::text("A dot."));
    let mut daemon =
        TestDaemon::builder().remote().messages(&server).persistent().start().await.unwrap();
    let mut paired = PairedPhone::pair(&daemon).await.unwrap();
    let all = PIXEL_PNG.len();
    let _: AttachmentPutResult = paired.phone.call(put(1, PIXEL_PNG, 0, all, None)).await.unwrap();

    daemon.restart().await.unwrap();
    let resumed = paired.reconnect(&daemon).await.unwrap();
    let phone = &mut paired.phone;
    *phone = resumed;
    let send = prompt(&daemon, 1, "what is this", vec![upload(1)], false);
    let refused = phone.call::<PromptSendResult>(send.clone()).await;
    let Err(TestDaemonError::PhoneServer { body }) = refused else { panic!("{refused:?}") };
    assert_eq!(body.code, ErrorCode::NotFound);
    assert_eq!(body.data, Some(json!({"missing_uploads": [upload(1)]})));
    let _: AttachmentPutResult = phone.call(put(1, PIXEL_PNG, 0, all, None)).await.unwrap();
    let sent: PromptSendResult = phone.call(send.clone()).await.unwrap();
    let again: PromptSendResult = phone.call(send).await.unwrap();
    assert_eq!(again, sent);
    let terminal = daemon.client().await.unwrap();
    let events = ended(&daemon, &terminal, &sent).await;
    let origins: Vec<AttachmentOrigin> = events
        .iter()
        .filter_map(|event| match event {
            Event::PromptQueued { attachments, .. } => {
                Some(attachments.iter().map(|a| a.origin.clone()))
            }
            _ => None,
        })
        .flatten()
        .collect();
    assert_eq!(origins, [AttachmentOrigin::Phone]);
    assert_eq!(server.received().len(), 1, "the prompt ran once");
    drop(terminal);
    daemon.stop().await.unwrap();
}

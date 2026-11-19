//! `$SCRATCH/attachments` in the real sandbox of `just test-sandbox`: a contained call
//! reads the copy of the first prompt of a new conversation and cannot change it, add
//! an entry or make the dir before efrd does.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use efr_protocol::{
    AdminStatus, AdminStatusResult, ApprovalDecision, AttachmentOrigin, AttachmentPut,
    AttachmentPutResult, Base64Bytes, ConversationId, Event, EventEnvelope, Method, Mode,
    PromptSend, PromptSendResult, ShellContext, TurnSettings, UploadId,
};
use efr_test_daemon::{Client, TTY, TestDaemon, command_id};
use efr_test_support::TestDirs;
use serde_json::json;

use super::{ScriptedModel, completed, real_launcher, run_prompt, skip, user_answers, with_zsh};
use crate::support::zsh_enabled;

/// The file that the tests attach.
const NOTES: &[u8] = b"the notes of the user\n";

/// A daemon in `auto` over `model` with the real launcher; `None` after a `skipped:`
/// line without zsh or the launcher, or when the probe says no.
async fn real_daemon(
    test: &str,
    dirs: &Arc<TestDirs>,
    model: Arc<ScriptedModel>,
) -> Option<TestDaemon> {
    if !zsh_enabled(test) {
        return None;
    }
    let launcher = real_launcher(test)?;
    let user_runtime = dirs.create_dir("xrt").unwrap();
    let env = std::collections::BTreeMap::from([
        ("HOME".to_owned(), dirs.home().to_string_lossy().into_owned()),
        ("PATH".to_owned(), "/usr/bin:/bin".to_owned()),
        ("LANG".to_owned(), "C.UTF-8".to_owned()),
        ("XDG_RUNTIME_DIR".to_owned(), user_runtime.to_string_lossy().into_owned()),
    ]);
    let daemon = user_answers(with_zsh(TestDaemon::builder()))
        .dirs(Arc::clone(dirs))
        .shell_env(env)
        .sandbox_launcher(&launcher)
        .custom_provider(model)
        .start()
        .await
        .unwrap();
    let client = daemon.client().await.unwrap();
    let status: AdminStatusResult = client.call(Method::AdminStatus(AdminStatus {})).await.unwrap();
    if !status.sandbox.as_ref().is_some_and(|sandbox| sandbox.available) {
        // NOTE: just test-sandbox fails when this says skipped on a ready machine.
        skip(test, &format!("the probe says the sandbox is unavailable: {:?}", status.sandbox));
        return None;
    }
    Some(daemon)
}

/// Uploads [`NOTES`] as `notes.txt` of the working directory on `client`, as upload `n`.
async fn upload(daemon: &TestDaemon, client: &Client, n: u128) -> UploadId {
    let upload_id = UploadId::from_uuid(uuid::Uuid::from_u128(n));
    let put = Method::AttachmentPut(AttachmentPut {
        upload_id,
        name: "notes.txt".to_owned(),
        total_bytes: NOTES.len() as u64,
        offset: 0,
        data: Base64Bytes::new(NOTES),
        origin: Some(AttachmentOrigin::Path { path: daemon.cwd().join("notes.txt") }),
    });
    let done: AttachmentPutResult = client.call(put).await.unwrap();
    assert!(done.upload.is_some(), "the upload is whole");
    upload_id
}

/// Prompt `n` in `auto` from the terminal, with `text` and `uploads`.
fn send(daemon: &TestDaemon, n: u128, text: &str, uploads: Vec<UploadId>) -> PromptSend {
    let mut context = ShellContext::new(daemon.cwd());
    context.tty = Some(TTY.to_owned());
    PromptSend {
        command_id: command_id(n),
        conversation_id: None,
        new_conversation: false,
        text: text.to_owned(),
        context: Some(context),
        last_command: None,
        recent_commands: Vec::new(),
        settings: TurnSettings { mode: Some(Mode::Auto), ..TurnSettings::default() },
        attachments: uploads,
    }
}

/// The first turn of a new conversation: prompt `n` with `text` and `uploads`, sent
/// on `client`, every exit refused.
async fn run(
    daemon: &TestDaemon,
    client: &Client,
    n: u128,
    text: &str,
    uploads: Vec<UploadId>,
) -> Vec<EventEnvelope> {
    let params = send(daemon, n, text, uploads);
    run_prompt(daemon, client, params, ApprovalDecision::Deny).await.1
}

/// The conversation of `events`.
fn conversation(events: &[EventEnvelope]) -> ConversationId {
    events.iter().find_map(|envelope| envelope.conversation_id).unwrap()
}

/// The copy of the one attachment that the prompt of `events` recorded.
fn copy(events: &[EventEnvelope]) -> PathBuf {
    let copies: Vec<PathBuf> = events
        .iter()
        .filter_map(|envelope| match &envelope.event {
            Event::PromptQueued { attachments, .. } => Some(attachments.clone()),
            _ => None,
        })
        .flatten()
        .filter_map(|attachment| attachment.copy)
        .collect();
    let [copy] = copies.try_into().unwrap();
    copy
}

/// Below the target dir, not /tmp, which the real sandbox replaces with its own.
fn dirs() -> Arc<TestDirs> {
    Arc::new(TestDirs::new_in(Path::new(env!("CARGO_TARGET_TMPDIR"))).unwrap())
}

#[tokio::test]
async fn shell_a_contained_call_reads_an_attachment_and_cannot_change_the_dir() {
    let test = "shell_a_contained_call_reads_an_attachment_and_cannot_change_the_dir";
    // NOTE: the writes hide in `sh -c`, so efr predicts no exit and the sandbox alone
    // must stop them.
    let line = "cat \"$SCRATCH/attachments/a1-notes.txt\"; \
                sh -c \"echo changed > $SCRATCH/attachments/a1-notes.txt\"; \
                sh -c \"echo planted > $SCRATCH/attachments/a2-notes.txt\"; \
                print -r -- status=$?";
    let model = ScriptedModel::new(vec![json!({ "command": line })]);
    let dirs = dirs();
    let Some(daemon) = real_daemon(test, &dirs, model).await else { return };
    let client = daemon.client_for_tty(TTY).await.unwrap();
    let notes = upload(&daemon, &client, 1).await;

    let events = run(&daemon, &client, 1, "read my notes", vec![notes]).await;

    let [(output, _)] = completed(&events).try_into().unwrap();
    assert!(output.contains("the notes of the user"), "the copy of the first prompt: {output}");
    assert!(output.to_lowercase().contains("read-only file system"), "{output}");
    // NOTE: the status of a failed redirect is 1 in bash and 2 in dash (Ubuntu's sh).
    assert!(output.contains("status=1") || output.contains("status=2"), "{output}");
    let copy = copy(&events);
    assert_eq!(std::fs::read(&copy).unwrap(), NOTES, "the copy is as it was attached");
    assert!(!copy.with_file_name("a2-notes.txt").exists(), "no call adds an entry");
    drop(client);
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn shell_a_contained_call_cannot_make_the_dir_of_the_attachments_first() {
    let test = "shell_a_contained_call_cannot_make_the_dir_of_the_attachments_first";
    // NOTE: hidden in `sh -c` as above.
    let line = "sh -c \"mkdir -p $SCRATCH/attachments/a1-notes.txt\"; print -r -- status=$?";
    let model = ScriptedModel::new(vec![json!({ "command": line })]);
    let dirs = dirs();
    let Some(daemon) = real_daemon(test, &dirs, model).await else { return };
    let client = daemon.client_for_tty(TTY).await.unwrap();

    let first = run(&daemon, &client, 1, "make a dir", Vec::new()).await;
    let [(output, _)] = completed(&first).try_into().unwrap();
    assert!(output.contains("status=1"), "efrd made the dir before the call: {output}");
    let notes = upload(&daemon, &client, 2).await;
    let second = send(&daemon, 2, "read my notes", vec![notes]);
    // NOTE: the prompt binds its upload before it answers, so no wait for its turn.
    let _: PromptSendResult = client.call(Method::PromptSend(second)).await.unwrap();

    let root = dirs.dirs().data().join("scratch");
    let [scratch] = efr_conversation::scratch_dirs(&root, conversation(&first)).try_into().unwrap();
    let dir = scratch.join("attachments");
    assert_eq!(std::fs::read(dir.join("a1-notes.txt")).unwrap(), NOTES);
    assert!(dir.join(".efr-attachments").is_file(), "the dir is efrd's");
    drop(client);
    daemon.stop().await.unwrap();
}

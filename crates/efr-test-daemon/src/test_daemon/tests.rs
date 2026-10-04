use std::os::unix::fs::PermissionsExt as _;

use efr_protocol::{
    ConversationsList, ConversationsListResult, ErrorCode, Event, Method, PromptSendResult,
};
use efr_test_support::TestDirs;
use pretty_assertions::assert_eq;

use super::{
    API_KEY, HOME_PLACEHOLDER, TTY, TestDaemon, command_id, events_until, store_api_key,
    working_dir,
};

#[test]
fn command_ids_have_the_form_the_fixtures_use() {
    assert_eq!(command_id(1).to_string(), "0192f0c1-7a00-7000-8000-000000000001");
    assert_eq!(command_id(0xbeef).to_string(), "0192f0c1-7a00-7000-8000-00000000beef");
}

#[test]
fn the_working_directory_and_the_home_have_placeholders() {
    let dirs = TestDirs::new().unwrap();
    let (cwd, redactor) = working_dir(&dirs).unwrap();
    let root = dirs.root().display();

    assert!(cwd.is_dir());
    assert_eq!(cwd, dirs.home().join("project"));
    let text = format!(
        "{} {}/notes {} {root}/data at 2026-10-04T12:00:00Z",
        cwd.display(),
        cwd.display(),
        dirs.home().display()
    );
    assert_eq!(
        redactor.redact(&text),
        format!("<CWD> <CWD>/notes {HOME_PLACEHOLDER} <TMP>/data at <TIMESTAMP>")
    );
}

#[test]
fn the_api_key_is_stored_as_the_file_store_keeps_secrets() {
    let dirs = TestDirs::new().unwrap();
    let data = dirs.dirs().data();

    store_api_key(data).unwrap();

    let file = data.join("secrets/openai-api.json");
    let record: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(record, serde_json::json!({ "version": 1, "kind": "api_key", "key": API_KEY }));
    let mode = |path: &std::path::Path| std::fs::metadata(path).unwrap().permissions().mode();
    assert_eq!(mode(&file) & 0o777, 0o600);
    assert_eq!(mode(&data.join("secrets")) & 0o777, 0o700);
}

#[tokio::test]
async fn a_daemon_serves_on_its_own_tree_and_cleans_up_when_it_stops() {
    let daemon = TestDaemon::start().await.unwrap();
    let runtime = daemon.dirs().dirs().runtime().to_path_buf();

    assert_eq!(daemon.socket_path(), runtime.join("daemon.sock"));
    assert!(runtime.join("daemon.json").is_file());
    let client = daemon.client().await.unwrap();
    assert_eq!(client.hello().daemon_id, daemon.daemon_id());
    assert!(format!("{daemon:?}").contains("running: true"));
    drop(client);
    let dirs = daemon.dirs().clone();
    daemon.stop().await.unwrap();

    assert!(!runtime.join("daemon.json").exists());
    assert!(!runtime.join("daemon.sock").exists());
    drop(dirs);
}

#[tokio::test]
async fn without_a_provider_a_model_call_fails_instead_of_reaching_the_network() {
    let daemon = TestDaemon::start().await.unwrap();
    let client = daemon.client_for_tty(TTY).await.unwrap();

    let sent: PromptSendResult = client.call(daemon.prompt(1, "hello", TTY)).await.unwrap();
    let mut follow = daemon.follow(&client, sent.conversation_id).await.unwrap();
    let events = events_until(&mut follow, |event| {
        matches!(event, Event::TurnFailed { .. } | Event::TurnCompleted { .. })
    })
    .await
    .unwrap();

    let Event::TurnFailed { error, .. } = &events.last().unwrap().event else {
        panic!("{events:#?}")
    };
    assert_eq!(error.code, ErrorCode::Internal);
    assert!(error.message.contains("replay"), "{}", error.message);
    drop((follow, client));
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn a_restart_keeps_the_daemon_id_and_only_a_persistent_log() {
    for persistent in [true, false] {
        let builder = TestDaemon::builder();
        let builder = if persistent { builder.persistent() } else { builder };
        let mut daemon = builder.start().await.unwrap();
        let before = daemon.daemon_id();
        let client = daemon.client_for_tty(TTY).await.unwrap();
        let sent: PromptSendResult = client.call(daemon.prompt(1, "hello", TTY)).await.unwrap();
        drop(client);

        daemon.restart().await.unwrap();

        assert_eq!(daemon.daemon_id(), before);
        let client = daemon.client().await.unwrap();
        let list: ConversationsListResult =
            client.call(Method::ConversationsList(ConversationsList::default())).await.unwrap();
        let ids: Vec<_> = list.conversations.iter().map(|summary| summary.id).collect();
        if persistent {
            assert_eq!(ids, [sent.conversation_id]);
            let events = daemon.events(&client, sent.conversation_id).await.unwrap();
            assert_eq!(events[0].event.kind(), "conversation_created");
        } else {
            assert!(ids.is_empty(), "an in-memory log ends with its daemon");
        }
        drop(client);
        daemon.stop().await.unwrap();
    }
}

//! The accounts of one kind over a `TestDaemon`: three Anthropic keys against one local
//! Messages API, each account with its own key. A conversation moves to another account
//! of the same model and back, and each turn sends the key of its account. A logout of
//! the account that the conversation keeps moves the next turn back to the account that
//! the router picks; an account that `[accounts]` names stays listed when it signs out,
//! so the conversation keeps it and its turn fails until a new login, with no restart.
//! A phone prompt that names an account runs on it, and a turn that a job notice
//! starts runs on the account that the conversation keeps. An account that
//! reached a limit whose reset comes before the one of the stopped account is offered as
//! full, with its reset. A logout and a new login of an account clear its limits, with
//! no restart.

use std::collections::BTreeMap;
use std::sync::Arc;

use efr_protocol::{
    AccountState, AdminLoginApiKey, AdminLoginApiKeyResult, AdminLogout, AdminLogoutResult,
    AdminStatus, AdminStatusResult, ConversationId, Event, EventEnvelope, LimitKind, LimitReached,
    Method, Mode, ModelsList, ModelsListResult, Origin, OtherAccount, PromptSendResult, SecretText,
    TurnSettings,
};
use efr_stdx::time::Clock as _;
use efr_test_daemon::{
    ANTHROPIC_API_KEY, Client, FakeLauncher, MessagesAnswer, MessagesServer, TTY, TestDaemon,
    TestDaemonBuilder, events_until,
};
use efr_test_support::{TestDirs, Wait};
use pretty_assertions::assert_eq;

/// The Claude model of the Messages API's list.
const CLAUDE: &str = "claude-opus-5-5";

/// The key of the account `anthropic-api.work`.
const WORK_KEY: &str = "sk-ant-api03-efr-accounts-work-1a2b";

/// The key of the account `anthropic-api.home`.
const HOME_KEY: &str = "sk-ant-api03-efr-accounts-home-3c4d";

/// The answer of the Messages API for the usage limits that the user set in the
/// Console.
const USAGE_LIMITS: &str = "You have reached your specified API usage limits. You will regain access on 2026-12-01 at 00:00 UTC.";

fn login(provider: &str, key: &str) -> Method {
    Method::AdminLoginApiKey(AdminLoginApiKey {
        provider: provider.to_owned(),
        key: SecretText::new(key),
        check: false,
    })
}

fn logout(provider: &str) -> Method {
    Method::AdminLogout(AdminLogout { provider: provider.to_owned() })
}

async fn models(client: &Client) -> ModelsListResult {
    client.call(Method::ModelsList(ModelsList::default())).await.unwrap()
}

/// A daemon on `messages` with the default Anthropic key and the accounts `logins`,
/// each with its list of models.
async fn daemon_with(messages: &MessagesServer, logins: &[(&str, &str)]) -> (TestDaemon, Client) {
    built_with(TestDaemon::builder(), messages, logins).await
}

/// [`daemon_with`] from `builder`.
async fn built_with(
    builder: TestDaemonBuilder,
    messages: &MessagesServer,
    logins: &[(&str, &str)],
) -> (TestDaemon, Client) {
    messages.set_models(vec![MessagesServer::model(CLAUDE)]);
    let daemon = builder.messages(messages).start().await.unwrap();
    let client = daemon.client_for_tty(TTY).await.unwrap();
    for (provider, key) in logins {
        let _: AdminLoginApiKeyResult = client.call(login(provider, key)).await.unwrap();
    }
    let mut providers = vec!["anthropic-api"];
    providers.extend(logins.iter().map(|(provider, _)| *provider));
    Wait::new("the lists of the accounts")
        .until_some_async(async || {
            let listed = models(&client).await.models;
            providers
                .iter()
                .all(|id| listed.iter().any(|model| model.provider.as_deref() == Some(*id)))
                .then_some(())
        })
        .await
        .unwrap();
    (daemon, client)
}

/// Sends prompt `n` from `client` to `conversation` (none: the terminal's), with
/// `provider` as the account it names, and waits for the end of its turn; returns the
/// conversation and the turn's events.
async fn prompt(
    daemon: &TestDaemon,
    client: &Client,
    n: u128,
    conversation: Option<ConversationId>,
    provider: Option<&str>,
) -> (ConversationId, Vec<EventEnvelope>) {
    let Method::PromptSend(mut params) = daemon.prompt(n, &format!("prompt {n}"), TTY) else {
        unreachable!()
    };
    if conversation.is_some() {
        params.conversation_id = conversation;
        params.context = None;
    }
    params.settings = TurnSettings {
        model: provider.map(|_| CLAUDE.to_owned()),
        provider: provider.map(str::to_owned),
        ..TurnSettings::default()
    };
    let sent: PromptSendResult = client.call(Method::PromptSend(params)).await.unwrap();
    let terminal = daemon.client().await.unwrap();
    let mut follow = daemon.follow(&terminal, sent.conversation_id).await.unwrap();
    let turn_id = sent.turn_id;
    let events = events_until(&mut follow, |event| {
        matches!(
            event,
            Event::TurnCompleted { turn_id: t, .. } | Event::TurnFailed { turn_id: t, .. }
                if *t == turn_id
        )
    })
    .await
    .unwrap();
    let events =
        events.into_iter().filter(|envelope| envelope.event.turn_id() == Some(turn_id)).collect();
    (sent.conversation_id, events)
}

/// The account that ran the turn of `events`.
fn ran_on(events: &[EventEnvelope]) -> String {
    events
        .iter()
        .find_map(|envelope| match &envelope.event {
            Event::TurnStarted { settings: Some(settings), .. } => settings.provider.clone(),
            _ => None,
        })
        .expect("the turn recorded its account")
}

/// The accounts of the `model_switched` event of the turn of `events`.
fn switched(events: &[EventEnvelope]) -> Option<(String, String)> {
    events.iter().find_map(|envelope| match &envelope.event {
        Event::ModelSwitched { from, to, .. } => Some((
            from.provider.clone().unwrap_or_default(),
            to.provider.clone().unwrap_or_default(),
        )),
        _ => None,
    })
}

fn completed(events: &[EventEnvelope]) -> bool {
    events.last().is_some_and(|envelope| envelope.event.kind() == "turn_completed")
}

/// The `Authorization` header of the newest call of `messages`.
fn last_key(messages: &MessagesServer) -> String {
    messages.received().last().and_then(|request| request.authorization.clone()).unwrap()
}

fn bearer(key: &str) -> String {
    format!("Bearer {key}")
}

#[tokio::test]
async fn a_conversation_moves_to_another_account_of_its_model_and_back() {
    let messages = MessagesServer::start().await;
    let (daemon, client) = daemon_with(&messages, &[("anthropic-api.work", WORK_KEY)]).await;

    messages.push(MessagesAnswer::text("on the default account"));
    let (conversation, events) = prompt(&daemon, &client, 1, None, None).await;
    assert!(completed(&events), "{events:#?}");
    assert_eq!(ran_on(&events), "anthropic-api");
    assert_eq!(last_key(&messages), bearer(ANTHROPIC_API_KEY));

    messages.push(MessagesAnswer::text("on work"));
    let (moved, events) = prompt(&daemon, &client, 2, None, Some("work")).await;
    assert_eq!(moved, conversation, "one conversation");
    assert_eq!(ran_on(&events), "anthropic-api.work");
    assert_eq!(
        switched(&events),
        Some(("anthropic-api".to_owned(), "anthropic-api.work".to_owned())),
        "the same model on another account is a switch"
    );
    assert_eq!(last_key(&messages), bearer(WORK_KEY));

    messages.push(MessagesAnswer::text("still on work"));
    let (_, events) = prompt(&daemon, &client, 3, None, None).await;
    assert_eq!(ran_on(&events), "anthropic-api.work", "the conversation keeps the account");
    assert_eq!(switched(&events), None);

    messages.push(MessagesAnswer::text("back on the default account"));
    let (_, events) = prompt(&daemon, &client, 4, None, Some("anthropic")).await;
    assert_eq!(ran_on(&events), "anthropic-api");
    assert_eq!(
        switched(&events),
        Some(("anthropic-api.work".to_owned(), "anthropic-api".to_owned()))
    );
    let keys: Vec<String> =
        messages.received().into_iter().filter_map(|request| request.authorization).collect();
    assert_eq!(
        keys,
        [ANTHROPIC_API_KEY, WORK_KEY, WORK_KEY, ANTHROPIC_API_KEY].map(bearer),
        "each turn sent the key of its account"
    );
    let last = messages.received().last().unwrap().body.to_string();
    for says in ["on the default account", "on work", "still on work", "prompt 4"] {
        assert!(last.contains(says), "{says:?}: the history goes along");
    }
    drop(client);
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn a_logout_of_the_account_that_the_conversation_keeps_moves_the_next_turn_back() {
    let messages = MessagesServer::start().await;
    let (daemon, client) = daemon_with(&messages, &[("anthropic-api.work", WORK_KEY)]).await;
    messages.push(MessagesAnswer::text("on work"));
    let (conversation, events) = prompt(&daemon, &client, 1, None, Some("work")).await;
    assert_eq!(ran_on(&events), "anthropic-api.work");

    let out: AdminLogoutResult = client.call(logout("anthropic-api.work")).await.unwrap();
    assert!(!out.active, "{out:?}");
    messages.push(MessagesAnswer::text("after the logout"));
    let (same, events) = prompt(&daemon, &client, 2, None, None).await;

    assert_eq!(same, conversation);
    assert!(completed(&events), "{events:#?}");
    assert_eq!(ran_on(&events), "anthropic-api", "the router picks again");
    assert_eq!(
        switched(&events),
        Some(("anthropic-api.work".to_owned(), "anthropic-api".to_owned()))
    );
    assert_eq!(last_key(&messages), bearer(ANTHROPIC_API_KEY));
    drop(client);
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn a_phone_prompt_that_names_an_account_runs_on_it() {
    let messages = MessagesServer::start().await;
    let (daemon, client) = daemon_with(&messages, &[("anthropic-api.work", WORK_KEY)]).await;
    messages.push(MessagesAnswer::text("from the terminal"));
    let (conversation, events) = prompt(&daemon, &client, 1, None, None).await;
    assert_eq!(ran_on(&events), "anthropic-api");
    let phone = daemon.connect(daemon.connect_options_from(Origin::Phone)).await.unwrap();

    messages.push(MessagesAnswer::text("from the phone"));
    let (same, events) = prompt(&daemon, &phone, 2, Some(conversation), Some("work")).await;

    assert_eq!(same, conversation);
    assert!(completed(&events), "{events:#?}");
    assert_eq!(ran_on(&events), "anthropic-api.work");
    assert_eq!(last_key(&messages), bearer(WORK_KEY));
    // The terminal's next prompt keeps the account that the phone picked.
    messages.push(MessagesAnswer::text("the terminal again"));
    let (_, events) = prompt(&daemon, &client, 3, None, None).await;
    assert_eq!(ran_on(&events), "anthropic-api.work");
    drop((phone, client));
    daemon.stop().await.unwrap();
}

/// A turn that a job notice starts runs on the model and the account of the turn
/// before (`sticky.rs`): the wake turn of a conversation that keeps `work` sends the key
/// of `work`, with no switch. The job runs through the fake launcher (`/bin/sh`, no zsh)
/// and blocks on a FIFO until the prompt's turn ended.
#[tokio::test]
async fn a_wake_turn_runs_on_the_account_that_the_conversation_keeps() {
    let messages = MessagesServer::start().await;
    let dirs = Arc::new(TestDirs::new().unwrap());
    let fake = FakeLauncher::write(&dirs.root().join("fake")).unwrap();
    let fifo = dirs.root().join("go");
    nix::unistd::mkfifo(&fifo, nix::sys::stat::Mode::from_bits_truncate(0o600)).unwrap();
    let env = BTreeMap::from([
        ("PATH".to_owned(), "/usr/local/bin:/usr/bin:/bin".to_owned()),
        ("HOME".to_owned(), dirs.home().display().to_string()),
    ]);
    let builder = TestDaemon::builder()
        .dirs(Arc::clone(&dirs))
        .sandbox_launcher(fake.path())
        .probe_override(crate::sandbox::ready())
        .job_env(env);
    let (daemon, client) =
        built_with(builder, &messages, &[("anthropic-api.work", WORK_KEY)]).await;
    let command = format!("read answer < {}; echo ended", fifo.display());
    messages.push(MessagesAnswer::tool_use(
        "call_job",
        "shell",
        &serde_json::json!({ "command": command, "background": true }),
    ));
    messages.push(MessagesAnswer::text("the job runs"));
    // The wake turn ends at once.
    messages.push(MessagesAnswer::text("the job ended"));

    let Method::PromptSend(mut params) = daemon.prompt(1, "start the job", TTY) else {
        unreachable!()
    };
    params.settings = TurnSettings {
        model: Some(CLAUDE.to_owned()),
        provider: Some("work".to_owned()),
        mode: Some(Mode::Auto),
        ..TurnSettings::default()
    };
    let sent: PromptSendResult = client.call(Method::PromptSend(params)).await.unwrap();
    let mut follow = daemon.follow(&client, sent.conversation_id).await.unwrap();
    let first = events_until(
        &mut follow,
        |event| matches!(event, Event::TurnCompleted { turn_id, .. } if *turn_id == sent.turn_id),
    )
    .await
    .unwrap();
    assert!(
        first.iter().any(|envelope| matches!(envelope.event, Event::JobStarted { .. })),
        "{first:#?}"
    );
    let fifo_path = fifo.clone();
    tokio::task::spawn_blocking(move || std::fs::write(&fifo_path, b"go\n"))
        .await
        .unwrap()
        .unwrap();
    let mut wake = None;
    let events = events_until(&mut follow, |event| match event {
        Event::TurnStarted { turn_id, .. } if *turn_id != sent.turn_id => {
            wake = Some(*turn_id);
            false
        }
        Event::TurnCompleted { turn_id, .. } | Event::TurnFailed { turn_id, .. } => {
            Some(*turn_id) == wake
        }
        _ => false,
    })
    .await
    .unwrap();

    let wake = wake.expect("a job notice started a turn");
    let events: Vec<EventEnvelope> =
        events.into_iter().filter(|envelope| envelope.event.turn_id() == Some(wake)).collect();
    assert!(completed(&events), "{events:#?}");
    assert!(
        events
            .iter()
            .any(|envelope| matches!(&envelope.event, Event::JobNotice { woke: true, .. })),
        "{events:#?}"
    );
    assert_eq!(ran_on(&events), "anthropic-api.work");
    assert_eq!(switched(&events), None, "the wake turn keeps the account");
    assert_eq!(last_key(&messages), bearer(WORK_KEY));
    let keys: Vec<String> =
        messages.received().into_iter().filter_map(|request| request.authorization).collect();
    assert_eq!(keys, [WORK_KEY; 3].map(bearer), "each call of both turns sent the key of work");
    drop(client);
    daemon.stop().await.unwrap();
}

/// A 429 whose `requests` bucket is empty until `reset`.
fn requests_used_up(reset: &str) -> MessagesAnswer {
    // NOTE: a wait above the policy's longest ends the tries at once.
    MessagesAnswer::error(
        429,
        "rate_limit_error",
        "Number of requests has exceeded your rate limit",
    )
    .with_retry_after(120)
    .with_header("anthropic-ratelimit-requests-remaining", "0")
    .with_header("anthropic-ratelimit-requests-reset", reset)
}

#[tokio::test]
async fn a_full_account_whose_reset_comes_first_is_offered_with_its_reset() {
    let messages = MessagesServer::start().await;
    let logins = [("anthropic-api.work", WORK_KEY), ("anthropic-api.home", HOME_KEY)];
    let (daemon, client) = daemon_with(&messages, &logins).await;
    let soon = (daemon.clock().shared().now() + std::time::Duration::from_secs(180)).to_string();

    // work runs into its rate limit, with a reset in three minutes.
    messages.push(requests_used_up(&soon));
    let (_, events) = prompt(&daemon, &client, 1, None, Some("work")).await;
    let Event::TurnFailed { account_out: Some(out), .. } = &events.last().unwrap().event else {
        panic!("{events:#?}")
    };
    let work_out =
        LimitReached { kind: LimitKind::Requests, resets_at: Some(soon.parse().unwrap()) };
    assert_eq!(out.reached, work_out);

    // The default account reaches its usage limits, which end on the 1st.
    messages.push(MessagesAnswer::error(400, "invalid_request_error", USAGE_LIMITS));
    let (_, events) = prompt(&daemon, &client, 2, None, Some("anthropic")).await;
    let Event::TurnFailed { account_out: Some(out), .. } = &events.last().unwrap().event else {
        panic!("{events:#?}")
    };
    assert_eq!(out.provider, "anthropic-api");
    assert_eq!(out.reached.kind, LimitKind::SpendLimit);
    let other = |provider: &str, state, resets_at| OtherAccount {
        provider: provider.to_owned(),
        label: None,
        state,
        resets_at,
    };
    assert_eq!(
        out.others,
        [
            other("anthropic-api.work", AccountState::Full, work_out.resets_at),
            other("anthropic-api.home", AccountState::Ready, None),
        ],
        "work frees up before the default account"
    );
    let keys = messages.received().into_iter().filter_map(|request| request.authorization);
    assert_eq!(keys.collect::<Vec<_>>(), [bearer(WORK_KEY), bearer(ANTHROPIC_API_KEY)]);

    // A logout and a new login of work clear what efrd knew of its limits.
    let _: AdminLogoutResult = client.call(logout("anthropic-api.work")).await.unwrap();
    let _: AdminLoginApiKeyResult =
        client.call(login("anthropic-api.work", WORK_KEY)).await.unwrap();
    let status: AdminStatusResult =
        client.call(Method::AdminStatus(AdminStatus::default())).await.unwrap();
    let work = status.providers.iter().find(|p| p.provider == "anthropic-api.work").unwrap();
    assert!(work.logged_in);
    assert_eq!(work.limits, None, "the new login starts with no limit");
    messages.push(MessagesAnswer::text("work serves again"));
    let (_, events) = prompt(&daemon, &client, 3, None, Some("work")).await;
    assert!(completed(&events), "{events:#?}");
    assert_eq!(last_key(&messages), bearer(WORK_KEY));
    drop(client);
    daemon.stop().await.unwrap();
}

/// An account that `[accounts]` names stays in the model list when it signs out, so
/// the conversation keeps it: its turn fails as `unauthorized`, and a new login serves
/// the next one with no restart.
#[tokio::test]
async fn a_named_account_that_signs_out_keeps_the_conversation_until_a_new_login() {
    let messages = MessagesServer::start().await;
    messages.set_models(vec![MessagesServer::model(CLAUDE)]);
    let daemon = TestDaemon::builder()
        .messages(&messages)
        .config(|config| {
            config.accounts.labels.insert("anthropic-api.work".to_owned(), "Work".to_owned());
        })
        .start()
        .await
        .unwrap();
    let client = daemon.client_for_tty(TTY).await.unwrap();
    let _: AdminLoginApiKeyResult =
        client.call(login("anthropic-api.work", WORK_KEY)).await.unwrap();
    Wait::new("the list of work")
        .until_some_async(async || {
            let listed = models(&client).await.models;
            listed
                .iter()
                .any(|model| model.provider.as_deref() == Some("anthropic-api.work"))
                .then_some(())
        })
        .await
        .unwrap();
    messages.push(MessagesAnswer::text("on work"));
    let (_, events) = prompt(&daemon, &client, 1, None, Some("work")).await;
    assert_eq!(ran_on(&events), "anthropic-api.work");

    let _: AdminLogoutResult = client.call(logout("anthropic-api.work")).await.unwrap();
    let (_, events) = prompt(&daemon, &client, 2, None, None).await;
    let Event::TurnFailed { error, .. } = &events.last().unwrap().event else {
        panic!("{events:#?}")
    };
    assert_eq!(ran_on(&events), "anthropic-api.work", "efr never moves the account by itself");
    assert_eq!(error.code, efr_protocol::ErrorCode::Unauthorized, "{error:?}");
    assert_eq!(messages.received().len(), 1, "the signed-out account sent nothing");

    let _: AdminLoginApiKeyResult =
        client.call(login("anthropic-api.work", WORK_KEY)).await.unwrap();
    messages.push(MessagesAnswer::text("work again"));
    let (_, events) = prompt(&daemon, &client, 3, None, None).await;
    assert!(completed(&events), "{events:#?}");
    assert_eq!(ran_on(&events), "anthropic-api.work");
    assert_eq!(last_key(&messages), bearer(WORK_KEY));
    drop(client);
    daemon.stop().await.unwrap();
}

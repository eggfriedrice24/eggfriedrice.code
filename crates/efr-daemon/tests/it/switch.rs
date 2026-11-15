//! One conversation on two providers with no restart: the real `openai-api` provider
//! against a local Responses server, the provider of new conversations, and the real
//! `anthropic-api` provider against a local Messages API, both with a key. Each turn
//! goes to the provider of its model: a Claude turn that thinks, a GPT turn, then a
//! Claude turn again. Each request reaches only its own server and carries the text of
//! every earlier turn. The signed thinking of Claude never reaches OpenAI, and the
//! second Claude request sends it back, because it is the same model on the same
//! provider. Each turn records the provider that ran it. A reload that changes
//! `[model] provider` moves the next turn that names no model, with no restart. A
//! `[model] name` of the other company runs on its provider when that one has a key. A
//! login of a second account of a provider runs it at once, with its own key.
//!
//! An account that reaches a limit stops its turn with the cause on `turn_failed`: the
//! limit with its reset, and the other accounts that serve the model, ready or signed
//! out, with their labels; efrd never moves the turn. `conversation.switch_preview`
//! says what a switch costs: nothing for the same model on the same account, a cold
//! cache for another account or model, and a compaction for a window that the context
//! does not fit.

use efr_protocol::{
    AccountOut, AccountState, AdminConfigReload, AdminConfigReloadResult, AdminLoginApiKey,
    AdminLoginApiKeyResult, AdminLogout, AdminLogoutResult, AdminStatus, AdminStatusResult,
    ConversationId, ConversationSwitchPreview, ConversationSwitchPreviewResult, ErrorCode, Event,
    EventEnvelope, LimitKind, LimitReached, Method, ModelsList, ModelsListResult, OtherAccount,
    PromptSend, PromptSendResult, SecretText, ShellContext, SwitchCost, TurnSettings,
};
use efr_test_daemon::{
    ANTHROPIC_API_KEY, ClientError, MessagesAnswer, MessagesServer, ResponsesAnswer,
    ResponsesServer, TTY, TestDaemon, command_id, events_until,
};
use efr_test_support::Wait;
use pretty_assertions::assert_eq;
use serde_json::{Value, json};

/// The Claude model of the Messages API's list.
const CLAUDE: &str = "claude-opus-5-5";

/// The signature of the Claude turn's thinking.
const SIGNATURE: &str = "sig-efr-switch";

/// The members that a Messages body may have; anything else, such as OpenAI's
/// `prompt_cache_key`, is a 400 at the API.
const BODY_MEMBERS: &[&str] = &[
    "model",
    "stream",
    "max_tokens",
    "system",
    "tools",
    "tool_choice",
    "thinking",
    "output_config",
    "messages",
];

/// An answer that thinks, signs its thinking and says `text`.
fn think_then_say(text: &str) -> MessagesAnswer {
    let usage = json!({
        "input_tokens": 10, "cache_creation_input_tokens": 0, "cache_read_input_tokens": 0,
        "output_tokens": 1,
    });
    MessagesAnswer::events(&[
        (
            "message_start",
            json!({"message": {
                "id": "msg_efr", "type": "message", "role": "assistant", "content": [],
                "model": CLAUDE, "stop_reason": null, "stop_sequence": null, "usage": usage,
            }}),
        ),
        (
            "content_block_start",
            json!({"index": 0, "content_block": {"type": "thinking", "thinking": "", "signature": ""}}),
        ),
        (
            "content_block_delta",
            json!({"index": 0, "delta": {"type": "thinking_delta", "thinking": "A short greeting."}}),
        ),
        (
            "content_block_delta",
            json!({"index": 0, "delta": {"type": "signature_delta", "signature": SIGNATURE}}),
        ),
        ("content_block_stop", json!({"index": 0})),
        ("content_block_start", json!({"index": 1, "content_block": {"type": "text", "text": ""}})),
        ("content_block_delta", json!({"index": 1, "delta": {"type": "text_delta", "text": text}})),
        ("content_block_stop", json!({"index": 1})),
        (
            "message_delta",
            json!({"delta": {"stop_reason": "end_turn", "stop_sequence": null}, "usage": {"output_tokens": 5}}),
        ),
        ("message_stop", json!({})),
    ])
}

/// Sends prompt `n` with `model` (none: the default) and waits for the end of its
/// turn; returns the turn's events.
async fn turn(
    daemon: &TestDaemon,
    n: u128,
    text: &str,
    model: Option<&str>,
) -> (ConversationId, Vec<EventEnvelope>) {
    let client = daemon.client_for_tty(TTY).await.unwrap();
    let mut context = ShellContext::new(daemon.cwd());
    context.tty = Some(TTY.to_owned());
    let prompt = Method::PromptSend(PromptSend {
        command_id: command_id(n),
        conversation_id: None,
        new_conversation: false,
        text: text.to_owned(),
        context: Some(context),
        last_command: None,
        recent_commands: Vec::new(),
        settings: TurnSettings { model: model.map(str::to_owned), ..TurnSettings::default() },
    });
    let sent: PromptSendResult = client.call(prompt).await.unwrap();
    let mut follow = daemon.follow(&client, sent.conversation_id).await.unwrap();
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
    let events: Vec<_> =
        events.into_iter().filter(|envelope| envelope.event.turn_id() == Some(turn_id)).collect();
    assert_eq!(events.last().unwrap().event.kind(), "turn_completed", "{events:#?}");
    (sent.conversation_id, events)
}

/// The provider and the model that the turn of `events` recorded.
fn ran_on(events: &[EventEnvelope]) -> (String, String) {
    let settings = events
        .iter()
        .find_map(|envelope| match &envelope.event {
            Event::TurnStarted { settings, .. } => settings.clone(),
            _ => None,
        })
        .unwrap();
    (settings.provider.unwrap(), settings.model)
}

async fn models(daemon: &TestDaemon) -> ModelsListResult {
    let client = daemon.client().await.unwrap();
    client.call(Method::ModelsList(ModelsList::default())).await.unwrap()
}

#[tokio::test]
async fn a_conversation_moves_from_claude_to_gpt_and_back_with_no_restart() {
    let responses = ResponsesServer::start().await;
    let messages = MessagesServer::start().await;
    messages.set_models(vec![MessagesServer::model(CLAUDE)]);
    let daemon =
        TestDaemon::builder().responses(&responses).also_messages(&messages).start().await.unwrap();
    // NOTE: no wait for the Claude list: the first prompt on a Claude model waits for
    // the first fetch of the list of Anthropic, which has a key.
    messages.push(think_then_say("claude answer one"));
    let (first, events) = turn(&daemon, 1, "prompt one", Some(CLAUDE)).await;
    assert_eq!(ran_on(&events), ("anthropic-api".to_owned(), CLAUDE.to_owned()));

    // NOTE: the conversation keeps Claude, its pick, so the GPT turn names its model.
    let gpt_model = models(&daemon)
        .await
        .models
        .into_iter()
        .find(|model| model.default)
        .map(|model| model.id)
        .unwrap();
    responses.push(ResponsesAnswer::text("gpt answer two"));
    let (second, events) = turn(&daemon, 2, "prompt two", Some(&gpt_model)).await;
    assert_eq!(second, first, "one conversation");
    let (provider, gpt) = ran_on(&events);
    assert_eq!(provider, "openai-api");
    assert!(gpt.starts_with("gpt-"), "{gpt}");

    messages.push(MessagesAnswer::text("claude answer three"));
    let (third, events) = turn(&daemon, 3, "prompt three", Some(CLAUDE)).await;
    let switched: Vec<_> = events
        .iter()
        .filter_map(|envelope| match &envelope.event {
            Event::ModelSwitched { from, to, .. } => {
                Some((from.provider.clone(), to.provider.clone()))
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        switched,
        [(Some("openai-api".to_owned()), Some("anthropic-api".to_owned()))],
        "the turn back to Claude records the switch"
    );
    assert_eq!(third, first, "one conversation");
    assert_eq!(ran_on(&events), ("anthropic-api".to_owned(), CLAUDE.to_owned()));

    let claude: Vec<Value> = messages.received().into_iter().map(|request| request.body).collect();
    let openai: Vec<Value> = responses.received().into_iter().map(|request| request.body).collect();
    assert_eq!((claude.len(), openai.len()), (2, 1), "each turn reached only its own server");

    let gpt_body = openai[0].to_string();
    for says in ["prompt one", "claude answer one", "prompt two"] {
        assert!(gpt_body.contains(says), "{says:?} in {gpt_body}");
    }
    assert!(!gpt_body.contains(SIGNATURE), "Claude's thinking stays at Anthropic: {gpt_body}");
    assert!(!gpt_body.contains("A short greeting."), "{gpt_body}");

    let last = &claude[1];
    let text = last.to_string();
    for says in ["prompt one", "claude answer one", "prompt two", "gpt answer two", "prompt three"]
    {
        assert!(text.contains(says), "{says:?} in {text}");
    }
    assert!(text.contains(SIGNATURE), "the same model on the same provider gets its thinking back");
    for body in &claude {
        let members: Vec<&String> = body.as_object().unwrap().keys().collect();
        assert!(
            members.iter().all(|member| BODY_MEMBERS.contains(&member.as_str())),
            "{members:?}"
        );
    }
}

#[tokio::test]
async fn a_reload_that_changes_the_provider_moves_the_next_turn_with_no_restart() {
    let responses = ResponsesServer::start().await;
    let messages = MessagesServer::start().await;
    messages.set_models(vec![MessagesServer::model(CLAUDE)]);
    let daemon =
        TestDaemon::builder().responses(&responses).also_messages(&messages).start().await.unwrap();
    Wait::new("the Claude model in the list")
        .until_some_async(async || {
            models(&daemon).await.models.iter().any(|model| model.id == CLAUDE).then_some(())
        })
        .await
        .unwrap();
    responses.push(ResponsesAnswer::text("gpt answer one"));
    let (_, events) = turn(&daemon, 1, "prompt one", None).await;
    assert_eq!(ran_on(&events).0, "openai-api");

    let config = daemon.dirs().dirs().config().join("config.toml");
    std::fs::create_dir_all(daemon.dirs().dirs().config()).unwrap();
    std::fs::write(&config, "[model]\nprovider = \"anthropic-api\"\n").unwrap();
    let client = daemon.client().await.unwrap();
    let reloaded: AdminConfigReloadResult =
        client.call(Method::AdminConfigReload(AdminConfigReload::default())).await.unwrap();
    assert!(reloaded.applied, "{reloaded:?}");
    // NOTE: the test's base URLs are not in the file; they wait for a restart, the
    // provider does not.
    assert!(!reloaded.restart_needed.iter().any(|key| key == "model.provider"), "{reloaded:?}");

    let list = models(&daemon).await;
    let default = list.models.iter().find(|model| model.default).unwrap();
    assert_eq!(
        (default.id.as_str(), default.provider.as_deref()),
        (CLAUDE, Some("anthropic-api")),
        "the default model is the one of the new provider"
    );
    messages.push(MessagesAnswer::text("claude answer two"));
    let (_, events) = turn(&daemon, 2, "prompt two", None).await;
    assert_eq!(ran_on(&events), ("anthropic-api".to_owned(), CLAUDE.to_owned()));
    assert_eq!(messages.received().len(), 1);
    assert_eq!(responses.received().len(), 1);
}

#[tokio::test]
async fn a_default_model_of_the_other_company_runs_on_its_provider_when_it_has_a_login() {
    let responses = ResponsesServer::start().await;
    let messages = MessagesServer::start().await;
    messages.set_models(vec![MessagesServer::model(CLAUDE)]);
    let daemon = TestDaemon::builder()
        .responses(&responses)
        .also_messages(&messages)
        .config(|config| config.model.name = Some(CLAUDE.to_owned()))
        .start()
        .await
        .unwrap();

    messages.push(MessagesAnswer::text("claude answer"));
    let (_, events) = turn(&daemon, 1, "prompt one", None).await;

    assert_eq!(ran_on(&events), ("anthropic-api".to_owned(), CLAUDE.to_owned()));
    assert_eq!(messages.received().len(), 1);
    assert!(responses.received().is_empty());
}

/// The second Anthropic key of the account tests.
const WORK_KEY: &str = "sk-ant-api03-efr-work-c3d4";

#[tokio::test]
async fn a_login_of_a_second_account_runs_it_at_once_and_a_turn_uses_its_key() {
    let messages = MessagesServer::start().await;
    messages.set_models(vec![MessagesServer::model(CLAUDE)]);
    let daemon = TestDaemon::builder().messages(&messages).start().await.unwrap();
    let client = daemon.client().await.unwrap();

    let login = |provider: &str, key: &str| {
        Method::AdminLoginApiKey(AdminLoginApiKey {
            provider: provider.to_owned(),
            key: SecretText::new(key),
            check: false,
        })
    };
    let stored: AdminLoginApiKeyResult =
        client.call(login("anthropic-api.work", WORK_KEY)).await.unwrap();
    assert_eq!(stored.provider, "anthropic-api.work");
    assert!(!stored.active);
    // The key of the default account under a second name has no use.
    let twice = client
        .call::<AdminLoginApiKeyResult>(login("anthropic-api.again", ANTHROPIC_API_KEY))
        .await
        .unwrap_err();
    let ClientError::Server { body } = twice else { panic!("{twice:?}") };
    assert!(body.message.contains("the key of anthropic-api already"), "{}", body.message);

    let status: AdminStatusResult =
        client.call(Method::AdminStatus(AdminStatus::default())).await.unwrap();
    let work = status.providers.iter().find(|p| p.provider == "anthropic-api.work").unwrap();
    assert!(work.logged_in);
    assert_eq!(work.key_hint.as_deref(), Some("sk-ant-...c3d4"));
    Wait::new("the list of the work account")
        .until_some_async(async || {
            models(&daemon)
                .await
                .models
                .iter()
                .any(|model| model.provider.as_deref() == Some("anthropic-api.work"))
                .then_some(())
        })
        .await
        .unwrap();

    let config = daemon.dirs().dirs().config().join("config.toml");
    std::fs::write(&config, "[model]\nprovider = \"anthropic-api.work\"\n").unwrap();
    let _: AdminConfigReloadResult =
        client.call(Method::AdminConfigReload(AdminConfigReload::default())).await.unwrap();
    messages.push(MessagesAnswer::text("work answer"));
    let (_, events) = turn(&daemon, 1, "prompt one", None).await;

    assert_eq!(ran_on(&events), ("anthropic-api.work".to_owned(), CLAUDE.to_owned()));
    let sent = messages.received();
    assert_eq!(sent.last().unwrap().authorization, Some(format!("Bearer {WORK_KEY}")));
}

#[tokio::test]
async fn a_prompt_names_the_work_account_and_the_conversation_stays_on_it() {
    let messages = MessagesServer::start().await;
    messages.set_models(vec![MessagesServer::model(CLAUDE)]);
    let daemon = TestDaemon::builder().messages(&messages).start().await.unwrap();
    let client = daemon.client().await.unwrap();
    let login = Method::AdminLoginApiKey(AdminLoginApiKey {
        provider: "anthropic-api.work".to_owned(),
        key: SecretText::new(WORK_KEY),
        check: false,
    });
    let _: AdminLoginApiKeyResult = client.call(login).await.unwrap();
    Wait::new("the list of the work account")
        .until_some_async(async || {
            models(&daemon)
                .await
                .models
                .iter()
                .any(|model| model.provider.as_deref() == Some("anthropic-api.work"))
                .then_some(())
        })
        .await
        .unwrap();

    messages.push(MessagesAnswer::text("work one"));
    let mut context = ShellContext::new(daemon.cwd());
    context.tty = Some(TTY.to_owned());
    let prompt = Method::PromptSend(PromptSend {
        command_id: command_id(1),
        conversation_id: None,
        new_conversation: false,
        text: "prompt one".to_owned(),
        context: Some(context),
        last_command: None,
        recent_commands: Vec::new(),
        settings: TurnSettings {
            model: Some(CLAUDE.to_owned()),
            provider: Some("work".to_owned()),
            ..TurnSettings::default()
        },
    });
    let client = daemon.client_for_tty(TTY).await.unwrap();
    let sent: PromptSendResult = client.call(prompt).await.unwrap();
    assert_eq!(sent.settings.unwrap().provider.as_deref(), Some("anthropic-api.work"));
    let mut follow = daemon.follow(&client, sent.conversation_id).await.unwrap();
    let turn_id = sent.turn_id;
    events_until(
        &mut follow,
        |event| matches!(event, Event::TurnCompleted { turn_id: t, .. } if *t == turn_id),
    )
    .await
    .unwrap();

    messages.push(MessagesAnswer::text("work two"));
    let (_, events) = turn(&daemon, 2, "prompt two", None).await;
    assert_eq!(ran_on(&events), ("anthropic-api.work".to_owned(), CLAUDE.to_owned()));
    let keys: Vec<Option<String>> =
        messages.received().into_iter().map(|request| request.authorization).collect();
    assert_eq!(keys, [Some(format!("Bearer {WORK_KEY}")), Some(format!("Bearer {WORK_KEY}"))]);
}

#[tokio::test]
async fn a_prompt_names_an_account_whose_list_is_still_missing() {
    let messages = MessagesServer::start().await;
    // NOTE: a 400 fails the fetch at once; a 503 would wait for its retry.
    messages.refuse_models(MessagesAnswer::error(400, "invalid_request_error", "no list"));
    let daemon = TestDaemon::builder()
        .messages(&messages)
        .config(|config| config.model.max_output_tokens = Some(1000))
        .start()
        .await
        .unwrap();
    let client = daemon.client().await.unwrap();
    let login = Method::AdminLoginApiKey(AdminLoginApiKey {
        provider: "anthropic-api.work".to_owned(),
        key: SecretText::new(WORK_KEY),
        check: false,
    });
    let _: AdminLoginApiKeyResult = client.call(login).await.unwrap();
    let prompt = |n: u128, provider: &str| {
        let mut context = ShellContext::new(daemon.cwd());
        context.tty = Some(TTY.to_owned());
        Method::PromptSend(PromptSend {
            command_id: command_id(n),
            conversation_id: None,
            new_conversation: false,
            text: "hi".to_owned(),
            context: Some(context),
            last_command: None,
            recent_commands: Vec::new(),
            settings: TurnSettings {
                model: Some(CLAUDE.to_owned()),
                provider: Some(provider.to_owned()),
                ..TurnSettings::default()
            },
        })
    };
    let client = daemon.client_for_tty(TTY).await.unwrap();

    let unknown = client.call::<PromptSendResult>(prompt(1, "home")).await.unwrap_err();
    let ClientError::Server { body } = unknown else { panic!("{unknown:?}") };
    assert_eq!(
        body.message,
        "the provider home is no provider with a login; choose one of: anthropic-api, anthropic-api.work"
    );
    messages.push(MessagesAnswer::text("work answer"));
    let sent: PromptSendResult = client.call(prompt(2, "work")).await.unwrap();
    assert_eq!(sent.settings.unwrap().provider.as_deref(), Some("anthropic-api.work"));
}

/// The third Anthropic key of the account tests.
const HOME_KEY: &str = "sk-ant-api03-efr-home-e5f6";

/// The answer of the Messages API for the usage limits that the user set in the
/// Console.
const USAGE_LIMITS: &str = "You have reached your specified API usage limits. You will regain access on 2026-12-01 at 00:00 UTC.";

/// A login with `key` for `provider`, with no check.
fn login(provider: &str, key: &str) -> Method {
    Method::AdminLoginApiKey(AdminLoginApiKey {
        provider: provider.to_owned(),
        key: SecretText::new(key),
        check: false,
    })
}

/// Sends prompt `n` with `model` on `provider` (none: as the prompt picks them) and
/// waits for the end of its turn, whatever ended it; returns the turn's events.
async fn ending(
    daemon: &TestDaemon,
    n: u128,
    model: Option<&str>,
    provider: Option<&str>,
) -> (ConversationId, Vec<EventEnvelope>) {
    let client = daemon.client_for_tty(TTY).await.unwrap();
    let mut context = ShellContext::new(daemon.cwd());
    context.tty = Some(TTY.to_owned());
    let prompt = Method::PromptSend(PromptSend {
        command_id: command_id(n),
        conversation_id: None,
        new_conversation: false,
        text: format!("prompt {n}"),
        context: Some(context),
        last_command: None,
        recent_commands: Vec::new(),
        settings: TurnSettings {
            model: model.map(str::to_owned),
            provider: provider.map(str::to_owned),
            ..TurnSettings::default()
        },
    });
    let sent: PromptSendResult = client.call(prompt).await.unwrap();
    let mut follow = daemon.follow(&client, sent.conversation_id).await.unwrap();
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

/// Waits until `models.list` holds a model of each of `providers`.
async fn lists_of(daemon: &TestDaemon, providers: &[&str]) {
    Wait::new("the lists of the accounts")
        .until_some_async(async || {
            let listed = models(daemon).await.models;
            providers
                .iter()
                .all(|id| listed.iter().any(|model| model.provider.as_deref() == Some(*id)))
                .then_some(())
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn an_account_at_its_limit_stops_the_turn_and_offers_the_accounts_of_the_model() {
    let messages = MessagesServer::start().await;
    messages.set_models(vec![MessagesServer::model(CLAUDE)]);
    let daemon = TestDaemon::builder()
        .messages(&messages)
        .config(|config| {
            let labels = &mut config.accounts.labels;
            labels.insert("anthropic-api.work".to_owned(), "Work".to_owned());
            labels.insert("anthropic-api.home".to_owned(), "Home".to_owned());
        })
        .start()
        .await
        .unwrap();
    let client = daemon.client().await.unwrap();
    for (provider, key) in [("anthropic-api.work", WORK_KEY), ("anthropic-api.home", HOME_KEY)] {
        let _: AdminLoginApiKeyResult = client.call(login(provider, key)).await.unwrap();
    }
    lists_of(&daemon, &["anthropic-api", "anthropic-api.work", "anthropic-api.home"]).await;
    // NOTE: home signs out; it keeps its last list, and `[accounts]` names it.
    let logout = Method::AdminLogout(AdminLogout { provider: "anthropic-api.home".to_owned() });
    let _: AdminLogoutResult = client.call(logout).await.unwrap();
    let catalogs: Vec<(Option<String>, Option<String>, bool)> = models(&daemon)
        .await
        .catalogs
        .into_iter()
        .map(|status| (status.provider, status.label, status.signed_out))
        .collect();
    let some = |text: &str| Some(text.to_owned());
    assert_eq!(
        catalogs,
        [
            (some("anthropic-api"), None, false),
            (some("anthropic-api.work"), some("Work"), false),
            (some("anthropic-api.home"), some("Home"), true),
        ]
    );

    messages.push(MessagesAnswer::error(400, "invalid_request_error", USAGE_LIMITS));
    let (_, events) = ending(&daemon, 1, Some(CLAUDE), None).await;

    let Event::TurnFailed { error, account_out, .. } = &events.last().unwrap().event else {
        panic!("{events:#?}")
    };
    assert_eq!(error.code, ErrorCode::Busy);
    assert_eq!(error.message, format!("the account reached its spend limit: {USAGE_LIMITS}"));
    let reached = LimitReached {
        kind: LimitKind::SpendLimit,
        resets_at: Some("2026-12-01T00:00:00Z".parse().unwrap()),
    };
    let other = |provider: &str, label: &str, state| OtherAccount {
        provider: provider.to_owned(),
        label: some(label),
        state,
        resets_at: None,
    };
    assert_eq!(
        account_out.as_ref(),
        Some(&AccountOut {
            provider: "anthropic-api".to_owned(),
            label: None,
            model: CLAUDE.to_owned(),
            reached: reached.clone(),
            others: vec![
                other("anthropic-api.work", "Work", AccountState::Ready),
                other("anthropic-api.home", "Home", AccountState::SignedOut),
            ],
        })
    );
    let keys: Vec<Option<String>> =
        messages.received().into_iter().map(|request| request.authorization).collect();
    assert_eq!(keys, [some(&format!("Bearer {ANTHROPIC_API_KEY}"))], "one try, no switch");
    let listed = models(&daemon).await.catalogs;
    let limits = |provider: &str| {
        listed
            .iter()
            .find(|status| status.provider.as_deref() == Some(provider))
            .and_then(|status| status.limits.clone())
            .and_then(|limits| limits.reached)
    };
    assert_eq!(limits("anthropic-api"), Some(reached.clone()));
    assert_eq!(limits("anthropic-api.work"), None);
    let status: AdminStatusResult =
        client.call(Method::AdminStatus(AdminStatus::default())).await.unwrap();
    let account = |id: &str| status.providers.iter().find(|p| p.provider == id).unwrap();
    assert_eq!(
        account("anthropic-api").limits.as_ref().and_then(|l| l.reached.clone()),
        Some(reached)
    );

    // The user switches by naming the account; its own key serves the turn.
    messages.push(MessagesAnswer::text("work answer"));
    let (_, events) = ending(&daemon, 2, Some(CLAUDE), Some("work")).await;
    assert_eq!(events.last().unwrap().event.kind(), "turn_completed", "{events:#?}");
    let sent = messages.received();
    assert_eq!(sent.last().unwrap().authorization, some(&format!("Bearer {WORK_KEY}")));
}

#[tokio::test]
async fn a_switch_preview_names_the_cost_of_the_target() {
    let messages = MessagesServer::start().await;
    messages.set_models(vec![MessagesServer::model(CLAUDE)]);
    let tiny: efr_config::ModelEntry =
        serde_json::from_value(json!({ "id": "claude-tiny", "context_window": 1000 })).unwrap();
    let daemon = TestDaemon::builder()
        .messages(&messages)
        .config(move |config| config.anthropic.models = Some(vec![tiny]))
        .start()
        .await
        .unwrap();
    let client = daemon.client().await.unwrap();
    let _: AdminLoginApiKeyResult =
        client.call(login("anthropic-api.work", WORK_KEY)).await.unwrap();
    lists_of(&daemon, &["anthropic-api", "anthropic-api.work"]).await;
    messages.push(MessagesAnswer::text("claude answer"));
    let (conversation_id, events) = ending(&daemon, 1, Some(CLAUDE), None).await;
    assert_eq!(events.last().unwrap().event.kind(), "turn_completed", "{events:#?}");
    let preview = async |model: &str, provider: Option<&str>| {
        let method = Method::ConversationSwitchPreview(ConversationSwitchPreview {
            conversation_id,
            model: model.to_owned(),
            provider: provider.map(str::to_owned),
        });
        let result: ConversationSwitchPreviewResult = client.call(method).await.unwrap();
        (result.cost, result.provider)
    };
    let on = |provider: &str| Some(provider.to_owned());

    assert_eq!(preview(CLAUDE, None).await, (SwitchCost::None, on("anthropic-api")));
    assert_eq!(preview(CLAUDE, Some("anthropic")).await, (SwitchCost::None, on("anthropic-api")));
    assert_eq!(
        preview(CLAUDE, Some("work")).await,
        (SwitchCost::ColdCache, on("anthropic-api.work")),
        "the same model on another account"
    );
    assert_eq!(
        preview("claude-tiny", None).await,
        (SwitchCost::Compacts, on("anthropic-api")),
        "the context does not fit a window of 1000 tokens"
    );
    assert_eq!(preview(CLAUDE, Some("nobody")).await, (SwitchCost::Unknown, None));
    let unknown = Method::ConversationSwitchPreview(ConversationSwitchPreview {
        conversation_id: ConversationId::from_uuid(uuid::Uuid::nil()),
        model: CLAUDE.to_owned(),
        provider: None,
    });
    let refused = client.call::<ConversationSwitchPreviewResult>(unknown).await.unwrap_err();
    let ClientError::Server { body } = refused else { panic!("{refused:?}") };
    assert_eq!(body.code, ErrorCode::NotFound);
    assert_eq!(messages.received().len(), 1, "a preview sends no model call");
}

/// With `compaction.auto` off, the next turn refuses a request above the hard cap and
/// compacts nothing, so the preview does not promise a compaction.
#[tokio::test]
async fn a_switch_preview_with_auto_compaction_off_promises_no_compaction() {
    let messages = MessagesServer::start().await;
    messages.set_models(vec![MessagesServer::model(CLAUDE)]);
    let tiny: efr_config::ModelEntry =
        serde_json::from_value(json!({ "id": "claude-tiny", "context_window": 1000 })).unwrap();
    let daemon = TestDaemon::builder()
        .messages(&messages)
        .config(move |config| {
            config.anthropic.models = Some(vec![tiny]);
            config.compaction.auto = false;
        })
        .start()
        .await
        .unwrap();
    let client = daemon.client().await.unwrap();
    messages.push(MessagesAnswer::text("claude answer"));
    let (conversation_id, events) = ending(&daemon, 1, Some(CLAUDE), None).await;
    assert_eq!(events.last().unwrap().event.kind(), "turn_completed", "{events:#?}");
    let method = Method::ConversationSwitchPreview(ConversationSwitchPreview {
        conversation_id,
        model: "claude-tiny".to_owned(),
        provider: None,
    });
    let result: ConversationSwitchPreviewResult = client.call(method).await.unwrap();

    assert_eq!(result.cost, SwitchCost::ColdCache, "the turn would say the context is full");
}

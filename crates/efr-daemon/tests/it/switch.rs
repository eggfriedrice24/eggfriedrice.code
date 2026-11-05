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

use efr_protocol::{
    AdminConfigReload, AdminConfigReloadResult, AdminLoginApiKey, AdminLoginApiKeyResult,
    AdminStatus, AdminStatusResult, ConversationId, Event, EventEnvelope, Method, ModelsList,
    ModelsListResult, PromptSend, PromptSendResult, SecretText, ShellContext, TurnSettings,
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

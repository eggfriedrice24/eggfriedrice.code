//! The limits of each account over a `TestDaemon`, from the answers of the real
//! providers against local servers.
//!
//! A ChatGPT plan tells its usage windows in the headers of each answer: efrd keeps
//! them for the account, with the time it learned them, and serves them in
//! `admin.status` and `models.list`. A logout clears them, and a signed-out account
//! shows none. On the Responses WebSocket, the default transport of a plan, the
//! `codex.rate_limits` message and the wrapped `error` event of a used-up plan do the
//! same ([`ProviderServer`]). An API key shows a rate limit only when it was hit: a 429
//! that outlived the tries and whose headers show an empty bucket stops the turn with
//! that bucket and its reset, the status shows it, and the next call that works clears
//! it.

use efr_protocol::{
    AccountLimits, AdminStatus, AdminStatusResult, Event, EventEnvelope, LimitKind, LimitReached,
    LimitWindow, Method, ModelsList, ModelsListResult, PromptSendResult,
};
use efr_stdx::time::Clock as _;
use efr_test_daemon::{
    Client, MessagesAnswer, MessagesServer, ProviderServer, ResponsesAnswer, ResponsesServer,
    SUBSCRIPTION_CREDENTIAL, Saw, TTY, TestDaemon, events_until,
};
use pretty_assertions::assert_eq;

/// Sends prompt `n` and waits for the end of its turn; returns the turn's events.
async fn turn(daemon: &TestDaemon, client: &Client, n: u128) -> Vec<EventEnvelope> {
    let sent: PromptSendResult = client.call(daemon.prompt(n, "hello", TTY)).await.unwrap();
    let mut follow = daemon.follow(client, sent.conversation_id).await.unwrap();
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
    events.into_iter().filter(|envelope| envelope.event.turn_id() == Some(turn_id)).collect()
}

/// The limits of `provider` in `admin.status` and in the catalogs of `models.list`.
async fn limits_of(
    client: &Client,
    provider: &str,
) -> (Option<AccountLimits>, Option<AccountLimits>) {
    let status: AdminStatusResult =
        client.call(Method::AdminStatus(AdminStatus::default())).await.unwrap();
    let status = status
        .providers
        .into_iter()
        .find(|status| status.provider == provider)
        .and_then(|status| status.limits);
    let list: ModelsListResult =
        client.call(Method::ModelsList(ModelsList::default())).await.unwrap();
    let listed = list
        .catalogs
        .into_iter()
        .chain(list.catalog)
        .find(|catalog| catalog.provider.as_deref() == Some(provider))
        .and_then(|catalog| catalog.limits);
    (status, listed)
}

#[tokio::test]
async fn the_windows_of_a_plan_show_after_a_call_and_a_logout_clears_them() {
    let server = ResponsesServer::start().await;
    let daemon = TestDaemon::builder().subscription(&server).start().await.unwrap();
    let client = daemon.client_for_tty(TTY).await.unwrap();
    assert_eq!(limits_of(&client, "openai-subscription").await, (None, None), "no call yet");

    let resets_at = daemon.clock().shared().now().as_second() + 3600;
    server.push(
        ResponsesAnswer::text("hi")
            .with_header("x-codex-primary-used-percent", "92.0")
            .with_header("x-codex-primary-window-minutes", "300")
            .with_header("x-codex-primary-reset-at", &resets_at.to_string())
            .with_header("x-codex-secondary-used-percent", "41.0")
            .with_header("x-codex-secondary-window-minutes", "10080"),
    );
    let learned = daemon.clock().shared().now();
    let events = turn(&daemon, &client, 1).await;
    assert_eq!(events.last().unwrap().event.kind(), "turn_completed", "{events:#?}");

    let (status, listed) = limits_of(&client, "openai-subscription").await;
    let status = status.expect("the windows in admin.status");
    assert_eq!(
        status.windows,
        [
            LimitWindow {
                minutes: Some(300),
                used_percent: 92,
                resets_at: Some(jiff::Timestamp::from_second(resets_at).unwrap()),
            },
            LimitWindow { minutes: Some(10080), used_percent: 41, resets_at: None },
        ]
    );
    assert_eq!(status.reached, None);
    assert_eq!(status.observed_at, learned, "the time of the answer, on the daemon's clock");
    assert_eq!(listed, Some(status), "the phone reads the same in models.list");

    // The login goes away outside efrd: the account is signed out and shows no limits.
    std::fs::remove_file(daemon.dirs().dirs().data().join(SUBSCRIPTION_CREDENTIAL)).unwrap();
    assert_eq!(limits_of(&client, "openai-subscription").await, (None, None));
    drop(client);
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn a_plan_at_its_usage_limit_stops_the_turn_after_one_try_with_its_reset() {
    let server = ResponsesServer::start().await;
    let daemon = TestDaemon::builder().subscription(&server).start().await.unwrap();
    let client = daemon.client_for_tty(TTY).await.unwrap();
    let resets_at = daemon.clock().shared().now().as_second() + 1200;
    let body = serde_json::json!({"error": {
        "type": "usage_limit_reached",
        "message": "You have hit your usage limit.",
        "plan_type": "plus",
        "resets_at": resets_at,
    }});
    server.push(ResponsesAnswer::new(429, body.to_string()));

    let events = turn(&daemon, &client, 1).await;

    let Event::TurnFailed { account_out, .. } = &events.last().unwrap().event else {
        panic!("{events:#?}")
    };
    let reached = LimitReached {
        kind: LimitKind::Usage,
        resets_at: Some(jiff::Timestamp::from_second(resets_at).unwrap()),
    };
    let out = account_out.as_ref().expect("the cause of the stop");
    assert_eq!((out.provider.as_str(), &out.reached), ("openai-subscription", &reached));
    assert_eq!(server.received().len(), 1, "a usage limit is never sent again");
    let (status, listed) = limits_of(&client, "openai-subscription").await;
    assert_eq!(status.as_ref().and_then(|limits| limits.reached.clone()), Some(reached));
    assert_eq!(listed, status);
    drop(client);
    daemon.stop().await.unwrap();
}

/// The kinds of what `server` saw since the last drain.
fn seen(server: &ProviderServer) -> Vec<Saw> {
    server.drain().into_iter().map(|seen| seen.what).collect()
}

/// A daemon whose ChatGPT plan runs over the Responses WebSocket of `server`, the
/// default transport (`websocket = "auto"`).
async fn on_the_websocket(server: &ProviderServer) -> TestDaemon {
    TestDaemon::builder()
        .subscription_at(&server.base_url("sub"), &server.issuer())
        .start()
        .await
        .unwrap()
}

/// Limits research 2.3 on the WebSocket: the `codex.rate_limits` message before an
/// answer gives the windows of the plan, which `admin.status` and `models.list` serve.
#[tokio::test]
async fn the_windows_of_a_plan_on_the_websocket_show_in_the_status() {
    let server = ProviderServer::start().await.unwrap();
    let daemon = on_the_websocket(&server).await;
    let client = daemon.client_for_tty(TTY).await.unwrap();
    server.set(|knobs| {
        knobs.ws_first_once = vec![serde_json::json!({
            "type": "codex.rate_limits",
            "rate_limits": {
                "primary": {"used_percent": 92.4, "window_minutes": 300},
                "secondary": {"used_percent": 41, "window_minutes": 10080},
            },
        })];
    });
    server.drain();

    let learned = daemon.clock().shared().now();
    let events = turn(&daemon, &client, 1).await;

    assert_eq!(events.last().unwrap().event.kind(), "turn_completed", "{events:#?}");
    let saw = seen(&server);
    assert!(saw.contains(&Saw::Create) && !saw.contains(&Saw::Post), "{saw:?}");
    let (status, listed) = limits_of(&client, "openai-subscription").await;
    let status = status.expect("the windows in admin.status");
    assert_eq!(
        status.windows,
        [
            LimitWindow { minutes: Some(300), used_percent: 92, resets_at: None },
            LimitWindow { minutes: Some(10080), used_percent: 41, resets_at: None },
        ]
    );
    assert_eq!(status.reached, None);
    assert_eq!(status.observed_at, learned);
    assert_eq!(listed, Some(status));
    drop(client);
    daemon.stop().await.unwrap();
}

/// The 429 of a used-up plan on the WebSocket, a wrapped `error` event with the
/// `x-codex-*` headers, stops the turn after one try with its reset, as on HTTP, and
/// `admin.status` shows the limit and the windows. No try goes out over HTTP.
#[tokio::test]
async fn a_plan_at_its_usage_limit_on_the_websocket_stops_the_turn_after_one_try() {
    let server = ProviderServer::start().await.unwrap();
    let daemon = on_the_websocket(&server).await;
    let client = daemon.client_for_tty(TTY).await.unwrap();
    let resets_at = daemon.clock().shared().now().as_second() + 1200;
    server.set(|knobs| {
        knobs.ws_error_once = Some(serde_json::json!({
            "type": "error",
            "status": 429,
            "error": {
                "type": "usage_limit_reached",
                "message": "You have hit your usage limit.",
                "plan_type": "plus",
                "resets_at": resets_at,
            },
            "headers": {
                "x-codex-primary-used-percent": "100",
                "x-codex-primary-window-minutes": "300",
                "x-codex-primary-reset-at": resets_at.to_string(),
                "x-codex-secondary-used-percent": "41",
                "x-codex-secondary-window-minutes": "10080",
            },
        }));
    });
    server.drain();

    let events = turn(&daemon, &client, 1).await;

    let Event::TurnFailed { account_out, .. } = &events.last().unwrap().event else {
        panic!("{events:#?}")
    };
    let at = jiff::Timestamp::from_second(resets_at).unwrap();
    let reached = LimitReached { kind: LimitKind::Usage, resets_at: Some(at) };
    let out = account_out.as_ref().expect("the cause of the stop");
    assert_eq!((out.provider.as_str(), &out.reached), ("openai-subscription", &reached));
    let saw = seen(&server);
    let creates = saw.iter().filter(|what| **what == Saw::Create).count();
    assert_eq!(creates, 1, "a usage limit is never sent again: {saw:?}");
    assert!(!saw.contains(&Saw::Post), "no try over HTTP: {saw:?}");
    let (status, listed) = limits_of(&client, "openai-subscription").await;
    let status = status.expect("the limit in admin.status");
    assert_eq!(status.reached, Some(reached));
    assert_eq!(
        status.windows,
        [
            LimitWindow { minutes: Some(300), used_percent: 100, resets_at: Some(at) },
            LimitWindow { minutes: Some(10080), used_percent: 41, resets_at: None },
        ]
    );
    assert_eq!(listed, Some(status));
    drop(client);
    daemon.stop().await.unwrap();
}

/// A 429 of the Messages API whose `requests` bucket is empty until `reset`.
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
    .with_header("anthropic-ratelimit-tokens-remaining", "12000")
}

#[tokio::test]
async fn an_api_key_shows_its_rate_limit_only_when_it_was_hit() {
    let messages = MessagesServer::start().await;
    messages.set_models(vec![MessagesServer::model("claude-opus-5-5")]);
    let daemon = TestDaemon::builder().messages(&messages).start().await.unwrap();
    let client = daemon.client_for_tty(TTY).await.unwrap();

    messages.push(MessagesAnswer::text("hi"));
    let events = turn(&daemon, &client, 1).await;
    assert_eq!(events.last().unwrap().event.kind(), "turn_completed", "{events:#?}");
    assert_eq!(limits_of(&client, "anthropic-api").await, (None, None), "a key that serves");

    let reset = (daemon.clock().shared().now() + std::time::Duration::from_secs(180)).to_string();
    messages.push(requests_used_up(&reset));
    let events = turn(&daemon, &client, 2).await;
    let Event::TurnFailed { account_out, .. } = &events.last().unwrap().event else {
        panic!("{events:#?}")
    };
    let reached =
        LimitReached { kind: LimitKind::Requests, resets_at: Some(reset.parse().unwrap()) };
    let out = account_out.as_ref().expect("the cause of the stop");
    assert_eq!((out.provider.as_str(), &out.reached), ("anthropic-api", &reached));
    assert_eq!(out.others, [], "no other account runs the model");
    let (status, listed) = limits_of(&client, "anthropic-api").await;
    assert_eq!(status.as_ref().and_then(|limits| limits.reached.clone()), Some(reached));
    assert_eq!(status.as_ref().map(|limits| limits.windows.len()), Some(0), "a key has none");
    assert_eq!(listed, status);

    messages.push(MessagesAnswer::text("hi again"));
    let events = turn(&daemon, &client, 3).await;
    assert_eq!(events.last().unwrap().event.kind(), "turn_completed", "{events:#?}");
    assert_eq!(limits_of(&client, "anthropic-api").await, (None, None), "the key serves again");
    drop(client);
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn a_429_with_room_in_each_bucket_is_too_many_requests_and_no_limit_of_the_key() {
    let messages = MessagesServer::start().await;
    messages.set_models(vec![MessagesServer::model("claude-opus-5-5")]);
    let daemon = TestDaemon::builder().messages(&messages).start().await.unwrap();
    let client = daemon.client_for_tty(TTY).await.unwrap();

    messages.push(
        MessagesAnswer::error(429, "rate_limit_error", "slow down")
            .with_retry_after(120)
            .with_header("anthropic-ratelimit-requests-remaining", "8"),
    );
    let events = turn(&daemon, &client, 1).await;

    let Event::TurnFailed { account_out, .. } = &events.last().unwrap().event else {
        panic!("{events:#?}")
    };
    let kind = account_out.as_ref().map(|out| out.reached.kind);
    assert_eq!(kind, Some(LimitKind::TooManyRequests));
    assert_eq!(limits_of(&client, "anthropic-api").await, (None, None));
    drop(client);
    daemon.stop().await.unwrap();
}

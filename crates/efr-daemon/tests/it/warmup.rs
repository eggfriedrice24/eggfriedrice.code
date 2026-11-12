//! `provider.warm`: the warm-up of a terminal's next prompt opens the connection that
//! the prompt's request then takes, and it never sends a model request.
//!
//! Each test runs the real providers against [`ProviderServer`] on loopback, which
//! closes each connection after its answer, so a prompt needs a new one.

use std::path::Path;
use std::time::{Duration, Instant};

use efr_protocol::{Method, ProviderWarm, ProviderWarmResult};
use efr_stdx::time::{Clock as _, SystemClock};
use efr_test_daemon::{ProviderServer, Saw, TestDaemon};
use efr_test_support::TestClock;

use crate::perf::{Follower, builder, prompt, run_turn, tree};

const TTY: &str = "/dev/pts/warm";

/// The provider path: the WebSocket of the subscription, the HTTP of an API key, and
/// the Messages API.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Route {
    Ws,
    Http,
    Messages,
}

async fn daemon(
    path: Route,
    server: &ProviderServer,
    clock: &TestClock,
) -> (TestDaemon, std::path::PathBuf) {
    let dirs = tree();
    let cwd = dirs.create_dir("home/project").unwrap();
    let base = builder(&dirs, clock);
    let builder = match path {
        Route::Ws => base.subscription_at(&server.base_url("sub"), &server.issuer()),
        Route::Http => base.responses_at(&server.base_url("openai")).config(|settings| {
            settings.openai.websocket = efr_config::WebSocketChoice::Off;
        }),
        Route::Messages => base.messages_at(&server.base_url("anthropic")),
    };
    (Box::pin(builder.start()).await.unwrap(), cwd)
}

/// Sends `provider.warm` for [`TTY`] in `cwd` and waits until the warm-up ends.
async fn warm(daemon: &TestDaemon, cwd: &Path) -> ProviderWarmResult {
    efr_stdx::mark::drain();
    let client = daemon.client_for_tty(TTY).await.unwrap();
    let params = ProviderWarm {
        tty: Some(TTY.to_owned()),
        pwd: Some(cwd.to_owned()),
        model: None,
        provider: None,
        new_conversation: false,
    };
    let result: ProviderWarmResult = client.call(Method::ProviderWarm(params)).await.unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !efr_stdx::mark::drain().iter().any(|mark| mark.name == "warm.done") {
        assert!(Instant::now() < deadline, "the warm-up did not end");
        SystemClock.sleep(Duration::from_millis(5)).await;
    }
    result
}

fn count(seen: &[efr_test_daemon::Seen], what: Saw) -> usize {
    seen.iter().filter(|seen| seen.what == what).count()
}

/// One prompt from [`TTY`] in `cwd`, to its end.
async fn one(daemon: &TestDaemon, cwd: &Path) {
    let client = daemon.client_for_tty(TTY).await.unwrap();
    let run = run_turn(&client, prompt(TTY, cwd, "how is the build?", None)).await;
    assert_eq!(run.end_kind(), "turn_completed", "{:#?}", run.events);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_warm_up_opens_the_websocket_of_the_conversation_and_runs_no_model() {
    efr_stdx::mark::enable();
    let clock = TestClock::new();
    let _follower = Follower::start(&clock);
    let server = ProviderServer::start().await.unwrap();
    server.set(|knobs| knobs.close = true);
    let (daemon, cwd) = daemon(Route::Ws, &server, &clock).await;
    one(&daemon, &cwd).await;
    SystemClock.sleep(Duration::from_millis(50)).await;
    server.drain();

    let result = warm(&daemon, &cwd).await;
    let warmed = server.drain();
    one(&daemon, &cwd).await;
    let prompted = server.drain();

    assert_eq!(result.provider.as_deref(), Some("openai-subscription"));
    assert_eq!(count(&warmed, Saw::Upgraded), 1, "{warmed:?}");
    assert_eq!(count(&warmed, Saw::Create) + count(&warmed, Saw::Post), 0, "{warmed:?}");
    assert_eq!(count(&prompted, Saw::Upgrade), 0, "{prompted:?}");
    assert_eq!(count(&prompted, Saw::Create), 1, "{prompted:?}");
    daemon.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_warm_up_on_an_http_path_opens_the_pooled_connection_with_the_model_list_only() {
    efr_stdx::mark::enable();
    for path in [Route::Http, Route::Messages] {
        let clock = TestClock::new();
        let _follower = Follower::start(&clock);
        let server = ProviderServer::start().await.unwrap();
        // NOTE: a list that the fetch gets, so the catalog's task asks again only after
        // an hour, and each fetch below is the warm-up's.
        server.set(|knobs| {
            knobs.claude = vec!["claude-opus-5-5".to_owned()];
            knobs.openai_list = true;
        });
        let (daemon, cwd) = daemon(path, &server, &clock).await;
        one(&daemon, &cwd).await;
        server.set(|knobs| knobs.close = true);
        one(&daemon, &cwd).await;
        SystemClock.sleep(Duration::from_millis(50)).await;
        // NOTE: a prompt of the provider within a minute leaves its connection in the
        // pool, so no warm-up fetches then.
        clock.advance(Duration::from_secs(61));
        server.drain();

        warm(&daemon, &cwd).await;
        let warmed = server.drain();
        one(&daemon, &cwd).await;
        let prompted = server.drain();

        assert_eq!(count(&warmed, Saw::Models), 1, "{path:?}: {warmed:?}");
        assert_eq!(count(&warmed, Saw::Post), 0, "{path:?}: {warmed:?}");
        assert_eq!(count(&prompted, Saw::Accepted), 0, "{path:?}: {prompted:?}");
        assert_eq!(count(&prompted, Saw::Post), 1, "{path:?}: {prompted:?}");
        daemon.stop().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_warm_up_right_after_a_prompt_fetches_no_model_list() {
    efr_stdx::mark::enable();
    let clock = TestClock::new();
    let _follower = Follower::start(&clock);
    let server = ProviderServer::start().await.unwrap();
    let (daemon, cwd) = daemon(Route::Http, &server, &clock).await;
    one(&daemon, &cwd).await;
    server.drain();

    warm(&daemon, &cwd).await;
    warm(&daemon, &cwd).await;
    let warmed = server.drain();

    assert!(warmed.is_empty(), "{warmed:?}");
    daemon.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_warm_up_without_a_running_conversation_names_the_default_provider() {
    efr_stdx::mark::enable();
    let clock = TestClock::new();
    let _follower = Follower::start(&clock);
    let server = ProviderServer::start().await.unwrap();
    let (daemon, cwd) = daemon(Route::Ws, &server, &clock).await;
    server.drain();

    let result = warm(&daemon, &cwd).await;
    let warmed = server.drain();

    // No conversation, so no socket: a socket belongs to one conversation.
    assert_eq!(result.provider.as_deref(), Some("openai-subscription"));
    assert_eq!(count(&warmed, Saw::Upgrade), 0, "{warmed:?}");
    assert_eq!(count(&warmed, Saw::Post) + count(&warmed, Saw::Create), 0, "{warmed:?}");
    daemon.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_warm_up_takes_the_provider_of_the_conversations_pick_or_of_the_asked_model() {
    efr_stdx::mark::enable();
    let clock = TestClock::new();
    let _follower = Follower::start(&clock);
    let server = ProviderServer::start().await.unwrap();
    server.set(|knobs| knobs.claude = vec!["claude-opus-5-5".to_owned()]);
    let dirs = tree();
    let cwd = dirs.create_dir("home/project").unwrap();
    let daemon = Box::pin(
        builder(&dirs, &clock)
            .subscription_at(&server.base_url("sub"), &server.issuer())
            .also_messages_at(&server.base_url("anthropic"))
            .start(),
    )
    .await
    .unwrap();
    let client = daemon.client_for_tty(TTY).await.unwrap();
    let mut picked = prompt(TTY, &cwd, "use claude", None);
    picked.settings.model = Some("claude-opus-5-5".to_owned());
    let run = run_turn(&client, picked).await;
    assert_eq!(run.end_kind(), "turn_completed", "{:#?}", run.events);

    let pick = warm(&daemon, &cwd).await;
    let asked = {
        efr_stdx::mark::drain();
        let params = ProviderWarm {
            tty: Some(TTY.to_owned()),
            pwd: Some(cwd.clone()),
            model: Some("gpt-5.5".to_owned()),
            provider: None,
            new_conversation: false,
        };
        let result: ProviderWarmResult = client.call(Method::ProviderWarm(params)).await.unwrap();
        result
    };

    assert_eq!(pick.provider.as_deref(), Some("anthropic-api"));
    assert_eq!(asked.provider.as_deref(), Some("openai-subscription"));
    daemon.stop().await.unwrap();
}

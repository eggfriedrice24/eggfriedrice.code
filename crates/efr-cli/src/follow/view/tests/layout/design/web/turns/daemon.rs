//! A whole turn through the real daemon, for the tests that join the slices of a
//! feature: a test daemon on the real OpenAI or Anthropic provider, against a fake
//! server on loopback that answers each model call with one stream, such as a fixture
//! of efr-providers. The test then plays the turn's real events through a real view or
//! `efr history`.
//!
//! The fake server sends a whole stream at once, so the events of a turn come within
//! milliseconds. [`Timing`] gives them the times of a board: the events, their order
//! and their content stay the daemon's.

use std::collections::HashMap;
use std::path::Path;

use efr_config::{ModelEntry, ModelLimits, WebSearch};
use efr_protocol::{
    CallId, Event, EventEnvelope, Method, Mode, PromptSendResult, SandboxStatus, TurnId,
};
use efr_test_daemon::{
    MessagesAnswer, MessagesServer, ResponsesAnswer, ResponsesServer, TTY, TestDaemon, events_until,
};
use serde_json::{Value, json};

/// The Claude model of a turn on Anthropic.
pub(crate) const CLAUDE: &str = "claude-opus-5-5";

/// The OpenAI model of a turn on OpenAI.
pub(crate) const GPT: &str = "gpt-5.5";

/// The company whose real provider runs the turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Company {
    /// The Responses API with an API key.
    OpenAi,
    /// The Messages API with an API key.
    Anthropic,
}

impl Company {
    /// The text of fixture `name` of efr-providers for this company's API.
    pub(crate) fn fixture(self, name: &str) -> String {
        let dir = match self {
            Company::OpenAi => "openai/responses",
            Company::Anthropic => "anthropic/messages",
        };
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../efr-providers/fixtures")
            .join(dir)
            .join(name);
        std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
    }
}

/// What a turn runs against.
#[derive(Debug, Clone)]
pub(crate) struct Wire {
    pub(crate) company: Company,
    /// The model's context window in tokens, which sets the `ctx N%` of the end line.
    pub(crate) window: u64,
    /// One stream for each model call, in order.
    pub(crate) streams: Vec<String>,
    /// The permission mode of the turn.
    pub(crate) mode: Mode,
    /// `web.search` of the config.
    pub(crate) web: WebSearch,
}

impl Wire {
    /// A turn in auto mode with `web.search = "mode"`, whose model calls get the
    /// fixtures `names` of `company`.
    pub(crate) fn fixtures(company: Company, window: u64, names: &[&str]) -> Wire {
        Wire {
            company,
            window,
            streams: names.iter().map(|name| company.fixture(name)).collect(),
            mode: Mode::Auto,
            web: WebSearch::Mode,
        }
    }

    pub(crate) fn mode(mut self, mode: Mode) -> Wire {
        self.mode = mode;
        self
    }

    pub(crate) fn web(mut self, web: WebSearch) -> Wire {
        self.web = web;
        self
    }
}

/// A turn that ran.
#[derive(Debug)]
pub(crate) struct Ran {
    pub(crate) turn: TurnId,
    /// The events of the turn, in the order of the log.
    pub(crate) events: Vec<EventEnvelope>,
    /// The body of each model call that the fake server got.
    pub(crate) bodies: Vec<Value>,
}

/// Runs one prompt on a test daemon against `wire` and follows its turn to the end.
pub(crate) async fn run(wire: Wire) -> Ran {
    let (daemon, bodies): (TestDaemon, Box<dyn Fn() -> Vec<Value>>) = match wire.company {
        Company::Anthropic => {
            let server = MessagesServer::start().await;
            let mut model = MessagesServer::model(CLAUDE);
            model["max_input_tokens"] = json!(wire.window);
            server.set_models(vec![model]);
            for stream in &wire.streams {
                server.push(MessagesAnswer::from_sse(stream));
            }
            let web = wire.web;
            let daemon = TestDaemon::builder()
                .messages(&server)
                .probe_override(ready())
                .config(move |config| {
                    config.web.search = web;
                    config.permissions.allow_full = true;
                })
                .start()
                .await
                .unwrap();
            let bodies =
                move || server.received().into_iter().map(|request| request.body).collect();
            (daemon, Box::new(bodies))
        }
        Company::OpenAi => {
            let server = ResponsesServer::start().await;
            for stream in &wire.streams {
                server.push(ResponsesAnswer::from_sse(stream));
            }
            let (web, window) = (wire.web, wire.window);
            let daemon = TestDaemon::builder()
                .responses(&server)
                .probe_override(ready())
                .config(move |config| {
                    config.web.search = web;
                    config.permissions.allow_full = true;
                    let mut limits = ModelLimits::new(GPT);
                    limits.context_window = Some(window);
                    config.openai.models = Some(vec![ModelEntry::Model(limits)]);
                })
                .start()
                .await
                .unwrap();
            let bodies =
                move || server.received().into_iter().map(|request| request.body).collect();
            (daemon, Box::new(bodies))
        }
    };
    let Method::PromptSend(mut send) = daemon.prompt(1, "does ROB RUN work on Linux by now?", TTY)
    else {
        unreachable!("a prompt is a prompt.send")
    };
    send.settings.mode = Some(wire.mode);
    send.settings.model = Some(
        match wire.company {
            Company::Anthropic => CLAUDE,
            Company::OpenAi => GPT,
        }
        .to_owned(),
    );
    let client = daemon.client_for_tty(TTY).await.unwrap();
    let sent: PromptSendResult = client.call(Method::PromptSend(send)).await.unwrap();
    let turn = sent.turn_id;
    let mut follow = daemon.follow(&client, sent.conversation_id).await.unwrap();
    let events = events_until(&mut follow, |event| {
        matches!(
            event,
            Event::TurnCompleted { turn_id, .. }
                | Event::TurnFailed { turn_id, .. }
                | Event::TurnInterrupted { turn_id, .. }
                if *turn_id == turn
        )
    })
    .await
    .unwrap();
    let events: Vec<EventEnvelope> =
        events.into_iter().filter(|envelope| envelope.event.turn_id() == Some(turn)).collect();
    // NOTE: every turn of these tests completes; a stream that ends early fails it.
    assert_eq!(
        events.last().map(|envelope| envelope.event.kind()),
        Some("turn_completed"),
        "{events:?}"
    );
    let bodies = bodies();
    daemon.stop().await.unwrap();
    Ran { turn, events, bodies }
}

/// A sandbox that is ready, so `auto` runs as `auto`.
fn ready() -> SandboxStatus {
    serde_json::from_value(json!({
        "available": true,
        "landlock_abi": 10,
        "errata": 15,
        "bwrap": "/usr/bin/bwrap",
        "bwrap_version": "0.13.0",
        "cache_mode": "tmp",
        "network_mode": "none",
    }))
    .unwrap()
}

/// The times of a board, in milliseconds after the prompt.
#[derive(Debug, Clone)]
pub(crate) struct Timing {
    /// The start and the length of each web step, in the order of the starts.
    pub(crate) steps: Vec<(i64, i64)>,
    /// When the text after the last step comes.
    pub(crate) answer: i64,
    /// When the turn ends: its report and its terminal event.
    pub(crate) end: i64,
}

impl Timing {
    /// The events of `ran` with the time of each: a step at its start and at its end,
    /// the text after the last step at `answer`, the end of the turn at `end`, and any
    /// other event at the time of the event before it.
    pub(crate) fn of(&self, ran: &Ran) -> Vec<(i64, Event)> {
        let mut started: HashMap<CallId, usize> = HashMap::new();
        let mut ended = 0;
        let mut now = 0;
        let mut timed = Vec::new();
        for envelope in &ran.events {
            let event = &envelope.event;
            now = match event {
                Event::WebCallStarted { call_id, .. } => {
                    let n = started.len();
                    started.insert(*call_id, n);
                    self.steps[n].0
                }
                Event::WebCallCompleted { call_id, .. } => {
                    ended += 1;
                    let n = started.get(call_id).copied().unwrap_or_else(|| {
                        panic!("the end of a step that did not start: {event:?}")
                    });
                    self.steps[n].0 + self.steps[n].1
                }
                Event::AssistantMessageUpdated { .. } | Event::AssistantMessageCompleted { .. }
                    if ended == self.steps.len() && !started.is_empty() =>
                {
                    now.max(self.answer)
                }
                Event::TurnWebReport { .. }
                | Event::TurnCompleted { .. }
                | Event::TurnFailed { .. }
                | Event::TurnInterrupted { .. } => self.end,
                _ => now,
            };
            timed.push((now, event.clone()));
        }
        assert_eq!(started.len(), self.steps.len(), "a time for each step of {:?}", ran.events);
        timed
    }
}

//! The scenario driver: an NDJSON transcript replayed through a [`TestDaemon`].
//!
//! A scenario is a fixture in `crates/efr-test-daemon/fixtures/<name>.ndjson` plus a
//! line in [`SCENARIOS`] saying how its daemon runs. [`Replay`] starts the daemon with
//! a paced [`ReplayProvider`] over the transcript (or a [`ResponsesServer`] that answers
//! from it), connects as the shell plugin of [`TTY`], and walks the records in order:
//!
//! - `client_frame`: sends the frame's method and keeps its result by the frame's
//!   `id`. The first `prompt.send` that names a conversation starts a subscription to
//!   it, from the beginning of its log. A `{"cancel": id}` frame cancels the stream the
//!   frame `id` started.
//! - `event`: the next event of that subscription must equal the body. Events of the
//!   kinds `assistant_message_updated` and `tool_call_output_updated` depend on how
//!   bytes are chunked, so they are skipped unless the record expects that kind. A
//!   record is a barrier: the driver goes on only once the event is in the log.
//! - `pty_bytes`: `emit_inbound` prints the bytes on the fake PTY named `pty` (the
//!   n-th name is the n-th PTY the daemon spawns); `expect_outbound` reads what the
//!   session typed, which must be exactly the bytes.
//! - `clock_advance`: moves the daemon's [`TestClock`](efr_test_support::TestClock).
//! - `provider_request`, `provider_sse`: the provider handles these. After every other
//!   record the driver tells the paced provider it got so far, so an answer never runs
//!   ahead of the records before it.
//!
//! Ids the daemon mints are written as placeholders ([`Bindings`]); paths and
//! timestamps as the [`Redactor`] placeholders of the daemon (`<TMP>`, `<CWD>`,
//! `<HOME>`, `<TIMESTAMP>`). [`Replay::verify`] checks at the end that the provider got
//! every request of the transcript and nothing else.
//!
//! In the Responses mode the server queues every exchange's answer at the start: the
//! `provider_sse` records of an exchange, joined, are the body of a 200 answer, unless
//! the first line is the comment `: status <code>`, which sets the status. The requests
//! are compared with the `provider_request` records at the end. A subscription
//! scenario runs the `openai-subscription` provider with a stored login, and the
//! server's token endpoint answers one refresh grant.
//!
//! [`Replay::bless`] rewrites a fixture's outbound records (`provider_request`,
//! `event`, typed `pty_bytes`) with what the daemon actually did, keeping the inbound
//! ones, so a fixture is written as a skeleton and then reviewed as a diff.

mod bindings;

use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use efr_client::{Client, ClientError, ItemStream};
use efr_protocol::{
    ClientFrame, ConversationId, ConversationSubscribe, ConversationSubscribeItem, ErrorBody,
    EventEnvelope, Method, PromptSendResult, Seq,
};
use efr_test_support::{
    Inbound, Outbound, Record, Redactor, ReplayProvider, TestDirs, TestSupportError, Transcript,
};
use futures::StreamExt as _;
use serde_json::Value;

pub use self::bindings::Bindings;
use crate::pty_script::bytes_json;
use crate::test_daemon::{ResponsesAnswer, ResponsesServer, TTY, working_dir};
use crate::{FakeTerminal, TestDaemon, TestDaemonError};

/// Event kinds whose number depends on how a stream is chunked.
const INCIDENTAL: &[&str] = &["assistant_message_updated", "tool_call_output_updated"];

/// The access token the token endpoint of a subscription scenario hands out for the
/// refresh grant.
pub const REFRESHED_ACCESS_TOKEN: &str = "efr-test-access-2";

/// The refresh token it rotates to.
pub const REFRESHED_REFRESH_TOKEN: &str = "efr-test-refresh-2";

/// How often a replay yields while it waits for the provider to get a request,
/// before it gives up.
const MAX_REQUEST_POLLS: usize = 10_000_000;

/// How many runs a bless may take: each provider request that changed costs one.
const MAX_BLESS_RUNS: usize = 16;

/// How the daemon of a scenario runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct ScenarioSpec {
    /// The fixture's name without `.ndjson`.
    pub name: &'static str,
    /// The real OpenAI provider against a [`ResponsesServer`] instead of a replay
    /// provider.
    pub responses: bool,
    /// With `responses`, the `openai-subscription` provider with a stored login instead
    /// of `openai-api`; the server's token endpoint answers one refresh grant with
    /// [`REFRESHED_ACCESS_TOKEN`] and [`REFRESHED_REFRESH_TOKEN`].
    pub subscription: bool,
    /// A file store, so the database survives a restart.
    pub persistent: bool,
    /// Replaces the daemon's update interval of streamed text, in milliseconds.
    pub update_interval_ms: Option<u64>,
    /// Restarts the daemon once this many event records have matched.
    pub restart_after_event: Option<usize>,
    /// A registered project: its id and its root relative to the working directory.
    pub project: Option<(&'static str, &'static str)>,
}

impl ScenarioSpec {
    const fn new(name: &'static str) -> Self {
        ScenarioSpec {
            name,
            responses: false,
            subscription: false,
            persistent: false,
            update_interval_ms: None,
            restart_after_event: None,
            project: None,
        }
    }

    const fn subscription(mut self) -> Self {
        self.responses = true;
        self.subscription = true;
        self
    }

    const fn persistent(mut self) -> Self {
        self.persistent = true;
        self
    }

    const fn update_interval_ms(mut self, ms: u64) -> Self {
        self.update_interval_ms = Some(ms);
        self
    }

    const fn restart_after_event(mut self, events: usize) -> Self {
        self.restart_after_event = Some(events);
        self
    }

    const fn project(mut self, id: &'static str, root: &'static str) -> Self {
        self.project = Some((id, root));
        self
    }
}

/// The milestone 1 scenarios (structure document, section 7), one fixture each.
pub const SCENARIOS: &[ScenarioSpec] = &[
    ScenarioSpec::new("single_turn_text"),
    ScenarioSpec::new("tool_call_shell_ok"),
    ScenarioSpec::new("tool_call_shell_nonzero_exit"),
    ScenarioSpec::new("approval_ask_then_allow"),
    ScenarioSpec::new("approval_deny"),
    ScenarioSpec::new("cwd_move_between_turns")
        .project("0192f0c1-7a00-7000-8000-00000000be7a", "beta"),
    ScenarioSpec::new("interrupt_mid_stream"),
    ScenarioSpec::new("queue_second_prompt"),
    ScenarioSpec::new("subscribe_resume_after_seq"),
    // Every text delta is its own event, so one answer makes the log longer than a
    // resume may replay.
    ScenarioSpec::new("subscribe_gap_too_large_snapshot").update_interval_ms(0),
    ScenarioSpec::new("duplicate_command_id_receipt"),
    // The subscription's token source refreshes its login once on a 401.
    ScenarioSpec::new("provider_401_refresh_once").subscription(),
    // After the second prompt queued behind the running turn.
    ScenarioSpec::new("restart_reconcile_inflight_turn").persistent().restart_after_event(4),
    ScenarioSpec::new("pty_attach_since_seq"),
];

/// A scenario: its spec and its transcript.
#[derive(Debug, Clone)]
pub struct Scenario {
    spec: ScenarioSpec,
    path: PathBuf,
    transcript: Transcript,
}

impl Scenario {
    /// The spec of the scenario `name`.
    pub fn spec_of(name: &str) -> Option<ScenarioSpec> {
        SCENARIOS.iter().find(|spec| spec.name == name).copied()
    }

    /// The fixture file of the scenario `name`, which need not exist yet.
    pub fn path_of(name: &str) -> Result<PathBuf, TestDaemonError> {
        Ok(efr_test_support::fixtures::path(file!(), format!("{name}.ndjson"))?)
    }

    /// Reads and validates the scenario `name`.
    pub fn load(name: &str) -> Result<Scenario, TestDaemonError> {
        let spec = Scenario::spec_of(name)
            .ok_or_else(|| TestDaemonError::UnknownScenario { name: name.to_owned() })?;
        let path = Scenario::path_of(name)?;
        let transcript = Transcript::read(&path)?;
        Ok(Scenario { spec, path, transcript })
    }

    /// How its daemon runs.
    pub fn spec(&self) -> ScenarioSpec {
        self.spec
    }

    /// Its fixture file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Its records.
    pub fn transcript(&self) -> &Transcript {
        &self.transcript
    }

    /// The lines of the records of `kind`, such as `client_frame`, in order.
    pub fn lines(&self, kind: &str) -> Vec<usize> {
        self.transcript
            .entries()
            .iter()
            .filter(|entry| entry.record.kind() == kind)
            .map(|entry| entry.line)
            .collect()
    }

    /// The lines of the `event` records whose event is of `event_kind`, in order.
    pub fn event_lines(&self, event_kind: &str) -> Vec<usize> {
        self.transcript
            .entries()
            .iter()
            .filter(|entry| {
                matches!(&entry.record, Record::ExpectOutbound(Outbound::Event(body))
                    if body.get("kind").and_then(Value::as_str) == Some(event_kind))
            })
            .map(|entry| entry.line)
            .collect()
    }

    /// The same scenario with the records at the lines of `rewrites` replaced.
    fn rewritten(&self, rewrites: &BTreeMap<usize, Record>) -> Scenario {
        let records = self.transcript.entries().iter().map(|entry| {
            rewrites.get(&entry.line).cloned().unwrap_or_else(|| entry.record.clone())
        });
        Scenario {
            spec: self.spec,
            path: self.path.clone(),
            transcript: Transcript::from_records(records),
        }
    }
}

/// A scenario being replayed through a [`TestDaemon`].
#[derive(Debug)]
pub struct Replay {
    scenario: Scenario,
    daemon: TestDaemon,
    client: Client,
    provider: Option<Arc<ReplayProvider>>,
    server: Option<ResponsesServer>,
    redactor: Redactor,
    bindings: Bindings,
    conversation: Option<ConversationId>,
    subscription: Option<ItemStream<ConversationSubscribeItem>>,
    /// Events received and not matched yet (from a snapshot).
    pending: VecDeque<EventEnvelope>,
    /// The sequence number of the last event handed to a record.
    last_seq: Seq,
    results: BTreeMap<u64, Result<Value, ErrorBody>>,
    streams: BTreeMap<u64, ItemStream<Value>>,
    terminals: BTreeMap<String, FakeTerminal>,
    /// The PTY names of the transcript, in the order they appeared since the daemon
    /// (re)started: the n-th is the daemon's n-th spawn after `pty_base`.
    pty_names: Vec<String>,
    /// The spawns of the fake holder before the daemon last started.
    pty_base: usize,
    /// The index of the next record.
    next: usize,
    /// The `provider_request` records handled.
    requests: usize,
    /// The `event` records matched.
    events: usize,
    /// In a bless, the outbound records as the daemon actually sent them.
    rewrites: Option<BTreeMap<usize, Record>>,
}

impl Replay {
    /// Loads the scenario `name`, replays every record and verifies the provider.
    pub async fn run(name: &str) -> Result<Replay, TestDaemonError> {
        let mut replay = Replay::start(Scenario::load(name)?).await?;
        replay.run_to_end().await?;
        Ok(replay)
    }

    /// Starts the scenario's daemon and connects; no record is handled yet.
    pub async fn start(scenario: Scenario) -> Result<Replay, TestDaemonError> {
        Replay::launch(scenario, false).await
    }

    async fn launch(scenario: Scenario, bless: bool) -> Result<Replay, TestDaemonError> {
        let spec = scenario.spec;
        let dirs = Arc::new(TestDirs::new()?);
        let (cwd, redactor) = working_dir(&dirs)?;
        if let Some((id, root)) = spec.project {
            register_project(&dirs, id, &cwd.join(root))?;
        }
        let mut builder = TestDaemon::builder().dirs(Arc::clone(&dirs));
        if let Some(ms) = spec.update_interval_ms {
            builder = builder.config(|config| config.conversation.update_interval_ms = ms);
        }
        if spec.persistent {
            builder = builder.persistent();
        }
        let (provider, server) = if spec.responses {
            let server = ResponsesServer::start().await;
            for (_, _, answer) in exchanges(&scenario.transcript)? {
                server.push(ResponsesAnswer::from_sse(&answer));
            }
            builder = if spec.subscription {
                let refreshed = serde_json::json!({
                    "access_token": REFRESHED_ACCESS_TOKEN,
                    "refresh_token": REFRESHED_REFRESH_TOKEN,
                    "expires_in": 3600,
                });
                server.push_token(ResponsesAnswer::new(200, refreshed.to_string()));
                builder.subscription(&server)
            } else {
                builder.responses(&server)
            };
            (None, Some(server))
        } else {
            let provider =
                ReplayProvider::new(&scenario.transcript)?.with_redactor(redactor.clone()).paced();
            let provider = Arc::new(provider);
            builder = builder.provider(Arc::clone(&provider));
            (Some(provider), None)
        };
        let daemon = builder.start().await?;
        let client = daemon.client_for_tty(TTY).await?;
        Ok(Replay {
            scenario,
            daemon,
            client,
            provider,
            server,
            redactor,
            bindings: Bindings::new(),
            conversation: None,
            subscription: None,
            pending: VecDeque::new(),
            last_seq: Seq::ZERO,
            results: BTreeMap::new(),
            streams: BTreeMap::new(),
            terminals: BTreeMap::new(),
            pty_names: Vec::new(),
            pty_base: 0,
            next: 0,
            requests: 0,
            events: 0,
            rewrites: bless.then(BTreeMap::new),
        })
    }

    /// The scenario.
    pub fn scenario(&self) -> &Scenario {
        &self.scenario
    }

    /// The daemon.
    pub fn daemon(&self) -> &TestDaemon {
        &self.daemon
    }

    /// The client the records are sent with, the shell plugin of [`TTY`].
    pub fn client(&self) -> &Client {
        &self.client
    }

    /// The replay provider, unless the scenario runs against a Responses server.
    pub fn provider(&self) -> Option<&Arc<ReplayProvider>> {
        self.provider.as_ref()
    }

    /// The Responses server of a scenario in the Responses mode.
    pub fn server(&self) -> Option<&ResponsesServer> {
        self.server.as_ref()
    }

    /// The ids bound so far.
    pub fn bindings(&self) -> &Bindings {
        &self.bindings
    }

    /// The conversation the replay follows, once a `prompt.send` named one.
    pub fn conversation(&self) -> Option<ConversationId> {
        self.conversation
    }

    /// The answer to the client frame with `id`: its result, or the daemon's refusal.
    pub fn result(&self, id: u64) -> Option<&Result<Value, ErrorBody>> {
        self.results.get(&id)
    }

    /// The items of the stream that the client frame with `id` started.
    pub fn stream(&mut self, id: u64) -> Option<&mut ItemStream<Value>> {
        self.streams.get_mut(&id)
    }

    /// The fake PTY named `name` in the transcript, once the daemon has spawned it.
    pub async fn terminal(&mut self, name: &str) -> Result<&mut FakeTerminal, TestDaemonError> {
        if !self.terminals.contains_key(name) {
            let position = match self.pty_names.iter().position(|known| known == name) {
                Some(position) => position,
                None => {
                    self.pty_names.push(name.to_owned());
                    self.pty_names.len() - 1
                }
            };
            let holder = self.daemon.holder().ok_or(TestDaemonError::NoFakeHolder)?;
            let terminal = holder.terminal(self.pty_base + position).await?;
            self.terminals.insert(name.to_owned(), terminal);
        }
        match self.terminals.get_mut(name) {
            Some(terminal) => Ok(terminal),
            None => unreachable!("the terminal of {name} was inserted above"),
        }
    }

    /// Every event of the followed conversation in the log, oldest first.
    pub async fn events(&self) -> Result<Vec<EventEnvelope>, TestDaemonError> {
        match self.conversation {
            Some(conversation) => self.daemon.events(&self.client, conversation).await,
            None => Ok(Vec::new()),
        }
    }

    /// The line of the next record, or `None` when every record is handled.
    pub fn next_line(&self) -> Option<usize> {
        self.scenario.transcript.entries().get(self.next).map(|entry| entry.line)
    }

    /// Handles the next record and returns its line, or `None` when none is left.
    pub async fn step(&mut self) -> Result<Option<usize>, TestDaemonError> {
        let Some(entry) = self.scenario.transcript.entries().get(self.next).cloned() else {
            return Ok(None);
        };
        self.next += 1;
        let line = entry.line;
        match &entry.record {
            Record::ExpectOutbound(Outbound::ProviderRequest(_)) => {
                self.requests += 1;
                return Ok(Some(line));
            }
            Record::EmitInbound(Inbound::ProviderSse(_)) => return Ok(Some(line)),
            Record::EmitInbound(Inbound::ClientFrame(body)) => self.send_frame(line, body).await?,
            Record::ExpectOutbound(Outbound::Event(expected)) => {
                self.expect_event(line, expected).await?;
                self.events += 1;
                if self.scenario.spec.restart_after_event == Some(self.events) {
                    self.restart(line).await?;
                }
            }
            Record::EmitInbound(Inbound::PtyBytes { pty, bytes }) => {
                let bytes = restore_bytes(&self.redactor, bytes);
                self.terminal(pty).await?.print(&bytes).await?;
            }
            Record::ExpectOutbound(Outbound::PtyBytes { pty, bytes }) => {
                self.expect_typed(line, pty, bytes).await?;
            }
            Record::ClockAdvance(by) => self.daemon.clock().advance(*by),
            _ => {
                return Err(TestDaemonError::InvalidRecord {
                    line,
                    problem: "the replay does not know this kind of record",
                });
            }
        }
        if let Some(provider) = &self.provider {
            provider.handled_through(line);
        }
        Ok(Some(line))
    }

    /// Handles every record up to and including `line`.
    pub async fn run_to(&mut self, line: usize) -> Result<(), TestDaemonError> {
        while self.next_line().is_some_and(|next| next <= line) {
            self.step().await?;
        }
        Ok(())
    }

    /// Handles every record left, then [`verify`](Self::verify)s.
    pub async fn run_to_end(&mut self) -> Result<(), TestDaemonError> {
        while self.step().await?.is_some() {}
        self.verify()
    }

    /// Checks that the provider got every request of the transcript, each as its
    /// record says, and no other.
    pub fn verify(&self) -> Result<(), TestDaemonError> {
        if let Some(provider) = &self.provider {
            provider.finish()?;
        }
        if let Some(server) = &self.server {
            let expected = exchanges(&self.scenario.transcript)?;
            let received = server.received();
            if received.len() != expected.len() {
                return Err(TestDaemonError::ResponsesCount {
                    received: received.len(),
                    expected: expected.len(),
                });
            }
            for (index, (request, (line, body, _))) in received.iter().zip(expected).enumerate() {
                let actual = self.redactor.redact_json(&request.body);
                let expected = self.redactor.redact_json(&body);
                if actual != expected {
                    return Err(TestDaemonError::ResponsesMismatch {
                        index,
                        line,
                        expected,
                        actual,
                    });
                }
            }
        }
        Ok(())
    }

    /// Stops the daemon.
    pub async fn stop(self) -> Result<(), TestDaemonError> {
        let Replay { daemon, client, subscription, streams, terminals, .. } = self;
        drop((client, subscription, streams, terminals));
        daemon.stop().await
    }

    /// Rewrites the fixture of the scenario `name` with what the daemon actually sends
    /// for its outbound records, and returns its path. Inbound records, the number and
    /// order of records and their kinds stay as written.
    pub async fn bless(name: &str) -> Result<PathBuf, TestDaemonError> {
        let mut scenario = Scenario::load(name)?;
        for _ in 0..MAX_BLESS_RUNS {
            let mut replay = Replay::launch(scenario.clone(), true).await?;
            let mut outcome = Ok(());
            while outcome.is_ok() {
                match replay.step().await {
                    Ok(Some(_)) => {}
                    Ok(None) => break,
                    Err(error) => outcome = Err(error),
                }
            }
            if let Some((line, actual)) = replay.provider_mismatch() {
                let fixed = BTreeMap::from([(
                    line,
                    Record::ExpectOutbound(Outbound::ProviderRequest(actual)),
                )]);
                scenario = scenario.rewritten(&fixed);
                replay.stop().await?;
                continue;
            }
            outcome?;
            let mut rewrites = replay.rewrites.take().unwrap_or_default();
            if let Some(server) = &replay.server {
                let received = server.received();
                for ((line, _, _), request) in
                    exchanges(&scenario.transcript)?.into_iter().zip(received)
                {
                    let body = replay.redactor.redact_json(&request.body);
                    rewrites.insert(line, Record::ExpectOutbound(Outbound::ProviderRequest(body)));
                }
            }
            replay.stop().await?;
            let blessed = scenario.rewritten(&rewrites);
            let path = blessed.path.clone();
            std::fs::write(&path, blessed.transcript.to_ndjson())
                .map_err(|source| TestDaemonError::Write { path: path.clone(), source })?;
            return Ok(path);
        }
        Err(TestDaemonError::BlessUnsettled { name: name.to_owned(), runs: MAX_BLESS_RUNS })
    }

    /// The first request the replay provider refused, with the line it was compared
    /// with and the request as it came, redacted.
    fn provider_mismatch(&self) -> Option<(usize, Value)> {
        match self.provider.as_ref()?.finish() {
            Err(TestSupportError::RequestMismatch { line, actual, .. }) => Some((line, actual)),
            _ => None,
        }
    }

    async fn send_frame(&mut self, line: usize, body: &Value) -> Result<(), TestDaemonError> {
        let body = self
            .bindings
            .substitute(body)
            .map_err(|placeholder| TestDaemonError::UnboundPlaceholder { line, placeholder })?;
        let body = self.redactor.restore_json(&body);
        let bytes = serde_json::to_vec(&body)
            .map_err(|source| TestDaemonError::Json { what: "encoding a client frame", source })?;
        let frame = ClientFrame::from_json(&bytes)
            .map_err(|source| TestDaemonError::InvalidFrame { line, source })?;
        match frame {
            ClientFrame::Request { id, method } => {
                if let Method::PromptSend(params) = &method
                    && let Some(context) = &params.context
                    && let Ok(relative) = context.pwd.strip_prefix(self.daemon.dirs().root())
                {
                    self.daemon.dirs().create_dir(relative)?;
                }
                let is_prompt = matches!(method, Method::PromptSend(_));
                if method.is_stream() {
                    let stream = self.client.stream::<Value>(method).await?;
                    self.streams.insert(id.get(), stream);
                    return Ok(());
                }
                let result = match self.client.call::<Value>(method).await {
                    Ok(value) => Ok(value),
                    Err(ClientError::Server { body }) => Err(body),
                    Err(error) => return Err(error.into()),
                };
                if let Ok(value) = &result {
                    self.bindings.normalize(value);
                    if is_prompt && self.conversation.is_none() {
                        let sent: PromptSendResult = serde_json::from_value(value.clone())
                            .map_err(|source| TestDaemonError::Json {
                                what: "reading a prompt.send result",
                                source,
                            })?;
                        self.conversation = Some(sent.conversation_id);
                        self.subscribe().await?;
                    }
                }
                self.results.insert(id.get(), result);
                Ok(())
            }
            ClientFrame::Cancel { id } => {
                if let Some(stream) = self.streams.get(&id.get()) {
                    stream.cancel().await?;
                }
                Ok(())
            }
            _ => Err(TestDaemonError::InvalidRecord { line, problem: "an unknown client frame" }),
        }
    }

    async fn subscribe(&mut self) -> Result<(), TestDaemonError> {
        let Some(conversation_id) = self.conversation else {
            return Ok(());
        };
        let params = ConversationSubscribe { conversation_id, after_seq: Some(self.last_seq) };
        self.pending.clear();
        self.subscription = Some(self.client.stream(Method::ConversationSubscribe(params)).await?);
        Ok(())
    }

    async fn expect_event(&mut self, line: usize, expected: &Value) -> Result<(), TestDaemonError> {
        let expected_kind = expected.get("kind").and_then(Value::as_str).unwrap_or_default();
        loop {
            let envelope = self.next_event(line).await?;
            let kind = envelope.event.kind().to_owned();
            let event = serde_json::to_value(&envelope.event)
                .map_err(|source| TestDaemonError::Json { what: "encoding an event", source })?;
            let actual = self.redactor.redact_json(&self.bindings.normalize(&event));
            if INCIDENTAL.contains(&kind.as_str()) && kind != expected_kind {
                continue;
            }
            if let Some(rewrites) = &mut self.rewrites {
                if let Some(provider) = &self.provider {
                    // A refused request makes every later event wrong; the bless fixes the
                    // request first and runs again.
                    if let Err(error @ TestSupportError::RequestMismatch { .. }) = provider.finish()
                    {
                        return Err(error.into());
                    }
                }
                rewrites.insert(line, Record::ExpectOutbound(Outbound::Event(actual)));
                return Ok(());
            }
            if &actual == expected {
                return Ok(());
            }
            return Err(TestDaemonError::Mismatch {
                line,
                kind: "event",
                expected: expected.clone(),
                actual,
            });
        }
    }

    async fn next_event(&mut self, line: usize) -> Result<EventEnvelope, TestDaemonError> {
        loop {
            if let Some(envelope) = self.pending.pop_front() {
                self.last_seq = envelope.seq;
                return Ok(envelope);
            }
            let Some(subscription) = self.subscription.as_mut() else {
                return Err(TestDaemonError::InvalidRecord {
                    line,
                    problem: "an event record comes before any prompt.send named a conversation",
                });
            };
            match subscription.next().await {
                Some(Ok(ConversationSubscribeItem::Event(envelope))) => {
                    self.pending.push_back(envelope);
                }
                Some(Ok(ConversationSubscribeItem::Snapshot(snapshot))) => {
                    self.pending.extend(snapshot.events);
                }
                Some(Ok(_)) => {}
                Some(Err(error)) => return Err(error.into()),
                None => return Err(TestDaemonError::SubscriptionEnded { line }),
            }
        }
    }

    async fn expect_typed(
        &mut self,
        line: usize,
        pty: &str,
        expected: &[u8],
    ) -> Result<(), TestDaemonError> {
        let expected = restore_bytes(&self.redactor, expected);
        let blessing = self.rewrites.is_some();
        let terminal = self.terminal(pty).await?;
        let typed = if blessing {
            terminal.typed_line().await?
        } else {
            terminal.typed_against(&expected).await?
        };
        if let Some(rewrites) = &mut self.rewrites {
            let bytes = redact_bytes(&self.redactor, &typed);
            rewrites.insert(
                line,
                Record::ExpectOutbound(Outbound::PtyBytes { pty: pty.to_owned(), bytes }),
            );
            return Ok(());
        }
        if typed == expected {
            return Ok(());
        }
        Err(TestDaemonError::Mismatch {
            line,
            kind: "pty_bytes",
            expected: bytes_json(&redact_bytes(&self.redactor, &expected)),
            actual: bytes_json(&redact_bytes(&self.redactor, &typed)),
        })
    }

    /// Restarts the daemon on the same tree, then follows the conversation again from
    /// the last event a record saw.
    async fn restart(&mut self, line: usize) -> Result<(), TestDaemonError> {
        // NOTE: the turn must have reached the provider first, or whether its exchange
        // was used would depend on timing.
        self.wait_for_requests(line).await?;
        self.subscription = None;
        self.streams.clear();
        self.terminals.clear();
        self.pty_names.clear();
        self.daemon.restart().await?;
        self.pty_base = self.daemon.holder().map_or(0, |holder| holder.spawned());
        self.client = self.daemon.client_for_tty(TTY).await?;
        self.subscribe().await
    }

    async fn wait_for_requests(&self, line: usize) -> Result<(), TestDaemonError> {
        let Some(provider) = &self.provider else {
            return Ok(());
        };
        for _ in 0..MAX_REQUEST_POLLS {
            if provider.served() >= self.requests {
                return Ok(());
            }
            if let Err(error @ TestSupportError::RequestMismatch { .. }) = provider.finish() {
                return Err(error.into());
            }
            tokio::task::yield_now().await;
        }
        Err(TestDaemonError::RequestNeverCame { line })
    }
}

/// The exchanges of a Responses mode transcript: the line and body of each
/// `provider_request` record and the joined `provider_sse` bodies after it.
fn exchanges(transcript: &Transcript) -> Result<Vec<(usize, Value, String)>, TestDaemonError> {
    let mut exchanges: Vec<(usize, Value, String)> = Vec::new();
    for entry in transcript.entries() {
        match &entry.record {
            Record::ExpectOutbound(Outbound::ProviderRequest(body)) => {
                exchanges.push((entry.line, body.clone(), String::new()));
            }
            Record::EmitInbound(Inbound::ProviderSse(text)) => match exchanges.last_mut() {
                Some((_, _, answer)) => answer.push_str(text),
                None => {
                    return Err(TestDaemonError::InvalidRecord {
                        line: entry.line,
                        problem: "a provider_sse record must follow a provider_request record",
                    });
                }
            },
            _ => {}
        }
    }
    Ok(exchanges)
}

/// Registers the project `id` at `root` in the daemon's `projects.toml`.
fn register_project(dirs: &TestDirs, id: &str, root: &Path) -> Result<(), TestDaemonError> {
    std::fs::create_dir_all(root)
        .map_err(|source| TestDaemonError::Write { path: root.to_path_buf(), source })?;
    let path = dirs.dirs().config().join("projects.toml");
    let root = Value::from(root.to_string_lossy().into_owned());
    let text = format!("[[project]]\nid = \"{id}\"\nroot = {root}\n");
    std::fs::write(&path, text).map_err(|source| TestDaemonError::Write { path, source })
}

/// `bytes` with the redactor's placeholders put back, when they are text.
fn restore_bytes(redactor: &Redactor, bytes: &[u8]) -> Vec<u8> {
    match std::str::from_utf8(bytes) {
        Ok(text) => redactor.restore(text).into_bytes(),
        Err(_) => bytes.to_vec(),
    }
}

/// `bytes` with the redactor's values replaced by placeholders, when they are text.
fn redact_bytes(redactor: &Redactor, bytes: &[u8]) -> Vec<u8> {
    match std::str::from_utf8(bytes) {
        Ok(text) => redactor.redact(text).into_bytes(),
        Err(_) => bytes.to_vec(),
    }
}

#[cfg(test)]
mod tests;

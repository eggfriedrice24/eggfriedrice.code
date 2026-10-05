//! Fakes and a harness for the tests of this crate.
//!
//! The harness runs a real actor against the real store (in memory), a [`TestClock`],
//! a seeded generator, a [`ReplayProvider`] that checks every request, the
//! [`FakeToolbox`] and the [`FakeScope`]. A test builds its expected requests with the
//! same types the turn uses, so a request that differs in any message fails the replay.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use efr_permissions::{Engine, Locations, Requirements};
use efr_protocol::{
    ApprovalDecision, ApprovalRespond, CallId, CommandId, ConversationId, Event, EventEnvelope,
    InputWait, Origin, ProjectId, PromptSend, PromptSendResult, Seq, ShellContext, TurnId,
};
use efr_provider::{Message, ProviderEvent, ProviderId, Request, ToolDefinition};
use efr_scope::{Derivation, Home};
use efr_stdx::id::uuid_v7;
use efr_store::Committed;
use efr_test_support::{
    Inbound, Outbound, Record, ReplayProvider, TestClock, TestDirs, TestRng, TestStore, Transcript,
};
use jiff::civil::date;
use serde_json::{Map, Value, json};
use tokio::sync::{Notify, broadcast, watch};

use crate::preamble::LiveState;
use crate::resolver::machine;
use crate::scratch;
use crate::{
    CallContext, ConversationActor, ConversationConfig, ConversationDeps, ConversationHandle,
    ConversationStart, HostInfo, OutputSink, ScopeResolver, ToolCall, ToolOutcome, Toolbox,
};

pub(crate) const MODEL: &str = "test-model";
pub(crate) const SYSTEM: &str = "You are the efr test model.";
pub(crate) const HOST: &str = "testhost";
pub(crate) const OS: &str = "TestOS";

/// Tools that declare what their input names and answer with fixed text.
///
/// - `read_file {path}` reads a path and answers `contents of <path>`;
/// - `write_file {path, content}` writes a path and answers `written <path>`;
/// - `shell {command}` runs a command, reports `partial` as output, yields, then
///   answers `done` with exit code 0; the command `ask-password` instead prints a
///   prompt, waits for hidden input, takes an answer and prints `ok`, yielding between
///   the steps;
/// - `hang {}` declares nothing, signals [`FakeToolbox::hang_started`] and never ends.
#[derive(Debug, Default)]
pub(crate) struct FakeToolbox {
    invoked: Mutex<Vec<(String, Value)>>,
    cancelled: Mutex<Vec<CallId>>,
    pub(crate) hang_started: Notify,
    /// What [`Toolbox::shell_cwd`] answers.
    pub(crate) shell_cwd: Mutex<Option<PathBuf>>,
    /// The context of every call that `requirements` was asked about.
    judged: Mutex<Vec<CallContext>>,
}

impl FakeToolbox {
    pub(crate) fn tools() -> Vec<ToolDefinition> {
        let tool = |name: &str, description: &str, properties: Value| ToolDefinition {
            name: name.to_owned(),
            description: description.to_owned(),
            input_schema: json!({"type": "object", "properties": properties}),
        };
        vec![
            tool("read_file", "Reads a file.", json!({"path": {"type": "string"}})),
            tool(
                "write_file",
                "Writes a file.",
                json!({"path": {"type": "string"}, "content": {"type": "string"}}),
            ),
            tool("shell", "Runs a command.", json!({"command": {"type": "string"}})),
            tool("hang", "Never ends.", json!({})),
        ]
    }

    /// The name and input of every call that reached `invoke`, in order.
    pub(crate) fn invoked(&self) -> Vec<(String, Value)> {
        self.invoked.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// The calls whose work `cancel` was asked to stop.
    pub(crate) fn cancelled(&self) -> Vec<CallId> {
        self.cancelled.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// The context of every call that the check point judged, in order.
    pub(crate) fn judged(&self) -> Vec<CallContext> {
        self.judged.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

fn text_input(input: &Value, key: &str) -> Result<String, String> {
    input
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("the input has no {key}"))
}

#[async_trait]
impl Toolbox for FakeToolbox {
    fn definitions(&self) -> Vec<ToolDefinition> {
        FakeToolbox::tools()
    }

    async fn requirements(&self, call: &ToolCall) -> Result<Requirements, String> {
        self.judged.lock().unwrap_or_else(PoisonError::into_inner).push(call.context.clone());
        match call.name.as_str() {
            "read_file" => Ok(Requirements::none().with_read(text_input(&call.input, "path")?)),
            "write_file" => Ok(Requirements::none().with_write(text_input(&call.input, "path")?)),
            "shell" => Ok(Requirements::none().with_command(text_input(&call.input, "command")?)),
            "hang" => Ok(Requirements::none()),
            other => Err(format!("no tool is named {other:?}")),
        }
    }

    async fn preview(&self, call: &ToolCall) -> Option<String> {
        (call.name == "write_file").then(|| format!("+{}", call.input["content"]))
    }

    async fn shell_cwd(&self, _conversation_id: ConversationId) -> Option<PathBuf> {
        self.shell_cwd.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    async fn invoke(&self, call: ToolCall, out: &mut dyn OutputSink) -> ToolOutcome {
        self.invoked
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((call.name.clone(), call.input.clone()));
        let path = call.input.get("path").and_then(Value::as_str).unwrap_or_default();
        match call.name.as_str() {
            "read_file" => ToolOutcome::ok(format!("contents of {path}")),
            "write_file" => ToolOutcome::ok(format!("written {path}")),
            "shell" if call.input["command"] == "ask-password" => {
                out.update("pw: ", 4);
                tokio::task::yield_now().await;
                out.input_changed(InputWait::Hidden);
                tokio::task::yield_now().await;
                out.update("pw: \nok\n", 8);
                out.input_changed(InputWait::None);
                ToolOutcome::ok("pw: \nok\n").with_exit_code(Some(0))
            }
            "shell" => {
                out.update("partial", 7);
                // NOTE: a real command takes time; yielding lets the turn see the update
                // before the outcome, as it would.
                tokio::task::yield_now().await;
                ToolOutcome::ok("done").with_exit_code(Some(0))
            }
            "hang" => {
                self.hang_started.notify_one();
                std::future::pending().await
            }
            other => ToolOutcome::error(format!("no tool is named {other:?}")),
        }
    }

    async fn cancel(&self, call: &CallContext) {
        self.cancelled.lock().unwrap_or_else(PoisonError::into_inner).push(call.call_id);
    }
}

/// A resolver that answers from a table and gives `Machine` for any other directory.
#[derive(Debug, Default)]
pub(crate) struct FakeScope {
    scopes: HashMap<PathBuf, Derivation>,
}

impl FakeScope {
    pub(crate) fn with(mut self, cwd: impl Into<PathBuf>, derivation: Derivation) -> Self {
        self.scopes.insert(cwd.into(), derivation);
        self
    }
}

#[async_trait]
impl ScopeResolver for FakeScope {
    async fn resolve(&self, cwd: &Path) -> Derivation {
        self.scopes.get(cwd).cloned().unwrap_or_else(machine)
    }
}

/// The paths and ids of a test, known before the actor runs, so the test can build
/// its expected requests.
#[derive(Debug)]
pub(crate) struct Setup {
    pub(crate) dirs: TestDirs,
    pub(crate) clock: TestClock,
    pub(crate) conversation_id: ConversationId,
    pub(crate) cwd: PathBuf,
    pub(crate) config: ConversationConfig,
    pub(crate) scope: FakeScope,
    /// A registered project the engine knows, with its root.
    pub(crate) project: Option<(ProjectId, PathBuf)>,
    /// The seed of the actor's generator. A restarted actor needs another one, or it
    /// would make the ids of the first actor again.
    rng_seed: u64,
}

impl Setup {
    pub(crate) fn new() -> Self {
        let dirs = TestDirs::new().expect("temporary directories");
        let clock = TestClock::new();
        let conversation_id = ConversationId::from_uuid(uuid_v7(&clock, &TestRng::new(1)));
        let cwd = dirs.create_dir("home/project").expect("working directory");
        let mut config = ConversationConfig::new(MODEL, dirs.dirs().data().join("scratch"))
            .with_system_prompt(SYSTEM)
            .with_host(HostInfo::new(Some(HOST.to_owned()), Some(OS.to_owned())));
        // NOTE: every update is sent at once unless a test asks for coalescing, so the
        // expected events do not depend on how the clock moves.
        config.update_interval = Duration::ZERO;
        Setup {
            dirs,
            clock,
            conversation_id,
            cwd,
            config,
            scope: FakeScope::default(),
            project: None,
            rng_seed: 7,
        }
    }

    pub(crate) fn home(&self) -> &Path {
        self.dirs.home()
    }

    /// The scratch directory the conversation claims, when its first prompt is
    /// `title`.
    pub(crate) fn scratch(&self, title: &str) -> PathBuf {
        let names = scratch::names(date(2026, 10, 4), &scratch::slug(title), self.conversation_id);
        self.config.scratch_root.join(&names[0])
    }

    /// The live state of a turn in `cwd` that has nothing else to say.
    pub(crate) fn live_state(&self, cwd: &Path, title: &str) -> LiveState {
        LiveState {
            cwd: cwd.to_path_buf(),
            oldpwd: None,
            last_command: None,
            last_status: None,
            repo: None,
            home: self.home().to_path_buf(),
            host: Some(HOST.to_owned()),
            os: Some(OS.to_owned()),
            ssh: false,
            scratch: self.scratch(title),
            agent_cwd: None,
        }
    }

    /// The newest prompt as the request carries it: the preamble, then the text.
    pub(crate) fn prompt(&self, state: &LiveState, text: &str) -> Message {
        user_prompt(state, text)
    }

    pub(crate) async fn start(self, records: Vec<Record>) -> Harness {
        self.start_with(records, ConversationStart::New { origin: Origin::Shell, tty: None }, None)
            .await
    }

    /// Starts the actor over the transcript `records`, with a store that holds the
    /// events of an earlier actor when `store` is given.
    pub(crate) async fn start_with(
        self,
        records: Vec<Record>,
        start: ConversationStart,
        store: Option<TestStore>,
    ) -> Harness {
        let transcript = Transcript::from_records(records);
        let provider = Arc::new(ReplayProvider::new(&transcript).expect("transcript").paced());
        self.start_provider(provider, start, store).await
    }

    pub(crate) async fn start_provider(
        self,
        provider: Arc<ReplayProvider>,
        start: ConversationStart,
        store: Option<TestStore>,
    ) -> Harness {
        let store = match store {
            Some(store) => store,
            None => TestStore::open(self.clock.shared()).await.expect("store"),
        };
        let events = store.writer().subscribe();
        let toolbox = Arc::new(FakeToolbox::default());
        let home = Home::new(self.home()).expect("home");
        let mut locations = Locations::new(self.home()).expect("locations");
        if let Some((id, root)) = &self.project {
            locations = locations.with_project(*id, root).expect("project root");
        }
        let engine = watch::channel(Arc::new(Engine::with_defaults(locations))).1;
        let deps = ConversationDeps {
            provider: provider.clone(),
            toolbox: toolbox.clone(),
            engine,
            scope: Arc::new(self.scope),
            writer: store.writer().clone(),
            readers: store.readers().clone(),
            clock: self.clock.shared(),
            rng: Arc::new(TestRng::new(self.rng_seed)),
            home,
        };
        let handle =
            ConversationActor::spawn(self.conversation_id, start, self.config.clone(), deps);
        Harness {
            config: self.config,
            dirs: self.dirs,
            clock: self.clock,
            store,
            toolbox,
            provider,
            handle,
            events,
            conversation_id: self.conversation_id,
            cwd: self.cwd,
            next_command: 100,
        }
    }
}

/// A running actor and everything around it.
#[derive(Debug)]
pub(crate) struct Harness {
    pub(crate) dirs: TestDirs,
    pub(crate) clock: TestClock,
    pub(crate) store: TestStore,
    pub(crate) toolbox: Arc<FakeToolbox>,
    pub(crate) provider: Arc<ReplayProvider>,
    pub(crate) handle: ConversationHandle,
    events: broadcast::Receiver<Committed>,
    pub(crate) conversation_id: ConversationId,
    pub(crate) cwd: PathBuf,
    config: ConversationConfig,
    next_command: u64,
}

impl Harness {
    /// Stops this actor and starts a new one for the same conversation over the same
    /// store, as the daemon does after a restart, with a provider named `provider_id`
    /// that answers from `records`.
    pub(crate) async fn restart(self, records: Vec<Record>, provider_id: &str) -> Harness {
        self.handle.shutdown().await.expect("the actor stops");
        let transcript = Transcript::from_records(records);
        let provider = ReplayProvider::new(&transcript)
            .expect("transcript")
            .with_id(ProviderId::new(provider_id).expect("provider id"))
            .paced();
        let setup = Setup {
            dirs: self.dirs,
            clock: self.clock,
            conversation_id: self.conversation_id,
            cwd: self.cwd,
            config: self.config,
            scope: FakeScope::default(),
            project: None,
            rng_seed: 8,
        };
        let mut harness = setup
            .start_provider(Arc::new(provider), ConversationStart::Existing, Some(self.store))
            .await;
        harness.next_command = self.next_command;
        harness
    }

    /// The handle, the store, the provider and the directories, with the rest
    /// dropped.
    pub(crate) fn into_parts(
        self,
    ) -> (ConversationHandle, TestStore, Arc<ReplayProvider>, TestDirs) {
        (self.handle, self.store, self.provider, self.dirs)
    }

    /// A command id no other command of the test has.
    pub(crate) fn command_id(&mut self) -> CommandId {
        self.next_command += 1;
        CommandId::from_uuid(uuid_v7(&self.clock, &TestRng::new(self.next_command)))
    }

    /// Sends `text` from the shell in `cwd`.
    pub(crate) async fn prompt_in(&mut self, cwd: &Path, text: &str) -> PromptSendResult {
        let params = self.prompt_params(cwd, text);
        self.handle.send_prompt(params, Origin::Shell).await.expect("prompt accepted")
    }

    /// Sends `text` from the shell in the test's working directory.
    pub(crate) async fn prompt(&mut self, text: &str) -> PromptSendResult {
        let cwd = self.cwd.clone();
        self.prompt_in(&cwd, text).await
    }

    pub(crate) fn prompt_params(&mut self, cwd: &Path, text: &str) -> PromptSend {
        PromptSend {
            command_id: self.command_id(),
            conversation_id: None,
            new_conversation: false,
            text: text.to_owned(),
            context: Some(ShellContext::new(cwd)),
            last_command: None,
        }
    }

    /// Waits until an event that `wanted` accepts is committed, and returns it.
    pub(crate) async fn wait_for(&mut self, wanted: impl Fn(&Event) -> bool) -> Event {
        loop {
            let committed = self.events.recv().await.expect("the store broadcasts");
            if let Some(envelope) = committed.events().iter().find(|e| wanted(&e.event)) {
                return envelope.event.clone();
            }
        }
    }

    /// Waits until the turn `turn_id` has its terminal event.
    pub(crate) async fn wait_end(&mut self, turn_id: TurnId) -> Event {
        self.wait_for(|event| {
            event.turn_id() == Some(turn_id)
                && matches!(
                    event,
                    Event::TurnCompleted { .. }
                        | Event::TurnFailed { .. }
                        | Event::TurnInterrupted { .. }
                )
        })
        .await
    }

    /// Waits until a tool call asks for approval, and returns its call id.
    pub(crate) async fn wait_approval(&mut self) -> CallId {
        match self.wait_for(|event| matches!(event, Event::ApprovalRequested { .. })).await {
            Event::ApprovalRequested { call_id, .. } => call_id,
            _ => unreachable!("wait_for returns an event it accepted"),
        }
    }

    pub(crate) async fn answer(&mut self, call_id: CallId, decision: ApprovalDecision) -> Seq {
        let params = ApprovalRespond {
            command_id: self.command_id(),
            conversation_id: self.conversation_id,
            call_id,
            decision,
        };
        self.handle.respond_approval(params, Origin::Shell).await.expect("answer accepted").seq
    }

    /// Every event in the log.
    pub(crate) async fn envelopes(&self) -> Vec<EventEnvelope> {
        self.store.events().await.expect("events")
    }

    /// Every event in the log, without its envelope.
    pub(crate) async fn events(&self) -> Vec<Event> {
        self.envelopes().await.into_iter().map(|envelope| envelope.event).collect()
    }

    /// The kinds of every event in the log, in order.
    pub(crate) async fn kinds(&self) -> Vec<String> {
        self.events().await.iter().map(|event| event.kind().to_owned()).collect()
    }

    /// The call ids of the `tool_call_started` events, in order.
    pub(crate) async fn call_ids(&self) -> Vec<CallId> {
        self.events()
            .await
            .into_iter()
            .filter_map(|event| match event {
                Event::ToolCallStarted { call_id, .. } => Some(call_id),
                _ => None,
            })
            .collect()
    }

    /// Checks that the replay served every exchange and every request matched.
    pub(crate) fn finish(&self) {
        if let Err(error) = self.provider.finish() {
            panic!("the replay did not go as the transcript says: {error}");
        }
    }
}

/// The newest prompt as the request carries it: the preamble, then the text.
pub(crate) fn user_prompt(state: &LiveState, text: &str) -> Message {
    use efr_provider::{ContentBlock, Role};
    Message::new(
        Role::User,
        vec![
            ContentBlock::Text { text: state.render() },
            ContentBlock::Text { text: text.to_owned() },
        ],
    )
}

/// The transcript record of a request.
pub(crate) fn expect_request(request: Request) -> Record {
    Record::ExpectOutbound(Outbound::ProviderRequest(
        serde_json::to_value(request).expect("a request serializes"),
    ))
}

/// The request a turn sends with `messages`.
pub(crate) fn request(messages: Vec<Message>) -> Request {
    Request {
        model: MODEL.to_owned(),
        system: Some(SYSTEM.to_owned()),
        messages,
        tools: FakeToolbox::tools(),
        max_output_tokens: None,
        provider_options: Map::new(),
    }
}

/// The transcript record of an answer made of `events`.
pub(crate) fn answer(events: &[ProviderEvent]) -> Record {
    let body: String = events
        .iter()
        .map(|event| format!("data: {}\n\n", serde_json::to_string(event).expect("serializes")))
        .collect();
    Record::EmitInbound(Inbound::ProviderSse(body))
}

/// The transcript record of an answer that fails with the replay error `error`, such
/// as `{"kind": "unauthorized"}`.
pub(crate) fn failure(error: Value) -> Record {
    Record::EmitInbound(Inbound::ProviderSse(format!("event: error\ndata: {error}\n\n")))
}

/// A record that holds the next answer of a paced provider until the test releases it
/// with `handled_through` and the record's line.
pub(crate) fn hold() -> Record {
    Record::ClockAdvance(Duration::ZERO)
}

/// A complete text answer.
pub(crate) fn text_answer(text: &str) -> Vec<ProviderEvent> {
    vec![
        ProviderEvent::TextDelta { text: text.to_owned() },
        done(efr_provider::StopReason::EndTurn, None),
    ]
}

/// An answer that calls one tool.
pub(crate) fn tool_answer(call_id: &str, name: &str, input: &Value) -> Vec<ProviderEvent> {
    vec![
        ProviderEvent::ToolCallStart { call_id: call_id.to_owned(), name: name.to_owned() },
        ProviderEvent::ToolCallEnd { call_id: call_id.to_owned(), arguments: input.to_string() },
        done(efr_provider::StopReason::ToolUse, None),
    ]
}

pub(crate) fn done(stop_reason: efr_provider::StopReason, raw: Option<Value>) -> ProviderEvent {
    ProviderEvent::Done { stop_reason, provider_raw: raw }
}

/// The assistant message of [`tool_answer`].
pub(crate) fn tool_message(call_id: &str, name: &str, input: &Value) -> Message {
    use efr_provider::{ContentBlock, Role};
    Message::new(
        Role::Assistant,
        vec![ContentBlock::ToolCall {
            call_id: call_id.to_owned(),
            name: name.to_owned(),
            input: input.clone(),
        }],
    )
}

/// The user message that answers one tool call.
pub(crate) fn result_message(call_id: &str, output: &str, is_error: bool) -> Message {
    use efr_provider::{ContentBlock, Role};
    Message::new(
        Role::User,
        vec![ContentBlock::ToolResult {
            call_id: call_id.to_owned(),
            output: output.to_owned(),
            is_error,
        }],
    )
}

/// The event of `events` that `wanted` accepts.
pub(crate) fn find(events: &[Event], wanted: impl Fn(&Event) -> bool) -> Event {
    events.iter().find(|event| wanted(event)).cloned().expect("the event is in the log")
}

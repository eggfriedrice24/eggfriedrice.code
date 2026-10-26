//! Recordings of `conversation.subscribe` streams for the phone app's replay tests
//! (`app/test/recordings/`): what a phone that subscribes with drafts gets during a
//! scripted turn, one item per line.
//!
//! A scripted model sends one provider event at a time. The test sends the next one
//! only when a second subscription with drafts got the draft of the one before, so
//! every delta becomes a draft of its own, as on a fast link. The test clock moves
//! 50 ms per delta, so the turn also records an `assistant_message_updated` now and
//! then. The phone's subscription reads at its own pace in a task, and its items are
//! the recording. The order of a draft and an event that come together can differ
//! between runs, so the test does not compare bytes: each run must replay to the
//! completed texts with no gap, and its events must match the committed recording.
//! `recordings::bless_recordings` writes the recordings again (`just bless`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use efr_protocol::{
    ApprovalDecision, ApprovalRespond, ApprovalRespondResult, CallId, ConversationId,
    ConversationSubscribe, ConversationSubscribeItem, DraftPart, ErrorCode, Event, Method, Origin,
    PromptSendResult, TurnId,
};
use efr_provider::{
    Provider, ProviderError, ProviderEvent, ProviderId, ProviderStream, Request, StopReason,
};
use efr_test_daemon::{Client, ClientError, ItemStream, TTY, TestDaemon, command_id};
use futures::StreamExt as _;
use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

/// How far the test clock moves before each delta of the model.
const DELTA_GAP: Duration = Duration::from_millis(50);

/// A model whose provider events the test sends one at a time. Each model call
/// streams the events up to its `done`.
#[derive(Debug)]
struct Lockstep {
    id: ProviderId,
    /// The test's events. The stream of a model call takes the receiver and puts it
    /// back at its `done`, so the next call goes on with it.
    events: Arc<Mutex<Option<mpsc::UnboundedReceiver<ProviderEvent>>>>,
}

#[async_trait]
impl Provider for Lockstep {
    fn id(&self) -> &ProviderId {
        &self.id
    }

    async fn stream(&self, request: Request) -> Result<ProviderStream, ProviderError> {
        assert!(!request.one_shot, "a cautious turn asks no reviewer");
        let receiver = self.events.lock().unwrap().take().expect("one model call at a time");
        let holder = Arc::clone(&self.events);
        let stream = futures::stream::unfold(Some(receiver), move |receiver| {
            let holder = Arc::clone(&holder);
            async move {
                let mut receiver = receiver?;
                let event = receiver.recv().await?;
                if matches!(event, ProviderEvent::Done { .. }) {
                    *holder.lock().unwrap() = Some(receiver);
                    return Some((Ok(event), None));
                }
                Some((Ok(event), Some(receiver)))
            }
        });
        Ok(Box::pin(stream))
    }
}

/// One step of a scripted turn.
#[derive(Debug, Clone)]
enum Step {
    /// The model sends text.
    Text(String),
    /// The model sends reasoning.
    Reasoning(String),
    /// The model calls `tool` with `input`, written in `pieces` deltas.
    Tool { tool: &'static str, input: Value, pieces: usize },
    /// The model ends its answer for `reason`; the test waits for an event of `until`.
    Done { reason: StopReason, until: &'static str },
    /// The phone answers the question that waits. When the question is local only,
    /// the phone's answer is refused and the terminal answers with `decision`.
    Answer { decision: ApprovalDecision, local_only: bool },
    /// The phone subscribes here, in the middle of the turn.
    Join,
}

/// One recording: its name, the prompt and the steps of the turn.
struct Scenario {
    name: &'static str,
    prompt: &'static str,
    /// A file that the test writes in the working directory first, with its text.
    file: Option<(&'static str, &'static str)>,
    steps: Vec<Step>,
}

/// `text` cut into deltas of at most `chars` characters each, on character
/// boundaries, so a delta can end inside an emoji of several code points.
fn chunks(text: &str, chars: usize) -> Vec<Step> {
    let all: Vec<char> = text.chars().collect();
    all.chunks(chars).map(|chunk| Step::Text(chunk.iter().collect())).collect()
}

/// `text` cut into deltas of `words` words each, as a model streams prose.
fn word_chunks(text: &str, words: usize) -> Vec<Step> {
    let mut steps = Vec::new();
    let mut current = String::new();
    let mut count = 0;
    for piece in text.split_inclusive(' ') {
        current.push_str(piece);
        count += 1;
        if count == words {
            steps.push(Step::Text(std::mem::take(&mut current)));
            count = 0;
        }
    }
    if !current.is_empty() {
        steps.push(Step::Text(current));
    }
    steps
}

/// The end of a model answer that ends the turn.
fn end_turn() -> Step {
    Step::Done { reason: StopReason::EndTurn, until: "turn_completed" }
}

/// About 20 KB of Markdown prose: headings, paragraphs and lists.
fn long_text() -> String {
    const WORDS: &[&str] = &[
        "the",
        "daemon",
        "keeps",
        "each",
        "conversation",
        "in",
        "one",
        "log",
        "and",
        "a",
        "client",
        "reads",
        "it",
        "from",
        "any",
        "point",
        "so",
        "phone",
        "can",
        "follow",
        "turn",
        "while",
        "model",
        "writes",
        "text",
        "terminal",
        "shows",
        "same",
        "words",
        "at",
        "time",
        "with",
        "no",
        "gap",
        "between",
        "drafts",
        "events",
    ];
    let mut text = String::new();
    let mut n = 0_usize;
    for section in 1..=8 {
        text.push_str(&format!("## Part {section}\n\n"));
        for paragraph in 0..4 {
            let mut sentence_start = true;
            for word in 0..90 {
                let mut next = WORDS[(n * 7 + word * 3 + paragraph) % WORDS.len()].to_owned();
                n += 1;
                if sentence_start {
                    next[..1].make_ascii_uppercase();
                    sentence_start = false;
                }
                text.push_str(&next);
                if word % 15 == 14 {
                    text.push('.');
                    sentence_start = true;
                }
                text.push(' ');
            }
            text.pop();
            text.push_str("\n\n");
        }
        text.push_str("- the first point of this part\n- the second point, with `code`\n");
        text.push_str("- the third point, in **bold** and *italic*\n\n");
    }
    text.push_str("That is the whole answer.");
    text
}

const CODE_TEXT: &str = "Here is the change in Rust:

```rust
fn utf8_len(text: &str) -> usize {
    text.len()
}
```

And the same rule in TypeScript:

```ts
export function utf8Length(text: string): number {
  let bytes = 0;
  for (const char of text) {
    const code = char.codePointAt(0) ?? 0;
    bytes += code < 0x80 ? 1 : code < 0x800 ? 2 : code < 0x10000 ? 3 : 4;
  }
  return bytes;
}
```

Run the tests with `just test`:

```bash
just test
```

An inline `code span` and a fence without a language:

```
plain text in a fence
```";

const TABLE_TEXT: &str = "The sizes of each part:

| Part | Bytes | Note |
|:-----|------:|:----:|
| text | 1024 | the reply so far |
| reasoning | 256 | *not stored* |
| tool input | 12 | `apply_patch` |
| tool output | 4096 | the tail only |

A second table with fewer columns:

| Key | Value |
|---|---|
| port | 7419 |
| enabled | false |

That is all.";

const MULTIBYTE_TEXT: &str = "Text in many scripts: 日本語のテキスト, 한국어 문장, 中文的句子, \
Ελληνικά, Русский, עברית and العربية.

Emoji: 🙂 👍🏽 👩‍👩‍👧 🇯🇵 🏳️‍🌈 and a keycap 1️⃣.

Accents: é, ñ, ü, and an e with a combining mark: e\u{301}.

Wide and narrow mixed: ａｂｃ abc ＡＢＣ.";

const REASONING: &str = "**Reading the request**

The user wants the size of the log. I check the store first.

**Planning the answer**

Two numbers matter: the events and the bytes.";

const REASONING_ANSWER: &str = "The log holds **1204 events** in 3.2 MB.";

const NOTES: &str = "# Notes

- buy milk
- call the bank
";

const NEW_NOTES: &str = "# Notes

- buy milk and bread
- call the bank
- fix the bike
";

/// Every scenario that the app's tests replay.
fn scenarios() -> Vec<Scenario> {
    let long = long_text();
    let mut join = vec![Step::Reasoning("**Thinking**\n\nThe user asks for a long text.".into())];
    // NOTE: the text is ASCII, so its middle is a character boundary.
    let middle = long.len() / 2;
    join.extend(word_chunks(&long[..middle], 4));
    join.push(Step::Join);
    join.extend(word_chunks(&long[middle..], 4));
    join.push(end_turn());
    vec![
        Scenario {
            name: "long_reply",
            prompt: "write a long text",
            file: None,
            steps: [word_chunks(&long, 4), vec![end_turn()]].concat(),
        },
        Scenario {
            name: "code_blocks",
            prompt: "show the code",
            file: None,
            steps: [chunks(CODE_TEXT, 9), vec![end_turn()]].concat(),
        },
        Scenario {
            name: "tables",
            prompt: "show the sizes",
            file: None,
            steps: [chunks(TABLE_TEXT, 7), vec![end_turn()]].concat(),
        },
        Scenario {
            name: "multibyte",
            prompt: "show some scripts",
            file: None,
            steps: [chunks(MULTIBYTE_TEXT, 2), vec![end_turn()]].concat(),
        },
        Scenario {
            name: "reasoning",
            prompt: "how big is the log",
            file: None,
            steps: [
                REASONING
                    .split_inclusive(' ')
                    .map(|piece| Step::Reasoning(piece.to_owned()))
                    .collect(),
                word_chunks(REASONING_ANSWER, 2),
                vec![end_turn()],
            ]
            .concat(),
        },
        Scenario {
            name: "tool_output",
            prompt: "read my notes",
            file: Some(("notes.md", NOTES)),
            steps: [
                word_chunks("I read the notes first.", 2),
                vec![
                    Step::Tool {
                        tool: "read_file",
                        input: json!({ "path": "notes.md" }),
                        pieces: 3,
                    },
                    Step::Done { reason: StopReason::ToolUse, until: "tool_call_completed" },
                ],
                word_chunks("The notes have two items: milk and the bank.", 2),
                vec![end_turn()],
            ]
            .concat(),
        },
        Scenario {
            name: "approval_in_the_middle",
            prompt: "update my notes and restart the web server",
            file: Some(("notes.md", NOTES)),
            steps: [
                word_chunks("I change the notes now.", 2),
                vec![
                    Step::Tool {
                        tool: "write_file",
                        input: json!({ "path": "notes.md", "content": NEW_NOTES }),
                        pieces: 4,
                    },
                    Step::Done { reason: StopReason::ToolUse, until: "approval_requested" },
                    Step::Answer { decision: ApprovalDecision::Allow, local_only: false },
                ],
                word_chunks("The notes are updated. Now I restart the server.", 2),
                vec![
                    Step::Tool {
                        tool: "shell",
                        input: json!({ "command": "sudo systemctl restart nginx" }),
                        pieces: 2,
                    },
                    Step::Done { reason: StopReason::ToolUse, until: "approval_requested" },
                    Step::Answer { decision: ApprovalDecision::Deny, local_only: true },
                ],
                word_chunks("You did not allow the restart, so the server still runs.", 2),
                vec![end_turn()],
            ]
            .concat(),
        },
        Scenario { name: "join_mid_reply", prompt: "write a long text", file: None, steps: join },
    ]
}

/// The directory of the recordings in the repository.
fn recordings_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../app/test/recordings")
}

/// A scripted turn on a test daemon, as the phone sees it.
struct Run {
    daemon: TestDaemon,
    terminal: Client,
    phone: Client,
    model: mpsc::UnboundedSender<ProviderEvent>,
    conversation_id: ConversationId,
    /// The terminal's subscription with drafts, which paces the model.
    pacer: ItemStream<ConversationSubscribeItem>,
    /// The phone's subscription, read in a task until the turn ends.
    reader: Option<JoinHandle<Vec<ConversationSubscribeItem>>>,
    /// The question that waits, from the pacer.
    asked: Option<(CallId, bool)>,
    next_command: u128,
}

impl Run {
    async fn start(scenario: &Scenario) -> Run {
        let (model, events) = mpsc::unbounded_channel();
        let provider = Lockstep {
            id: ProviderId::new("test").unwrap(),
            events: Arc::new(Mutex::new(Some(events))),
        };
        let daemon = TestDaemon::builder()
            .custom_provider(Arc::new(provider) as Arc<dyn Provider>)
            .config(|config| config.conversation.draft_interval_ms = 0)
            .start()
            .await
            .unwrap();
        if let Some((name, text)) = scenario.file {
            std::fs::write(daemon.cwd().join(name), text).unwrap();
        }
        let terminal = daemon.client_for_tty(TTY).await.unwrap();
        let phone = daemon.connect(daemon.connect_options_from(Origin::Phone)).await.unwrap();
        let sent: PromptSendResult =
            terminal.call(daemon.prompt(1, scenario.prompt, TTY)).await.unwrap();
        let conversation_id = sent.conversation_id;
        let (pacer, _) = subscribe(&terminal, conversation_id).await;
        let mut run = Run {
            daemon,
            terminal,
            phone,
            model,
            conversation_id,
            pacer,
            reader: None,
            asked: None,
            next_command: 2,
        };
        if !scenario.steps.iter().any(|step| matches!(step, Step::Join)) {
            run.join().await;
        }
        run
    }

    /// The phone subscribes with drafts, and a task reads its items until the turn ends.
    async fn join(&mut self) {
        let (mut stream, first) = subscribe(&self.phone, self.conversation_id).await;
        self.reader = Some(tokio::spawn(async move {
            let mut items = vec![first];
            while let Some(item) = stream.next().await {
                let item = item.unwrap();
                let ended = matches!(&item, ConversationSubscribeItem::Event(envelope)
                    if matches!(envelope.event, Event::TurnCompleted { .. }));
                items.push(item);
                if ended {
                    break;
                }
            }
            items
        }));
    }

    /// Reads the pacer until `done` accepts an item; notes the question that waits.
    async fn pace(&mut self, done: impl Fn(&ConversationSubscribeItem) -> bool) {
        loop {
            let item = self.pacer.next().await.unwrap().unwrap();
            if let ConversationSubscribeItem::Event(envelope) = &item
                && let Event::ApprovalRequested { call_id, local_only, .. } = envelope.event
            {
                self.asked = Some((call_id, local_only));
            }
            if done(&item) {
                return;
            }
        }
    }

    /// Sends `event` to the model after the clock moved, and waits for its draft.
    async fn delta(&mut self, event: ProviderEvent, draft: fn(&DraftPart) -> bool) {
        self.daemon.clock().advance(DELTA_GAP);
        self.model.send(event).unwrap();
        self.pace(|item| matches!(item, ConversationSubscribeItem::Draft(d) if draft(&d.draft)))
            .await;
    }

    async fn step(&mut self, step: Step) {
        match step {
            Step::Text(text) => {
                let event = ProviderEvent::TextDelta { text };
                self.delta(event, |part| matches!(part, DraftPart::Text { .. })).await;
            }
            Step::Reasoning(text) => {
                let event = ProviderEvent::ReasoningDelta { text };
                self.delta(event, |part| matches!(part, DraftPart::Reasoning { .. })).await;
            }
            Step::Tool { tool, input, pieces } => {
                let call_id = format!("call_{tool}");
                let start = ProviderEvent::ToolCallStart {
                    call_id: call_id.clone(),
                    name: tool.to_owned(),
                    freeform: false,
                };
                let input_draft = |part: &DraftPart| matches!(part, DraftPart::ToolInput { .. });
                self.delta(start, input_draft).await;
                let arguments = input.to_string();
                let size = arguments.len().div_ceil(pieces);
                for piece in arguments.as_bytes().chunks(size) {
                    let piece = String::from_utf8(piece.to_vec()).unwrap();
                    let event =
                        ProviderEvent::ToolCallDelta { call_id: call_id.clone(), arguments: piece };
                    self.delta(event, input_draft).await;
                }
                self.model.send(ProviderEvent::ToolCallEnd { call_id, arguments }).unwrap();
            }
            Step::Done { reason, until } => {
                self.model
                    .send(ProviderEvent::Done { stop_reason: reason, provider_raw: None })
                    .unwrap();
                self.pace(|item| {
                    matches!(item, ConversationSubscribeItem::Event(envelope)
                    if envelope.event.kind() == until)
                })
                .await;
            }
            Step::Answer { decision, local_only } => {
                let (call_id, asked_local_only) = self.asked.take().expect("a question waits");
                assert_eq!(asked_local_only, local_only, "the question's local_only");
                let answer = self.answer(call_id, decision);
                let from_phone = self.phone.call::<ApprovalRespondResult>(answer).await;
                if local_only {
                    let Err(ClientError::Server { body }) = from_phone else {
                        panic!("a phone never answers a local-only question: {from_phone:?}");
                    };
                    assert_eq!(body.code, ErrorCode::Forbidden, "{body:?}");
                    let answer = self.answer(call_id, decision);
                    self.terminal.call::<ApprovalRespondResult>(answer).await.unwrap();
                } else {
                    from_phone.unwrap();
                }
                self.pace(|item| {
                    matches!(item, ConversationSubscribeItem::Event(envelope)
                    if envelope.event.kind() == "tool_call_completed")
                })
                .await;
            }
            Step::Join => self.join().await,
        }
    }

    fn answer(&mut self, call_id: CallId, decision: ApprovalDecision) -> Method {
        self.next_command += 1;
        Method::ApprovalRespond(ApprovalRespond {
            command_id: command_id(self.next_command),
            conversation_id: self.conversation_id,
            call_id,
            decision,
        })
    }

    /// Ends the run: the phone's items, each as one line of JSON with the test's paths
    /// replaced by fixed ones.
    async fn finish(mut self) -> Vec<String> {
        let items = self.reader.take().expect("the phone joined").await.unwrap();
        let home = self.daemon.dirs().home().display().to_string();
        let root = self.daemon.dirs().root().display().to_string();
        let lines = items
            .iter()
            .map(|item| {
                serde_json::to_string(item)
                    .unwrap()
                    .replace(&home, "/home/you")
                    .replace(&root, "/efr-test")
            })
            .collect();
        let Run { daemon, terminal, phone, pacer, .. } = self;
        drop((pacer, terminal, phone));
        daemon.stop().await.unwrap();
        lines
    }
}

/// Subscribes `client` to `conversation_id` with drafts; returns the stream and its
/// first item, the snapshot.
async fn subscribe(
    client: &Client,
    conversation_id: ConversationId,
) -> (ItemStream<ConversationSubscribeItem>, ConversationSubscribeItem) {
    let params = ConversationSubscribe {
        conversation_id,
        after_seq: None,
        answers_input: false,
        drafts: true,
    };
    let mut stream = client.stream(Method::ConversationSubscribe(params)).await.unwrap();
    let first = stream.next().await.unwrap().unwrap();
    assert!(matches!(first, ConversationSubscribeItem::Snapshot(_)), "{first:?}");
    (stream, first)
}

/// Runs `scenario` and returns the phone's recording.
async fn record(scenario: &Scenario) -> Vec<String> {
    let mut run = Run::start(scenario).await;
    for step in scenario.steps.clone() {
        run.step(step).await;
    }
    run.finish().await
}

/// What a client builds from a recording, as the app's reducer does: the text of each
/// message from the snapshot's `live`, the drafts and the updates, joined at their
/// byte offsets.
#[derive(Debug, Default)]
struct Replayed {
    texts: BTreeMap<(TurnId, u32), String>,
    reasoning: BTreeMap<TurnId, String>,
    /// The completed text of each message, in order.
    completed: Vec<String>,
    /// The kinds of the events that every run of the scenario has in the same order.
    skeleton: Vec<String>,
    /// The `local_only` of each question.
    questions: Vec<bool>,
    /// True when the snapshot held the live parts of a running turn.
    joined_live: bool,
}

/// The events whose order does not depend on the timing of a run.
const SKELETON: &[&str] = &[
    "turn_started",
    "assistant_message_completed",
    "tool_call_started",
    "approval_requested",
    "approval_resolved",
    "tool_call_completed",
    "turn_completed",
];

/// Appends `delta` at byte `offset` of `text`: the part that `text` already has is cut
/// off. A delta past the end of `text` is a gap, which no recording may have.
fn join_at(text: &mut String, offset: u64, delta: &str, line: usize) {
    let have = text.len() as u64;
    assert!(offset <= have, "line {line}: a gap at {offset}, the text has {have} bytes");
    let skip = usize::try_from(have - offset).unwrap();
    if let Some(rest) = delta.get(skip..) {
        text.push_str(rest);
    } else {
        assert!(skip >= delta.len(), "line {line}: an overlap ends inside a character");
    }
}

fn replay(lines: &[String]) -> Replayed {
    let mut replayed = Replayed::default();
    for (line, json) in lines.iter().enumerate() {
        let item: ConversationSubscribeItem = serde_json::from_str(json).unwrap();
        match item {
            ConversationSubscribeItem::Snapshot(snapshot) => {
                for envelope in snapshot.events {
                    replayed.event(envelope.event, line);
                }
                if let Some(live) = snapshot.live {
                    assert_eq!(live.text.len() as u64, live.text_bytes, "line {line}");
                    replayed.joined_live = !live.text.is_empty();
                    replayed.texts.insert((live.turn_id, live.index), live.text);
                    replayed.reasoning.insert(live.turn_id, live.reasoning.unwrap_or_default());
                }
            }
            ConversationSubscribeItem::Event(envelope) => replayed.event(envelope.event, line),
            ConversationSubscribeItem::Draft(draft) => match draft.draft {
                DraftPart::Text { index, offset, delta } => {
                    let text = replayed.texts.entry((draft.turn_id, index)).or_default();
                    join_at(text, offset, &delta, line);
                }
                DraftPart::Reasoning { offset, delta, .. } => {
                    let text = replayed.reasoning.entry(draft.turn_id).or_default();
                    join_at(text, offset, &delta, line);
                }
                _ => {}
            },
            other => panic!("line {line}: an item of no known kind: {other:?}"),
        }
    }
    replayed
}

impl Replayed {
    fn event(&mut self, event: Event, line: usize) {
        if SKELETON.contains(&event.kind()) {
            self.skeleton.push(event.kind().to_owned());
        }
        match event {
            Event::AssistantMessageUpdated { turn_id, index, offset, delta } => {
                let text = self.texts.entry((turn_id, index)).or_default();
                join_at(text, offset, &delta, line);
            }
            Event::AssistantMessageCompleted { turn_id, index, text } => {
                if let Some(built) = self.texts.get(&(turn_id, index)) {
                    assert_eq!(built, &text, "line {line}: the deltas build the completed text");
                }
                self.completed.push(text);
            }
            Event::ApprovalRequested { local_only, .. } => self.questions.push(local_only),
            _ => {}
        }
    }
}

/// The text that `steps` stream, joined, and the reasoning.
fn scripted(steps: &[Step]) -> (String, String) {
    let mut text = String::new();
    let mut reasoning = String::new();
    for step in steps {
        match step {
            Step::Text(delta) => text.push_str(delta),
            Step::Reasoning(delta) => reasoning.push_str(delta),
            _ => {}
        }
    }
    (text, reasoning)
}

/// Runs the scenario `name`, checks what the phone got, and checks that the committed
/// recording has the same events.
async fn check(name: &str) {
    let scenario = scenarios().into_iter().find(|scenario| scenario.name == name).unwrap();
    let lines = record(&scenario).await;
    let fresh = replay(&lines);
    let (text, reasoning) = scripted(&scenario.steps);
    assert_eq!(fresh.completed.concat(), text, "every message completed with its text");
    let streamed: String = fresh.texts.values().map(String::as_str).collect();
    assert_eq!(streamed, text, "the drafts and the updates hold every message");
    let reasoned: String = fresh.reasoning.values().map(String::as_str).collect();
    assert_eq!(reasoned, reasoning, "the drafts hold the reasoning");
    let joins = scenario.steps.iter().any(|step| matches!(step, Step::Join));
    assert_eq!(fresh.joined_live, joins, "a phone that joins late gets the text so far");
    let asked: Vec<bool> = scenario
        .steps
        .iter()
        .filter_map(|step| match step {
            Step::Answer { local_only, .. } => Some(*local_only),
            _ => None,
        })
        .collect();
    assert_eq!(fresh.questions, asked);

    let path = recordings_dir().join(format!("{name}.ndjson"));
    let committed = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{}: {error}; run `just bless`", path.display()));
    let committed: Vec<String> = committed.lines().map(str::to_owned).collect();
    let old = replay(&committed);
    assert_eq!(old.skeleton, fresh.skeleton, "{name}: the recording is old; run `just bless`");
    assert_eq!(old.completed, fresh.completed, "{name}: the recording is old; run `just bless`");
    assert_eq!(old.questions, fresh.questions);
}

#[tokio::test]
async fn long_reply() {
    check("long_reply").await;
}

#[tokio::test]
async fn code_blocks() {
    check("code_blocks").await;
}

#[tokio::test]
async fn tables() {
    check("tables").await;
}

#[tokio::test]
async fn multibyte() {
    check("multibyte").await;
}

#[tokio::test]
async fn reasoning() {
    check("reasoning").await;
}

#[tokio::test]
async fn tool_output() {
    check("tool_output").await;
}

#[tokio::test]
async fn approval_in_the_middle() {
    check("approval_in_the_middle").await;
}

#[tokio::test]
async fn join_mid_reply() {
    check("join_mid_reply").await;
}

#[tokio::test]
#[ignore = "rewrites the recordings of the phone app; `just bless` runs it"]
async fn bless_recordings() {
    let dir = recordings_dir();
    std::fs::create_dir_all(&dir).unwrap();
    for scenario in scenarios() {
        let lines = record(&scenario).await;
        let mut text = lines.join("\n");
        text.push('\n');
        std::fs::write(dir.join(format!("{}.ndjson", scenario.name)), text).unwrap();
    }
}

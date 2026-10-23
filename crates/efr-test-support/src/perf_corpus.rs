//! A seeded corpus of long conversations for the `bench_perf_*` benches, written into a
//! real store as the conversation actor writes it.
//!
//! Each turn holds about 50 events: the `prompt_queued` and the `turn_started`, four
//! rounds of an answer of about 400 bytes (three deltas and its
//! `assistant_message_completed`) with one `shell` call (its `tool_call_started`, four
//! `tool_call_output_updated` of about 150 bytes and a `tool_call_completed` of about
//! 1 KB), a last answer of about 1 KB in four deltas, and the `turn_completed`.
//! The batch of the turn also saves its exact messages (`efr_store::turn_messages`), as
//! the actor does at a turn's end: the prompt with a preamble, each answer with its
//! tool call and its Responses items as `provider_raw` (an encrypted reasoning item of
//! about 600 bytes, the message item and the `function_call` item), and each tool result.
//!
//! A size with a compaction ([`PerfShape::compacted_through`]) records a
//! `conversation_compacted` with a summary through that turn, inside the turn six turns
//! later, as an auto compaction of a turn does. The writer then drops the saved
//! messages before the cut, as in efrd, so a later turn rebuilds the covered turns from
//! their events. The conversations of a corpus are interleaved by turn: the first turn
//! of each conversation, then the second turn of each, and so on.
//!
//! The same shape and seed give the same events on every run and every machine; a
//! change of the output increases [`PERF_CORPUS_VERSION`], which names the cached store
//! files of [`PerfCorpus::file`].

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use efr_protocol::{
    CallId, CommandId, Compaction, CompactionId, CompactionTrigger, ConversationId, Event, Origin,
    Scope, Seq, ShellContext, TurnId, TurnSettings, Usage,
};
use efr_provider::{ContentBlock, Message, Role};
use efr_stdx::rng::Rng as _;
use efr_store::turn_messages::NewTurnMessages;
use efr_store::{Batch, Migrations, Store, StoreConfig, WriterHandle};
use serde_json::{Value, json};
use tempfile::TempDir;

use crate::{TestClock, TestRng, TestSupportError};

/// The version of what the generator makes. Increase it with every change of the
/// output, so a cached store file of an older generator is built again.
pub const PERF_CORPUS_VERSION: u32 = 1;

/// The provider of the saved turns.
pub const PERF_PROVIDER: &str = "openai-perf";

/// The model of the saved turns.
pub const PERF_MODEL: &str = "gpt-perf";

/// The turns after the cut that a compaction runs in: the compaction through turn `n`
/// is recorded in turn `n + COMPACTED_IN`.
const COMPACTED_IN: u32 = 6;

/// The shape of a corpus: how many conversations, how many turns each, and the turn
/// that a summary covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PerfShape {
    /// The conversations, interleaved by turn.
    pub conversations: u32,
    /// The turns of each conversation.
    pub turns: u32,
    /// The turn, from 1, that a compaction with a summary covers in each conversation,
    /// or `None` for no compaction. The compaction runs in the turn six turns later, so
    /// it has an effect only when the conversation has six more turns.
    pub compacted_through: Option<u32>,
}

impl PerfShape {
    /// A short name for file names: `c<conversations>-t<turns>-k<through>`.
    pub fn name(&self) -> String {
        let through = self.compacted_through.map_or_else(|| "none".to_owned(), |n| n.to_string());
        format!("c{}-t{}-k{through}", self.conversations, self.turns)
    }

    /// Every turn of the corpus.
    pub fn all_turns(&self) -> u64 {
        u64::from(self.conversations) * u64::from(self.turns)
    }
}

/// The sizes of the plan's ladder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PerfSize {
    /// One conversation of 20 turns.
    S,
    /// One conversation of 200 turns.
    M,
    /// One conversation of 2,000 turns, with a summary through turn 1,990.
    L,
    /// 20 conversations of the L kind, interleaved by turn.
    Xl,
}

impl PerfSize {
    /// Every size, smallest first.
    pub const ALL: [PerfSize; 4] = [PerfSize::S, PerfSize::M, PerfSize::L, PerfSize::Xl];

    /// The shape of this size.
    pub fn shape(self) -> PerfShape {
        let (conversations, turns, compacted_through) = match self {
            PerfSize::S => (1, 20, None),
            PerfSize::M => (1, 200, None),
            PerfSize::L => (1, 2_000, Some(1_990)),
            PerfSize::Xl => (20, 2_000, Some(1_990)),
        };
        PerfShape { conversations, turns, compacted_through }
    }

    /// A short name for reports: `s`, `m`, `l` or `xl`.
    pub fn name(self) -> &'static str {
        match self {
            PerfSize::S => "s",
            PerfSize::M => "m",
            PerfSize::L => "l",
            PerfSize::Xl => "xl",
        }
    }
}

/// One turn of a conversation of the corpus.
#[derive(Debug, Clone, PartialEq)]
pub struct PerfTurn {
    /// The conversation.
    pub conversation_id: ConversationId,
    /// The turn's number in its conversation, from 1.
    pub number: u32,
    /// The turn.
    pub turn_id: TurnId,
    /// The events of the turn, in order; the first turn starts with
    /// `conversation_created`, and the turn that compacts holds the
    /// `conversation_compacted` after its `turn_started`.
    pub events: Vec<Event>,
    /// The turn's exact messages, as the actor saves them at the turn's end.
    pub messages: Vec<Message>,
}

impl PerfTurn {
    /// The batch that records the turn: its events and its saved messages, which the
    /// writer saves with the turn's terminal event, the batch's last.
    pub fn batch(&self) -> Batch {
        let mut batch = Batch::new();
        for event in &self.events {
            batch = batch.event(self.conversation_id, event.clone());
        }
        let messages = self
            .messages
            .iter()
            .map(|message| match serde_json::to_value(message) {
                Ok(value) => value,
                Err(error) => unreachable!("a message of the corpus serializes: {error}"),
            })
            .collect();
        batch.turn_messages(NewTurnMessages::new(
            self.conversation_id,
            self.turn_id,
            PERF_PROVIDER,
            PERF_MODEL,
            messages,
        ))
    }
}

/// What [`PerfCorpus::write`] wrote.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PerfWritten {
    /// The turns, one batch each.
    pub turns: u64,
    /// The events.
    pub events: u64,
    /// The sequence number of the newest event.
    pub last_seq: Seq,
}

/// The state of one conversation while the corpus makes it.
#[derive(Debug, Clone)]
struct Conversation {
    id: ConversationId,
    millis: u64,
    through: Option<TurnId>,
}

/// A seeded corpus of a [`PerfShape`]: an iterator of its turns, interleaved by turn.
#[derive(Debug)]
pub struct PerfCorpus {
    shape: PerfShape,
    rng: TestRng,
    conversations: Vec<Conversation>,
    /// The next turn as an index over every turn, interleaved.
    next: u64,
    id_number: u64,
}

impl PerfCorpus {
    /// The corpus of `size` from `seed`.
    pub fn new(size: PerfSize, seed: u64) -> Self {
        PerfCorpus::of_shape(size.shape(), seed)
    }

    /// The corpus of `shape` from `seed`.
    pub fn of_shape(shape: PerfShape, seed: u64) -> Self {
        let mut corpus = PerfCorpus {
            shape,
            rng: TestRng::new(seed),
            conversations: Vec::new(),
            next: 0,
            id_number: 0,
        };
        for number in 0..shape.conversations {
            // One minute apart from 2026-10-04T12:00:00Z, the start of `TestClock`.
            let millis = 1_791_115_200_000 + u64::from(number) * 60_000;
            let id = corpus.id(millis);
            corpus.conversations.push(Conversation { id, millis, through: None });
        }
        corpus
    }

    /// The shape of this corpus.
    pub fn shape(&self) -> PerfShape {
        self.shape
    }

    /// The ids of the conversations, in the order of their first turns.
    pub fn conversation_ids(&self) -> Vec<ConversationId> {
        self.conversations.iter().map(|conversation| conversation.id).collect()
    }

    /// Appends the rest of the corpus to `writer`, one batch per turn, and moves
    /// `clock` ten seconds before each turn.
    pub async fn write(
        &mut self,
        writer: &WriterHandle,
        clock: &TestClock,
    ) -> Result<PerfWritten, TestSupportError> {
        self.write_turns(writer, clock, u64::MAX).await
    }

    /// Appends at most `turns` more turns of the corpus to `writer`, as
    /// [`write`](Self::write) does. A bench that measures at several log sizes writes
    /// the corpus in parts.
    pub async fn write_turns(
        &mut self,
        writer: &WriterHandle,
        clock: &TestClock,
        turns: u64,
    ) -> Result<PerfWritten, TestSupportError> {
        let mut written = PerfWritten::default();
        for turn in self.by_ref().take(usize::try_from(turns).unwrap_or(usize::MAX)) {
            clock.advance(Duration::from_secs(10));
            let committed = writer
                .append(turn.batch())
                .await
                .map_err(|source| TestSupportError::WriteStore { source })?;
            written.turns += 1;
            written.events += committed.events().len() as u64;
            written.last_seq = committed.last_seq();
        }
        Ok(written)
    }

    /// The store file of `shape` from `seed` below `dir`, built when it is missing. The
    /// name holds the shape, the seed, the newest migration and
    /// [`PERF_CORPUS_VERSION`]. Two tests may build the same file at once: each builds
    /// in a dir of its own and renames the finished file into place. A test that writes
    /// to the store works on a [`copy`](Self::copy).
    pub async fn file(
        shape: PerfShape,
        seed: u64,
        dir: &Path,
    ) -> Result<PathBuf, TestSupportError> {
        let name = format!(
            "perf-{}-seed{seed}-schema{}-gen{PERF_CORPUS_VERSION}.sqlite",
            shape.name(),
            Migrations::new().latest()
        );
        let target = dir.join(name);
        if target.exists() {
            return Ok(target);
        }
        fs::create_dir_all(dir)
            .map_err(|source| TestSupportError::CreateDir { path: dir.to_path_buf(), source })?;
        let build = tempfile::tempdir_in(dir)
            .map_err(|source| TestSupportError::CreateTempDir { source })?;
        let path = build.path().join("efr.sqlite");
        let clock = TestClock::new();
        let store = open_store_file(&path, &clock).await?;
        PerfCorpus::of_shape(shape, seed).write(store.writer(), &clock).await?;
        store.close().await;
        fs::rename(&path, &target)
            .map_err(|source| TestSupportError::StoreFile { path: target.clone(), source })?;
        Ok(target)
    }

    /// A copy of the store file of `shape` (built when it is missing) in a new temp dir
    /// below `dir`: the dir, removed on drop, and the database file in it.
    pub async fn copy(
        shape: PerfShape,
        seed: u64,
        dir: &Path,
    ) -> Result<(TempDir, PathBuf), TestSupportError> {
        let source = PerfCorpus::file(shape, seed, dir).await?;
        let copy = tempfile::tempdir_in(dir)
            .map_err(|source| TestSupportError::CreateTempDir { source })?;
        let path = copy.path().join("efr.sqlite");
        fs::copy(&source, &path)
            .and_then(|_| fs::File::open(&path)?.sync_all())
            .map_err(|source| TestSupportError::StoreFile { path: path.clone(), source })?;
        Ok((copy, path))
    }

    fn below(&self, n: u64) -> u64 {
        self.rng.next_u64() % n.max(1)
    }

    /// Words to about `bytes` bytes, with a line break about every 80 characters.
    fn text(&self, bytes: usize) -> String {
        const WORDS: [&str; 32] = [
            "the", "build", "fails", "because", "cargo", "cannot", "find", "crate", "in", "path",
            "src", "main", "rs", "test", "passes", "now", "check", "config", "toml", "line",
            "error", "warning", "unused", "import", "module", "function", "returns", "value",
            "after", "change", "file", "commit",
        ];
        let mut text = String::with_capacity(bytes + 16);
        let mut line = 0;
        while text.len() < bytes {
            let word = WORDS[self.below(WORDS.len() as u64) as usize];
            if line > 80 {
                text.push('\n');
                line = 0;
            } else if !text.is_empty() {
                text.push(' ');
            }
            text.push_str(word);
            line += word.len() + 1;
        }
        text
    }

    /// Base64-like bytes of about `bytes`, as an encrypted reasoning item holds.
    fn opaque(&self, bytes: usize) -> String {
        const ALPHABET: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut text = String::with_capacity(bytes);
        while text.len() < bytes {
            let mut draw = self.rng.next_u64();
            for _ in 0..10 {
                text.push(char::from(ALPHABET[(draw & 63) as usize]));
                draw >>= 6;
            }
        }
        text
    }

    fn id<T: std::str::FromStr>(&mut self, millis: u64) -> T
    where
        T::Err: std::fmt::Debug,
    {
        self.id_number += 1;
        let random = self.rng.next_u64();
        let text = format!(
            "{:08x}-{:04x}-7{:03x}-8{:03x}-{:012x}",
            millis >> 16,
            millis & 0xffff,
            random & 0xfff,
            (random >> 12) & 0xfff,
            ((random >> 24) ^ self.id_number) & 0xffff_ffff_ffff,
        );
        match text.parse() {
            Ok(id) => id,
            Err(error) => unreachable!("a UUIDv7 in its text form parses: {error:?}"),
        }
    }

    /// The Responses id of an output item, such as `rs_...` or `fc_...`.
    fn item_id(&self, prefix: &str) -> String {
        format!("{prefix}_{:016x}{:016x}", self.rng.next_u64(), self.rng.next_u64())
    }

    /// One turn, number `number` from 1, of the conversation at `index`.
    fn turn(&mut self, index: usize, number: u32) -> PerfTurn {
        let Conversation { id: conversation_id, millis, through } =
            self.conversations[index].clone();
        let millis = millis + u64::from(number) * 10_000;
        let turn_id: TurnId = self.id(millis);
        let command_id: CommandId = self.id(millis);
        let cwd = format!("/home/user/src/project{}", index % 7);
        let mut events = Vec::with_capacity(52);
        let mut messages = Vec::with_capacity(10);
        if number == 1 {
            events.push(Event::ConversationCreated {
                origin: Origin::Shell,
                tty: Some(format!("pts-{index}")),
                route: None,
            });
        }
        let prompt = self.text(300);
        events.push(Event::PromptQueued {
            turn_id,
            command_id,
            text: prompt.clone(),
            origin: Origin::Shell,
            context: Some(ShellContext::new(&cwd)),
            settings: TurnSettings::default(),
            steers: Vec::new(),
        });
        events.push(Event::TurnStarted {
            turn_id,
            cwd: PathBuf::from(&cwd),
            scope: Scope::Path(PathBuf::from(&cwd)),
            settings: None,
        });
        messages.push(Message::user(format!("<context>\ncwd: {cwd}\n</context>\n\n{prompt}")));
        let shape = self.shape;
        if let (Some(cut), Some(through_turn)) = (shape.compacted_through, through)
            && number == cut + COMPACTED_IN
        {
            events.push(Event::ConversationCompacted(self.compaction(turn_id, through_turn)));
        }
        for round in 0..4u32 {
            let answer = self.text(400);
            self.answer(turn_id, round, &answer, 3, &mut events);
            let call_id: CallId = self.id(millis);
            let provider_call = self.item_id("call");
            let command = format!("cargo test -p crate{} -- {}", self.below(40), self.text(40));
            let input = json!({ "command": command });
            events.push(Event::ToolCallStarted {
                turn_id,
                call_id,
                tool: "shell".to_owned(),
                input: input.clone(),
                freeform: false,
                manual_input: false,
                launch: None,
            });
            let output = self.text(1_000);
            let mut bytes = 0;
            for _ in 0..4 {
                let tail = self.text(150);
                bytes += tail.len() as u64;
                events.push(Event::ToolCallOutputUpdated { turn_id, call_id, tail, bytes });
            }
            events.push(Event::ToolCallCompleted {
                turn_id,
                call_id,
                output: output.clone(),
                truncated: false,
                is_error: false,
                exit_code: Some(0),
                sandbox: None,
                refusal: None,
                changes: None,
                diff: None,
                left_running: false,
            });
            let raw = vec![
                self.reasoning_item(),
                message_item(&self.item_id("msg"), &answer),
                json!({
                    "type": "function_call",
                    "id": self.item_id("fc"),
                    "call_id": provider_call,
                    "name": "shell",
                    "arguments": input.to_string(),
                    "status": "completed",
                }),
            ];
            messages.push(Message {
                role: Role::Assistant,
                content: vec![
                    ContentBlock::Text { text: answer },
                    ContentBlock::ToolCall {
                        call_id: provider_call.clone(),
                        name: "shell".to_owned(),
                        input,
                        freeform: false,
                    },
                ],
                provider_raw: Some(Value::Array(raw)),
            });
            messages.push(Message::new(
                Role::User,
                vec![ContentBlock::ToolResult { call_id: provider_call, output, is_error: false }],
            ));
        }
        let last = self.text(1_000);
        self.answer(turn_id, 4, &last, 4, &mut events);
        let raw = vec![self.reasoning_item(), message_item(&self.item_id("msg"), &last)];
        messages.push(Message::assistant(last).with_provider_raw(Value::Array(raw)));
        events.push(Event::TurnCompleted {
            turn_id,
            usage: Some(Usage::new(20_000 + u64::from(number) * 100, 900)),
            changes: None,
            context: None,
            stop: None,
            undo: None,
        });
        if shape.compacted_through == Some(number) {
            self.conversations[index].through = Some(turn_id);
        }
        PerfTurn { conversation_id, number, turn_id, events, messages }
    }

    /// The deltas and the completed event of the answer `index` of a turn.
    fn answer(
        &self,
        turn_id: TurnId,
        index: u32,
        text: &str,
        parts: usize,
        events: &mut Vec<Event>,
    ) {
        let mut offset = 0;
        for delta in chunks(text, text.len().div_ceil(parts)) {
            events.push(Event::AssistantMessageUpdated {
                turn_id,
                index,
                offset: offset as u64,
                delta: delta.to_owned(),
            });
            offset += delta.len();
        }
        events.push(Event::AssistantMessageCompleted { turn_id, index, text: text.to_owned() });
    }

    /// A Responses reasoning item with about 600 bytes of encrypted content.
    fn reasoning_item(&self) -> Value {
        json!({
            "type": "reasoning",
            "id": self.item_id("rs"),
            "summary": [],
            "encrypted_content": self.opaque(600),
        })
    }

    /// An auto compaction with a summary through `through_turn`, run in `turn_id`.
    fn compaction(&mut self, turn_id: TurnId, through_turn: TurnId) -> Compaction {
        let compaction_id: CompactionId = self.id(1_791_115_200_000);
        Compaction {
            compaction_id,
            turn_id: Some(turn_id),
            trigger: CompactionTrigger::Auto,
            focus: None,
            model: PERF_MODEL.to_owned(),
            window: 400_000,
            limit: 360_000,
            tokens_before: 361_000,
            tokens_after: 40_000,
            through_turn,
            through_message: None,
            kept_turns: COMPACTED_IN,
            pruned_outputs: 0,
            pruned_tokens: 0,
            omitted_turns: 0,
            omitted_messages: 0,
            fresh: Some(self.text(2_000)),
            summary: Some(self.text(8_000)),
            usage: Some(Usage::new(300_000, 2_000)),
        }
    }
}

impl Iterator for PerfCorpus {
    type Item = PerfTurn;

    /// The next turn, made when asked, so a large corpus never sits in memory.
    fn next(&mut self) -> Option<PerfTurn> {
        if self.next >= self.shape.all_turns() {
            return None;
        }
        let conversations = u64::from(self.shape.conversations);
        let index = usize::try_from(self.next % conversations).unwrap_or(usize::MAX);
        let number = u32::try_from(self.next / conversations + 1).unwrap_or(u32::MAX);
        self.next += 1;
        Some(self.turn(index, number))
    }
}

/// A Responses message item of the assistant with `text`.
fn message_item(id: &str, text: &str) -> Value {
    json!({
        "type": "message",
        "id": id,
        "role": "assistant",
        "status": "completed",
        "content": [{ "type": "output_text", "text": text, "annotations": [] }],
    })
}

/// A store on the database file `path`, with no backups and times from `clock`.
pub async fn open_store_file(path: &Path, clock: &TestClock) -> Result<Store, TestSupportError> {
    let parent = path.parent().unwrap_or(Path::new("."));
    let mut config = StoreConfig::in_data_dir(parent);
    config.path = path.to_path_buf();
    config.backups = None;
    Store::open(config, clock.shared())
        .await
        .map_err(|source| TestSupportError::OpenStore { source })
}

/// `text` in parts of about `size` bytes, each ending on a character boundary.
fn chunks(text: &str, size: usize) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        let mut end = size.max(1).min(rest.len());
        while !rest.is_char_boundary(end) {
            end += 1;
        }
        let (part, tail) = rest.split_at(end);
        parts.push(part);
        rest = tail;
    }
    parts
}

#[cfg(test)]
mod tests;

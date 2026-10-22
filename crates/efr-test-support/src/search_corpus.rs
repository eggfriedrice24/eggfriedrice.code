//! A seeded corpus of conversations for the search benches and the disk budget.
//!
//! It makes only text and event shapes: the events of each turn of each conversation,
//! as `efr_protocol::Event` values, and no type of `efr-store`. The caller appends
//! them to its own store. The same size and seed give the same events on every run and
//! every machine; a change of what it makes increases [`CORPUS_VERSION`], so a cache
//! of an older corpus is not used again.
//!
//! The turn shape: a prompt of about 200 bytes, three answers of about 1 KB (each
//! streamed in four deltas first) and eight `shell` commands of about 80 bytes, each
//! with an output of about 400 bytes. A tail: 2% of the commands carry a heredoc of
//! about 2 KB, 1% of the answers are about 20 KB long, and one prompt in 1,000 turns
//! is about 100 KB long. The words come from a Zipf vocabulary of 5,000 made-up words,
//! with paths, identifiers and numbers among them.

use efr_protocol::{
    CallId, CommandId, ConversationId, Event, Origin, Scope, ShellContext, TurnId, TurnSettings,
    Usage,
};
use efr_stdx::rng::Rng as _;
use serde_json::json;

use crate::TestRng;

/// The version of what the generator makes. Increase it with every change of the
/// output, so a cached corpus of an older generator is built again.
pub const CORPUS_VERSION: u32 = 1;

/// The words of the vocabulary.
const VOCABULARY: usize = 5_000;

/// The size of a corpus.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CorpusSize {
    /// 100 conversations of 10 turns: 1,000 turns.
    S,
    /// 1,000 conversations of 20 turns: 20,000 turns.
    L,
    /// 10,000 conversations of 5 turns: 50,000 turns.
    Xl,
}

impl CorpusSize {
    /// The number of conversations.
    pub fn conversations(self) -> u32 {
        match self {
            CorpusSize::S => 100,
            CorpusSize::L => 1_000,
            CorpusSize::Xl => 10_000,
        }
    }

    /// The turns of each conversation.
    pub fn turns_per_conversation(self) -> u32 {
        match self {
            CorpusSize::S => 10,
            CorpusSize::L => 20,
            CorpusSize::Xl => 5,
        }
    }

    /// Every turn of the corpus.
    pub fn turns(self) -> u32 {
        self.conversations() * self.turns_per_conversation()
    }

    /// A short name for file names and reports: `s`, `l` or `xl`.
    pub fn name(self) -> &'static str {
        match self {
            CorpusSize::S => "s",
            CorpusSize::L => "l",
            CorpusSize::Xl => "xl",
        }
    }
}

/// One conversation of the corpus.
#[derive(Debug, Clone)]
pub struct CorpusConversation {
    /// Its id, a UUIDv7 whose time part grows with the conversation's number.
    pub id: ConversationId,
    /// The events of each turn, in order. The first turn starts with
    /// `conversation_created`.
    pub turns: Vec<Vec<Event>>,
}

/// A seeded corpus of a [`CorpusSize`].
#[derive(Debug)]
pub struct SearchCorpus {
    size: CorpusSize,
    rng: TestRng,
    words: Vec<String>,
    /// The cumulative Zipf weights of `words`, scaled to `u64`.
    cumulative: Vec<u64>,
    next_conversation: u32,
    turn_number: u64,
    id_number: u64,
}

impl SearchCorpus {
    /// The corpus of `size` from `seed`.
    pub fn new(size: CorpusSize, seed: u64) -> Self {
        let rng = TestRng::new(seed);
        let words = vocabulary(&rng);
        SearchCorpus {
            size,
            rng,
            words,
            cumulative: zipf_cumulative(VOCABULARY),
            next_conversation: 0,
            turn_number: 0,
            id_number: 0,
        }
    }

    /// The size of this corpus.
    pub fn size(&self) -> CorpusSize {
        self.size
    }

    /// A word of the vocabulary by rank, from 0 (the most common). Benches build their
    /// queries from it: a common word, a rare one, a prefix.
    pub fn word(&self, rank: usize) -> &str {
        self.words.get(rank).map_or("", String::as_str)
    }

    fn below(&self, n: u64) -> u64 {
        self.rng.next_u64() % n.max(1)
    }

    fn chance(&self, percent: u64) -> bool {
        self.below(100) < percent
    }

    fn zipf_word(&self) -> &str {
        let draw = self.rng.next_u64();
        let index = self.cumulative.partition_point(|&weight| weight < draw);
        self.word(index.min(VOCABULARY - 1))
    }

    /// A path of two to four words, such as `/srv/kelbo/src/mirat.rs`.
    fn path(&self) -> String {
        let roots = ["/srv", "/etc", "~/src", "/var/lib", "/home/user"];
        let mut path = roots[self.below(roots.len() as u64) as usize].to_owned();
        for _ in 0..2 + self.below(3) {
            path.push('/');
            path.push_str(self.zipf_word());
        }
        let endings = ["", ".rs", ".toml", ".nix", ".conf", ".py"];
        path.push_str(endings[self.below(endings.len() as u64) as usize]);
        path
    }

    /// A snake_case or camelCase identifier of two or three words.
    fn identifier(&self) -> String {
        let parts: Vec<String> =
            (0..2 + self.below(2)).map(|_| self.zipf_word().to_owned()).collect();
        if self.chance(50) {
            return parts.join("_");
        }
        let mut out = parts[0].clone();
        for part in &parts[1..] {
            let mut chars = part.chars();
            if let Some(first) = chars.next() {
                out.extend(first.to_uppercase());
                out.push_str(chars.as_str());
            }
        }
        out
    }

    fn token(&self) -> String {
        match self.below(100) {
            0..=84 => self.zipf_word().to_owned(),
            85..=89 => self.path(),
            90..=94 => self.identifier(),
            _ => self.below(100_000).to_string(),
        }
    }

    /// Prose of about `bytes` bytes, in sentences, with a line break about every 80
    /// characters when `lines` holds.
    fn prose(&self, bytes: usize, lines: bool) -> String {
        let mut text = String::with_capacity(bytes + 32);
        let mut line = 0;
        let mut sentence = 0;
        while text.len() < bytes {
            let token = self.token();
            if sentence > 0 {
                if lines && line > 80 {
                    text.push('\n');
                    line = 0;
                } else {
                    text.push(' ');
                }
            }
            line += token.len() + 1;
            text.push_str(&token);
            sentence += 1;
            if sentence > 8 + self.below(8) as usize {
                text.push('.');
                sentence = 0;
                if lines && self.chance(20) {
                    text.push('\n');
                    line = 0;
                }
            }
        }
        text
    }

    /// A command line of about 80 bytes, and in 2% of commands a heredoc of about 2 KB.
    fn command(&self) -> String {
        let programs = [
            "rsync -a",
            "git log --oneline",
            "cargo build -p",
            "grep -rn",
            "ls -la",
            "docker compose up",
            "systemctl status",
            "journalctl -u",
            "nix build",
            "rg",
        ];
        let mut command = programs[self.below(programs.len() as u64) as usize].to_owned();
        while command.len() < 70 {
            command.push(' ');
            if self.chance(40) {
                command.push_str(&self.path());
            } else {
                command.push_str(&self.token());
            }
        }
        if self.chance(2) {
            command = format!("cat > {} <<'EOF'\n{}\nEOF", self.path(), self.prose(2_000, true));
        }
        command
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

    /// The events of one turn of a conversation; the first turn creates it.
    fn turn(&mut self, millis: u64, first: bool) -> Vec<Event> {
        self.turn_number += 1;
        let turn_id: TurnId = self.id(millis);
        let command_id: CommandId = self.id(millis);
        let cwd = self.path();
        let mut events = Vec::with_capacity(40);
        if first {
            events.push(Event::ConversationCreated {
                origin: Origin::Shell,
                tty: Some(format!("pts-{}", self.below(16))),
                route: None,
            });
        }
        let prompt = if self.turn_number % 1_000 == 500 {
            self.prose(100_000, true)
        } else {
            self.prose(200, false)
        };
        events.push(Event::PromptQueued {
            turn_id,
            command_id,
            text: prompt,
            origin: Origin::Shell,
            context: Some(ShellContext::new(&cwd)),
            settings: TurnSettings::default(),
            steers: Vec::new(),
        });
        events.push(Event::TurnStarted {
            turn_id,
            cwd: cwd.clone().into(),
            scope: Scope::Path(cwd.into()),
            settings: None,
        });
        let mut calls = 0;
        for index in 0..3u32 {
            let size = if self.chance(1) { 20_000 } else { 1_000 };
            let answer = self.prose(size, true);
            let chunk = answer.len().div_ceil(4);
            let mut offset = 0;
            for delta in chunks(&answer, chunk) {
                events.push(Event::AssistantMessageUpdated {
                    turn_id,
                    index,
                    offset: offset as u64,
                    delta: delta.to_owned(),
                });
                offset += delta.len();
            }
            events.push(Event::AssistantMessageCompleted { turn_id, index, text: answer });
            let commands = if index == 2 { 2 } else { 3 };
            for _ in 0..commands {
                calls += 1;
                let call_id: CallId = self.id(millis);
                events.push(Event::ToolCallStarted {
                    turn_id,
                    call_id,
                    tool: "shell".to_owned(),
                    input: json!({ "command": self.command() }),
                    freeform: false,
                    manual_input: false,
                    launch: None,
                });
                events.push(tool_call_completed(turn_id, call_id, self.prose(400, true)));
            }
        }
        debug_assert_eq!(calls, 8);
        events.push(Event::TurnCompleted {
            turn_id,
            usage: Some(Usage::new(1_000, 200)),
            changes: None,
            context: None,
            stop: None,
            undo: None,
        });
        events
    }
}

impl Iterator for SearchCorpus {
    type Item = CorpusConversation;

    /// The next conversation, made when asked, so a large corpus never sits in memory.
    fn next(&mut self) -> Option<CorpusConversation> {
        if self.next_conversation >= self.size.conversations() {
            return None;
        }
        // One minute apart from 2026-10-04T12:00:00Z, the start of `TestClock`.
        let millis = 1_791_115_200_000 + u64::from(self.next_conversation) * 60_000;
        self.next_conversation += 1;
        let id = self.id(millis);
        let turns = (0..self.size.turns_per_conversation())
            .map(|turn| self.turn(millis, turn == 0))
            .collect();
        Some(CorpusConversation { id, turns })
    }
}

/// A `tool_call_completed` of a `shell` call that ran with exit code 0.
fn tool_call_completed(turn_id: TurnId, call_id: CallId, output: String) -> Event {
    Event::ToolCallCompleted {
        turn_id,
        call_id,
        output,
        truncated: false,
        is_error: false,
        exit_code: Some(0),
        sandbox: None,
        refusal: None,
        changes: None,
        diff: None,
        left_running: false,
    }
}

/// `text` in parts of about `size` bytes, each ending on a character boundary.
fn chunks(text: &str, size: usize) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        let mut end = size.min(rest.len());
        while !rest.is_char_boundary(end) {
            end += 1;
        }
        let (part, tail) = rest.split_at(end);
        parts.push(part);
        rest = tail;
    }
    parts
}

/// Made-up words of two to four syllables, all different, in a fixed order.
fn vocabulary(rng: &TestRng) -> Vec<String> {
    const ONSETS: [&str; 16] =
        ["b", "k", "d", "f", "g", "l", "m", "n", "p", "r", "s", "t", "v", "z", "br", "st"];
    const VOWELS: [&str; 6] = ["a", "e", "i", "o", "u", "ai"];
    let mut seen = std::collections::HashSet::with_capacity(VOCABULARY);
    let mut words = Vec::with_capacity(VOCABULARY);
    while words.len() < VOCABULARY {
        let syllables = 2 + rng.next_u64() % 3;
        let mut word = String::new();
        for _ in 0..syllables {
            word.push_str(ONSETS[(rng.next_u64() % 16) as usize]);
            word.push_str(VOWELS[(rng.next_u64() % 6) as usize]);
        }
        if seen.insert(word.clone()) {
            words.push(word);
        }
    }
    words
}

/// The cumulative weights of a Zipf distribution with exponent 1 over `n` ranks,
/// scaled so the last one is `u64::MAX`.
fn zipf_cumulative(n: usize) -> Vec<u64> {
    let weights: Vec<f64> = (1..=n).map(|rank| 1.0 / rank as f64).collect();
    let total: f64 = weights.iter().sum();
    let mut sum = 0.0;
    weights
        .iter()
        .map(|weight| {
            sum += weight / total;
            (sum.min(1.0) * u64::MAX as f64) as u64
        })
        .collect()
}

#[cfg(test)]
mod tests;

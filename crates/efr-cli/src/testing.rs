//! Fakes shared by the unit tests: a daemon on a temporary socket built from
//! `efr_protocol::framing`, a context on temporary directories, fixed clocks and
//! sizes, a clock whose sleeps end when the test says, scripted keys, and a Ctrl+C
//! and a `Ctrl+\` the test triggers.
//!
//! The fake daemon cannot come from `efr-transport`: `efr-cli` may not depend on it,
//! not even for tests. Speaking the framing directly keeps the CLI honest about the
//! bytes on the wire.

use std::collections::VecDeque;
use std::future::{pending, ready};
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use efr_protocol::framing::{self, Decoder};
use efr_protocol::{
    ActionFacts, CallId, Capabilities, CatalogOrigin, CatalogStatus, ClientFrame, Compaction,
    CompactionTrigger, ConversationId, ConversationSubscribeItem, DaemonPaths, ErrorBody, Event,
    EventEnvelope, ExitFacts, ExitInfo, ExitKind, ExitRecord, ExitSource, Hello, HelloResult,
    Launch, Method, ModelInfo, ModelSource, ModelsListResult, PROTOCOL_VERSION, ProgramFact,
    RequestId, Scope, Seq, ServerFrame, TurnId, Usage,
};
use efr_stdx::env::{Env, Var};
use efr_stdx::paths::{Dirs, RootSource, RootSources};
use efr_stdx::rng::Rng;
use efr_stdx::time::{Clock, Sleep};
use efr_test_support::Wait;
use jiff::Timestamp;
use serde::Serialize;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{Notify, mpsc, watch};
use zeroize::Zeroizing;

use crate::cli::{Cli, Command};
use crate::context::{
    Browser, Context, Ending, Interrupt, KeyInput, Resize, Resume, Signals, Stop, Terminate,
};
use crate::error::CliError;
use crate::keys::{Key, KeyReader, Keys, Read};
use crate::output::Output;
use crate::quit::Quit;
use crate::settings::Settings;
use crate::terminal::{Screen, Size, TermFacts};

/// The instant every fake clock reads: 2026-10-04T12:00:00Z.
pub(crate) fn now() -> Timestamp {
    Timestamp::from_second(1_791_115_200).unwrap()
}

pub(crate) const CONVERSATION: &str = "019a9b1c-3d00-7a10-8b20-000000000001";
pub(crate) const TURN: &str = "019a9b1c-3d00-7a10-8b20-000000000002";
pub(crate) const CALL: &str = "019a9b1c-3d00-7a10-8b20-000000000003";

/// A command of several lines that starts like one from the user's test of `auto`:
/// four commands, one per line.
pub(crate) const FROM_SRC: &str =
    "cd src\nexport RUST_LOG=debug\ncargo test -p efr-cli\nunset RUST_LOG";

/// The other command of several lines from that test: two commands, which joined by a
/// space look like one with more arguments.
pub(crate) const FAILED_UNITS: &str =
    "systemctl --failed --no-pager\njournalctl -b -n 20 --no-pager";

pub(crate) fn conversation() -> ConversationId {
    CONVERSATION.parse().unwrap()
}

pub(crate) fn turn() -> TurnId {
    TURN.parse().unwrap()
}

pub(crate) fn call() -> CallId {
    CALL.parse().unwrap()
}

/// Escape bytes as `\e` and carriage returns as `\r`, so snapshots of terminal output
/// stay readable and keep the carriage returns that insta would otherwise drop.
pub(crate) fn readable(painted: &str) -> String {
    painted.replace('\x1b', "\\e").replace('\r', "\\r")
}

/// `text` without its escape sequences, so a test can find text that paint splits.
pub(crate) fn bare(text: &str) -> String {
    let mut out = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('[') => {
                for c in chars.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            }
            Some(']') => {
                while let Some(c) = chars.next() {
                    if c == '\x07' || (c == '\x1b' && chars.next_if_eq(&'\\').is_some()) {
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// A terminal's screen for the tests: the writes move a cursor over a grid of cells.
/// Carriage return, newline (as `ONLCR` makes it), the cursor moves `CSI A`, `B`, `C`
/// and `D`, and the erases `CSI J`, `K` and `2K` act; colours, modes and OSC sequences
/// draw nothing. Every character takes one cell, a row wraps at the width, and the
/// screen grows downwards without scrolling.
#[derive(Debug, Default)]
pub(crate) struct Grid {
    cols: usize,
    rows: Vec<Vec<char>>,
    row: usize,
    col: usize,
}

impl Grid {
    pub(crate) fn new(cols: u16) -> Grid {
        Grid { cols: usize::from(cols), ..Grid::default() }
    }

    pub(crate) fn write(&mut self, text: &str) {
        let mut chars = text.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '\x1b' => match chars.next() {
                    Some('[') => {
                        let mut params = String::new();
                        for c in chars.by_ref() {
                            if ('@'..='~').contains(&c) {
                                self.csi(&params, c);
                                break;
                            }
                            params.push(c);
                        }
                    }
                    Some(']') => {
                        while let Some(c) = chars.next() {
                            if c == '\x07' || (c == '\x1b' && chars.next_if_eq(&'\\').is_some()) {
                                break;
                            }
                        }
                    }
                    _ => {}
                },
                '\r' => self.col = 0,
                '\n' => {
                    self.row += 1;
                    self.col = 0;
                }
                c => {
                    if self.col >= self.cols {
                        self.row += 1;
                        self.col = 0;
                    }
                    let col = self.col;
                    let row = self.line();
                    if row.len() <= col {
                        row.resize(col + 1, ' ');
                    }
                    row[col] = c;
                    self.col += 1;
                }
            }
        }
    }

    fn line(&mut self) -> &mut Vec<char> {
        if self.rows.len() <= self.row {
            self.rows.resize(self.row + 1, Vec::new());
        }
        &mut self.rows[self.row]
    }

    fn csi(&mut self, params: &str, last: char) {
        let count = params.parse::<usize>().unwrap_or(1).max(1);
        match (last, params) {
            ('A', _) => self.row = self.row.saturating_sub(count),
            ('B', _) => self.row += count,
            ('C', _) => self.col = (self.col + count).min(self.cols.saturating_sub(1)),
            ('D', _) => self.col = self.col.saturating_sub(count),
            ('J', "" | "0") => {
                let col = self.col;
                self.line().truncate(col);
                self.rows.truncate(self.row + 1);
            }
            ('K', "" | "0") => {
                let col = self.col;
                self.line().truncate(col);
            }
            ('K', "2") => self.line().clear(),
            _ => {}
        }
    }

    /// The rows with text, without the blanks at their ends, and without empty rows at
    /// the bottom.
    pub(crate) fn lines(&self) -> Vec<String> {
        let mut lines: Vec<String> = self
            .rows
            .iter()
            .map(|row| row.iter().collect::<String>().trim_end().to_owned())
            .collect();
        while lines.last().is_some_and(String::is_empty) {
            lines.pop();
        }
        lines
    }

    /// The row and the column of the cursor.
    pub(crate) fn cursor(&self) -> (usize, usize) {
        (self.row, self.col)
    }
}

/// Parses an `efr` command line.
pub(crate) fn command(args: &[&str]) -> Command {
    let mut argv = vec!["efr"];
    argv.extend_from_slice(args);
    match <Cli as clap::Parser>::try_parse_from(argv) {
        Ok(cli) => cli.command,
        Err(error) => panic!("{args:?} does not parse: {error}"),
    }
}

/// An event of the conversation as a subscription item.
pub(crate) fn item(seq: u64, event: Event) -> ConversationSubscribeItem {
    ConversationSubscribeItem::Event(envelope(seq, event))
}

/// The record of an exit of `shell` running `line` in `/home/user/project`, with the
/// facts of `facts`.
pub(crate) fn exit_record(line: &str, facts: ExitFacts) -> ExitRecord {
    ExitRecord {
        version: ExitRecord::VERSION,
        user_messages: vec!["add the alias".to_owned()],
        action: ActionFacts {
            tool: "shell".to_owned(),
            line: line.to_owned(),
            cwd: PathBuf::from("/home/user/project"),
            scope: Scope::Machine,
            exits: Vec::new(),
            grants: Vec::new(),
            source: ExitSource::Predicted,
        },
        facts,
    }
}

/// A program word of a line, resolved to `resolved`.
pub(crate) fn program_fact(word: &str, resolved: &str) -> ProgramFact {
    ProgramFact {
        word: word.to_owned(),
        resolved: Some(PathBuf::from(resolved)),
        in_write_root: false,
        changed_this_turn: false,
    }
}

/// What a question shows about an exit of `kinds` that runs as `launch` after a yes.
pub(crate) fn exit_info(kinds: &[ExitKind], launch: Launch) -> ExitInfo {
    ExitInfo {
        kinds: kinds.to_vec(),
        grants: launch.grants().to_vec(),
        launch,
        facts: Vec::new(),
        model_reason: None,
        judged: None,
        user_only: false,
    }
}

pub(crate) fn envelope(seq: u64, event: Event) -> EventEnvelope {
    EventEnvelope { seq: Seq::new(seq), conversation_id: Some(conversation()), at: now(), event }
}

/// A model list as the daemon sends it: a built-in default with a default effort, a
/// built-in model without one, and a model from `[openai] models` whose efforts efr
/// does not know.
pub(crate) fn models() -> ModelsListResult {
    let efforts = |names: &[&str]| names.iter().map(|name| (*name).to_owned()).collect();
    ModelsListResult {
        models: vec![
            ModelInfo {
                id: "gpt-5.5".to_owned(),
                efforts: efforts(&["low", "medium", "high", "xhigh"]),
                default_effort: Some("medium".to_owned()),
                default: true,
                source: ModelSource::Builtin,
                context_window: Some(272_000),
                max_context_window: Some(872_000),
                prefer_websockets: false,
            },
            ModelInfo {
                id: "gpt-5.4".to_owned(),
                efforts: efforts(&["low", "medium", "high"]),
                default_effort: None,
                default: false,
                source: ModelSource::Builtin,
                context_window: Some(128_000),
                max_context_window: Some(128_000),
                prefer_websockets: false,
            },
            ModelInfo {
                id: "my-model".to_owned(),
                efforts: Vec::new(),
                default_effort: None,
                default: false,
                source: ModelSource::Config,
                context_window: None,
                max_context_window: None,
                prefer_websockets: false,
            },
        ],
        catalog: Some(CatalogStatus {
            origin: CatalogOrigin::Backend,
            fetched_at: Some(now() - jiff::SignedDuration::from_mins(5)),
        }),
    }
}

/// A compaction of `trigger` from 231k to `after` tokens with a summary of 3.2k.
pub(crate) fn compaction(trigger: CompactionTrigger, after: u64) -> Compaction {
    Compaction {
        compaction_id: "019a9b1c-3d00-7a10-8b20-0000000000c1".parse().unwrap(),
        turn_id: (trigger != CompactionTrigger::Manual).then(turn),
        trigger,
        focus: None,
        model: "gpt-5.5".to_owned(),
        window: 272_000,
        limit: 206_720,
        tokens_before: 231_400,
        tokens_after: after,
        through_turn: turn(),
        through_message: None,
        kept_turns: 3,
        pruned_outputs: 0,
        pruned_tokens: 0,
        omitted_turns: 0,
        omitted_messages: 0,
        fresh: Some("<fresh-context>\n</fresh-context>".to_owned()),
        summary: Some("## Task and state\n".to_owned()),
        usage: Some(Usage::new(231_400, 3_250)),
    }
}

/// A clock whose sleeps never finish, so no timeout fires and no tick comes. A sleep of
/// one frame or less finishes at once, so a frame that waits for its time goes out.
#[derive(Debug)]
pub(crate) struct StoppedClock;

impl Clock for StoppedClock {
    fn now(&self) -> Timestamp {
        now()
    }

    fn sleep(&self, duration: Duration) -> Sleep {
        if duration <= crate::follow::FRAME {
            return Box::pin(ready(()));
        }
        Box::pin(pending())
    }
}

/// A clock whose sleeps finish at once, so a timeout fires at its first chance.
#[derive(Debug)]
pub(crate) struct InstantClock;

impl Clock for InstantClock {
    fn now(&self) -> Timestamp {
        now()
    }

    fn sleep(&self, _duration: Duration) -> Sleep {
        Box::pin(ready(()))
    }
}

/// Random bytes that count up, so ids differ and repeat across runs.
#[derive(Debug, Default)]
pub(crate) struct CountingRng(AtomicU64);

impl Rng for CountingRng {
    fn fill_bytes(&self, dest: &mut [u8]) {
        for byte in dest {
            *byte = self.0.fetch_add(1, Ordering::Relaxed).to_le_bytes()[0];
        }
    }
}

/// A screen of a fixed size.
#[derive(Debug, Clone, Copy)]
pub(crate) struct FixedScreen(pub(crate) Size);

impl Screen for FixedScreen {
    fn size(&self) -> Size {
        self.0
    }
}

/// A screen whose size the test changes.
#[derive(Debug)]
pub(crate) struct ResizableScreen(pub(crate) Mutex<Size>);

impl ResizableScreen {
    pub(crate) fn set(&self, size: Size) {
        *self.0.lock().unwrap() = size;
    }
}

impl Screen for ResizableScreen {
    fn size(&self) -> Size {
        *self.0.lock().unwrap()
    }
}

/// No terminal to read keys from.
#[derive(Debug)]
pub(crate) struct NoKeys;

impl Keys for NoKeys {
    fn available(&self) -> bool {
        false
    }

    fn start(&self) -> Result<KeyReader, CliError> {
        Err(CliError::Terminal { source: io::Error::other("no terminal in this test") })
    }

    fn keep(&self) -> Result<KeyReader, CliError> {
        self.start()
    }
}

/// Keys that the test sends. Every reader gets the keys sent after it started.
#[derive(Default)]
pub(crate) struct ScriptedKeys {
    senders: Mutex<Vec<mpsc::Sender<Read>>>,
    /// Each reader's check that says it threw away unread input when it stopped.
    discards: Mutex<Vec<Arc<dyn Fn() -> bool + Send + Sync>>>,
    /// How many readers kept the typeahead ([`Keys::keep`]).
    kept: AtomicUsize,
    /// The readers act as ones with a thread: the test sends [`Read::Marked`] and
    /// [`Read::Flushed`] where the thread would.
    threaded: AtomicBool,
    started: Notify,
}

impl std::fmt::Debug for ScriptedKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScriptedKeys").field("starts", &self.starts()).finish_non_exhaustive()
    }
}

impl ScriptedKeys {
    /// Waits until a reader has started, then sends `key` to the newest one.
    pub(crate) async fn press(&self, key: u8) {
        self.send(Read::Key(Key::Byte(key))).await;
    }

    /// Presses Esc alone.
    pub(crate) async fn press_esc(&self) {
        self.send(Read::Key(Key::Esc)).await;
    }

    /// Keys whose readers act as ones with a thread ([`ScriptedKeys::marked`]).
    pub(crate) fn threaded() -> ScriptedKeys {
        let keys = ScriptedKeys::default();
        keys.threaded.store(true, Ordering::SeqCst);
        keys
    }

    /// Sends what the thread sends after a mark: every key sent before was typed
    /// before it.
    pub(crate) async fn marked(&self) {
        self.send(Read::Marked).await;
    }

    async fn send(&self, read: Read) {
        loop {
            let sender = self.senders.lock().unwrap().last().cloned();
            if let Some(sender) = sender
                && sender.send(read).await.is_ok()
            {
                return;
            }
            self.started.notified().await;
        }
    }

    /// How many readers kept the typeahead, as the input row's reader does.
    pub(crate) fn kept(&self) -> usize {
        self.kept.load(Ordering::SeqCst)
    }

    /// Types `text`, one byte at a time, into the newest reader.
    pub(crate) async fn type_bytes(&self, text: &[u8]) {
        for key in text {
            self.press(*key).await;
        }
    }

    /// Waits until the newest reader is stopped or dropped.
    pub(crate) async fn stopped(&self) {
        let sender = self.senders.lock().unwrap().last().cloned();
        if let Some(sender) = sender {
            sender.closed().await;
        }
    }

    /// How many readers were started.
    pub(crate) fn starts(&self) -> usize {
        self.senders.lock().unwrap().len()
    }

    /// True when the newest reader was stopped with its unread input thrown away.
    pub(crate) fn discarded(&self) -> bool {
        self.discards.lock().unwrap().last().is_some_and(|discarded| discarded())
    }
}

impl Keys for ScriptedKeys {
    fn available(&self) -> bool {
        true
    }

    fn start(&self) -> Result<KeyReader, CliError> {
        let (sender, keys) = mpsc::channel(8);
        self.senders.lock().unwrap().push(sender);
        let reader = if self.threaded.load(Ordering::SeqCst) {
            KeyReader::from_channel_confirming(keys)
        } else {
            KeyReader::from_channel(keys)
        };
        self.discards.lock().unwrap().push(reader.discard_flag());
        self.started.notify_one();
        Ok(reader)
    }

    fn keep(&self) -> Result<KeyReader, CliError> {
        self.kept.fetch_add(1, Ordering::SeqCst);
        self.start()
    }
}

/// A Ctrl+C that the test triggers. A trigger before anyone waits is kept.
#[derive(Debug, Default)]
pub(crate) struct TestInterrupt(Arc<Notify>);

impl TestInterrupt {
    pub(crate) fn trigger(&self) {
        self.0.notify_one();
    }
}

impl Interrupt for TestInterrupt {
    fn wait(&self) -> Stop {
        let notify = Arc::clone(&self.0);
        Box::pin(async move { notify.notified().await })
    }
}

/// Window resizes that the test triggers. A trigger before anyone waits is kept.
#[derive(Debug, Default)]
pub(crate) struct TestResize(Arc<Notify>);

impl TestResize {
    pub(crate) fn trigger(&self) {
        self.0.notify_one();
    }
}

impl Resize for TestResize {
    fn resizes(&self) -> Signals {
        notified(&self.0)
    }
}

/// A SIGTERM that the test triggers. Every future that waits for it gets it, as each
/// handler of a real signal does, and a trigger before anyone waits is kept.
#[derive(Debug, Default)]
pub(crate) struct TestTerminate(Arc<Notify>);

impl TestTerminate {
    /// The number of SIGTERM.
    pub(crate) const SIGTERM: i32 = 15;

    pub(crate) fn trigger(&self) {
        self.0.notify_waiters();
        self.0.notify_one();
    }
}

impl Terminate for TestTerminate {
    fn wait(&self) -> Ending {
        let notify = Arc::clone(&self.0);
        Box::pin(async move {
            notify.notified().await;
            TestTerminate::SIGTERM
        })
    }
}

/// Returns from a stop that the test triggers. A trigger before anyone waits is kept.
#[derive(Debug, Default)]
pub(crate) struct TestResume(Arc<Notify>);

impl TestResume {
    pub(crate) fn trigger(&self) {
        self.0.notify_one();
    }
}

impl Resume for TestResume {
    fn resumes(&self) -> Signals {
        notified(&self.0)
    }
}

/// One item for each notification of `notify`.
fn notified(notify: &Arc<Notify>) -> Signals {
    Box::pin(futures::stream::unfold(Arc::clone(notify), |notify| async move {
        notify.notified().await;
        Some(((), notify))
    }))
}

/// A `Ctrl+\` that the test triggers; it counts how many waits for it live and how many
/// presses a wait took.
#[derive(Debug, Default)]
pub(crate) struct TestQuit {
    pressed: Arc<Notify>,
    armed: Arc<AtomicUsize>,
    taken: Arc<AtomicUsize>,
}

impl TestQuit {
    /// Presses the key for the wait that lives now, or the next one.
    pub(crate) fn trigger(&self) {
        self.pressed.notify_one();
    }

    /// How many waits for the key live.
    pub(crate) fn armed(&self) -> usize {
        self.armed.load(Ordering::SeqCst)
    }

    /// Waits until `count` waits for the key live.
    pub(crate) async fn until_armed(&self, count: usize) {
        Wait::new(&format!("{count} waits for the key"))
            .until(|| self.armed() == count)
            .await
            .unwrap();
    }

    /// Waits until waits for the key have taken `count` presses in all.
    pub(crate) async fn until_taken(&self, count: usize) {
        Wait::new(&format!("{count} presses taken"))
            .until(|| self.taken.load(Ordering::SeqCst) == count)
            .await
            .unwrap();
    }
}

impl Quit for TestQuit {
    fn wait(&self) -> Stop {
        let pressed = Arc::clone(&self.pressed);
        let armed = Arc::clone(&self.armed);
        let taken = Arc::clone(&self.taken);
        Box::pin(async move {
            armed.fetch_add(1, Ordering::SeqCst);
            let _live = Live(Arc::clone(&armed));
            pressed.notified().await;
            taken.fetch_add(1, Ordering::SeqCst);
        })
    }
}

/// Counts down a [`TestQuit`] wait when it ends or is dropped.
struct Live(Arc<AtomicUsize>);

impl Drop for Live {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// A clock whose sleeps all finish when the test opens its gate; it remembers what
/// each sleep asked for.
#[derive(Debug)]
pub(crate) struct GateClock {
    gate: watch::Sender<u64>,
    requested: Mutex<Vec<Duration>>,
}

impl Default for GateClock {
    fn default() -> Self {
        GateClock { gate: watch::channel(0).0, requested: Mutex::new(Vec::new()) }
    }
}

impl GateClock {
    /// Finishes every sleep that waits now.
    pub(crate) fn open(&self) {
        self.gate.send_modify(|opened| *opened += 1);
    }

    /// Waits until a sleep of `duration` was asked for.
    pub(crate) async fn until_slept(&self, duration: Duration) {
        Wait::new(&format!("a sleep of {duration:?}"))
            .until(|| self.requested.lock().unwrap().contains(&duration))
            .await
            .unwrap();
    }
}

impl Clock for GateClock {
    fn now(&self) -> Timestamp {
        now()
    }

    fn sleep(&self, duration: Duration) -> Sleep {
        self.requested.lock().unwrap().push(duration);
        let mut gate = self.gate.subscribe();
        Box::pin(async move {
            if gate.changed().await.is_err() {
                pending::<()>().await;
            }
        })
    }
}

/// A browser that only remembers the URLs it was asked to open.
#[derive(Debug, Default)]
pub(crate) struct RecordingBrowser(pub(crate) Mutex<Vec<String>>);

impl Browser for RecordingBrowser {
    fn open(&self, url: &str) -> io::Result<()> {
        self.0.lock().unwrap().push(url.to_owned());
        Ok(())
    }
}

/// Variables and a stdin that the test gives, for a key read without a terminal.
#[derive(Debug, Default)]
pub(crate) struct FixedKeyInput {
    pub(crate) vars: Vec<(&'static str, &'static str)>,
    pub(crate) stdin: &'static str,
}

impl KeyInput for FixedKeyInput {
    fn var(&self, name: &str) -> Option<Zeroizing<String>> {
        let value = self.vars.iter().find(|(known, _)| *known == name).map(|(_, value)| value);
        value.map(|value| Zeroizing::new((*value).to_owned()))
    }

    fn stdin(&self, limit: usize) -> io::Result<Zeroizing<Vec<u8>>> {
        let bytes = self.stdin.as_bytes();
        Ok(Zeroizing::new(bytes[..bytes.len().min(limit + 1)].to_vec()))
    }
}

/// An [`Output`] that keeps what was written, and the handle to read it.
pub(crate) fn capture() -> (Output, Captured) {
    let captured = Captured::default();
    let output = Output::from_writers(
        Box::new(Buffer(Arc::clone(&captured.stdout))),
        Box::new(Buffer(Arc::clone(&captured.stderr))),
    );
    (output, captured)
}

/// What a captured [`Output`] received.
#[derive(Debug, Clone, Default)]
pub(crate) struct Captured {
    stdout: Arc<Mutex<Vec<u8>>>,
    stderr: Arc<Mutex<Vec<u8>>>,
}

impl Captured {
    pub(crate) fn stdout(&self) -> String {
        String::from_utf8(self.stdout.lock().unwrap().clone()).unwrap()
    }

    pub(crate) fn stderr(&self) -> String {
        String::from_utf8(self.stderr.lock().unwrap().clone()).unwrap()
    }
}

struct Buffer(Arc<Mutex<Vec<u8>>>);

impl Write for Buffer {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Facts for a terminal on all three streams, with 16 colours.
pub(crate) fn terminal_facts() -> TermFacts {
    TermFacts {
        no_color: false,
        term: Some("xterm-256color".to_owned()),
        colorterm: None,
        stdout_tty: true,
        stderr_tty: true,
        stdin_tty: true,
        ..TermFacts::default()
    }
}

/// Temporary efr roots, and contexts and fake daemons on them.
#[derive(Debug)]
pub(crate) struct TestEnv {
    _dir: tempfile::TempDir,
    pub(crate) dirs: Dirs,
}

impl TestEnv {
    pub(crate) fn new() -> TestEnv {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for name in ["config", "data", "state", "runtime"] {
            std::fs::create_dir(root.join(name)).unwrap();
        }
        let dirs = Dirs::new(
            root.join("config"),
            root.join("data"),
            root.join("state"),
            root.join("runtime"),
        )
        .unwrap();
        TestEnv { _dir: dir, dirs }
    }

    pub(crate) fn socket(&self) -> PathBuf {
        self.dirs.socket_path()
    }

    /// A fake daemon listening on the default socket.
    pub(crate) fn listen(&self) -> FakeDaemon {
        FakeDaemon { listener: UnixListener::bind(self.socket()).unwrap() }
    }

    /// A context on these roots: not a terminal, no keys, no environment variables,
    /// the stopped clock, and a working directory of `/home/user/project`.
    pub(crate) fn context(&self) -> Context {
        Context {
            dirs: self.dirs.clone(),
            sources: RootSources::all(RootSource::Xdg),
            env: Env::fixed(Vec::<(Var, String)>::new()),
            editor: None,
            term: TermFacts::default(),
            settings: Settings::default(),
            clock: Arc::new(StoppedClock),
            rng: Arc::new(CountingRng::default()),
            screen: Arc::new(FixedScreen(Size::default())),
            keys: Arc::new(NoKeys),
            interrupt: Arc::new(TestInterrupt::default()),
            resize: Arc::new(TestResize::default()),
            resume: Arc::new(TestResume::default()),
            terminate: Arc::new(TestTerminate::default()),
            quit: Arc::new(TestQuit::default()),
            browser: Arc::new(RecordingBrowser::default()),
            key_input: Arc::new(FixedKeyInput::default()),
            cwd: Some(PathBuf::from("/home/user/project")),
            tty: None,
            home: Some(PathBuf::from("/home/user")),
        }
    }
}

/// What the fake daemon answers to hello.
pub(crate) fn hello_result() -> HelloResult {
    HelloResult {
        daemon_id: "019a9b1c-3d00-7a10-8b20-000000000007".parse().unwrap(),
        protocol: PROTOCOL_VERSION,
        version: "0.1.0".to_owned(),
        capabilities: Capabilities { admin: Some(true), ..Capabilities::default() },
        paths: DaemonPaths::default(),
        challenge: "AAAA".to_owned(),
    }
}

/// A daemon that the test scripts frame by frame.
#[derive(Debug)]
pub(crate) struct FakeDaemon {
    listener: UnixListener,
}

impl FakeDaemon {
    /// Accepts the next connection and answers its hello.
    pub(crate) async fn accept(&self) -> Conn {
        let (stream, _) = self.listener.accept().await.unwrap();
        let mut conn =
            Conn { stream, decoder: Decoder::new(), ready: VecDeque::new(), hello: None };
        let (id, method) = conn.request().await;
        let Method::Hello(hello) = method else {
            panic!("the first request must be hello, got {}", method.name());
        };
        conn.reply(id, &hello_result()).await;
        conn.hello = Some(hello);
        conn
    }
}

/// One accepted connection.
#[derive(Debug)]
pub(crate) struct Conn {
    stream: UnixStream,
    decoder: Decoder,
    ready: VecDeque<Vec<u8>>,
    hello: Option<Hello>,
}

impl Conn {
    /// The hello the client sent.
    pub(crate) fn hello(&self) -> &Hello {
        self.hello.as_ref().unwrap()
    }

    /// The next client frame, or `None` once the client closed its side.
    pub(crate) async fn recv(&mut self) -> Option<ClientFrame> {
        loop {
            if let Some(payload) = self.ready.pop_front() {
                return Some(ClientFrame::from_json(&payload).unwrap());
            }
            let mut buf = [0_u8; 4096];
            let read = match self.stream.read(&mut buf).await {
                Ok(read) => read,
                // A client that exits with frames unread resets the connection.
                Err(error) if error.kind() == io::ErrorKind::ConnectionReset => 0,
                Err(error) => panic!("reading from the client failed: {error}"),
            };
            if read == 0 {
                self.decoder.finish().unwrap();
                return None;
            }
            self.ready.extend(self.decoder.push(&buf[..read]).unwrap());
        }
    }

    /// The next frame, which must be a request.
    pub(crate) async fn request(&mut self) -> (RequestId, Method) {
        match self.recv().await {
            Some(ClientFrame::Request { id, method }) => (id, method),
            other => panic!("expected a request, got {other:?}"),
        }
    }

    pub(crate) async fn send(&mut self, frame: &ServerFrame) {
        let bytes = framing::encode(frame).unwrap();
        self.stream.write_all(&bytes).await.unwrap();
        self.stream.flush().await.unwrap();
    }

    /// One item of request `id`.
    pub(crate) async fn item<T: Serialize>(&mut self, id: RequestId, value: &T) {
        self.send(&ServerFrame::item(id, value).unwrap()).await;
    }

    pub(crate) async fn end(&mut self, id: RequestId) {
        self.send(&ServerFrame::end(id)).await;
    }

    /// The result of a unary request: its item and its end.
    pub(crate) async fn reply<T: Serialize>(&mut self, id: RequestId, value: &T) {
        self.item(id, value).await;
        self.end(id).await;
    }

    /// Answers the next request, which must be `models.list`, with `list`.
    pub(crate) async fn answer_models(&mut self, list: &ModelsListResult) {
        let (id, method) = self.request().await;
        assert!(
            matches!(method, Method::ModelsList(_)),
            "expected models.list, got {}",
            method.name()
        );
        self.reply(id, list).await;
    }

    pub(crate) async fn fail(&mut self, id: RequestId, body: ErrorBody) {
        self.send(&ServerFrame::error(Some(id), body)).await;
    }

    /// Reads until the client closes the connection and returns the frames it sent on
    /// the way, such as the cancel of a stream it dropped.
    pub(crate) async fn until_closed(&mut self) -> Vec<ClientFrame> {
        let mut frames = Vec::new();
        while let Some(frame) = self.recv().await {
            frames.push(frame);
        }
        frames
    }
}

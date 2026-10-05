//! Test doubles: a holder whose "PTY" is a socketpair the test scripts, screens, and
//! collectors for the recording and the notices.

use std::collections::{BTreeMap, HashMap};
use std::os::fd::{BorrowedFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use efr_holder::{
    ChildStatus, HolderError, PtyHandle, PtyHolder, PtyId, PtyInfo, Signal, SignalTarget, Size,
    SpawnSpec,
};
use efr_protocol::{ConversationId, Cursor, InputWait, RowCells, ScreenSnapshot, Seq};
use efr_screen::{Screen, ScreenActor, ScreenError, ScreenEvents, ScreenHandle, ScreenSink};
use efr_stdx::StdxError;
use efr_test_support::{TestClock, TestRng};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::sync::watch;

use crate::{
    InputModes, OutputUpdate, RecordingSink, RunProgress, ScreenFactory, ShellConfig, ShellDeps,
    ShellNotice, ShellObserver, ShellSessions, TerminalModes,
};

pub(crate) const SIZE: Size = Size { cols: 80, rows: 24 };

pub(crate) fn conversation(n: u8) -> ConversationId {
    format!("01920000-0000-7000-8000-0000000000{n:02}").parse().unwrap()
}

/// A holder whose masters are one end of a socketpair; the test holds the other end
/// as a [`FakeTerminal`] and plays the shell.
#[derive(Debug)]
pub(crate) struct FakeHolder {
    state: Mutex<FakeState>,
    spawns: watch::Sender<usize>,
}

#[derive(Debug, Default)]
struct FakeState {
    specs: Vec<SpawnSpec>,
    ptys: HashMap<PtyId, FakePty>,
    terminals: Vec<Option<(PtyId, UnixStream)>>,
    signals: Vec<(PtyId, Signal, SignalTarget)>,
    resizes: Vec<(PtyId, Size)>,
    released: Vec<PtyId>,
    /// How every new child has already ended, for a shell that dies at startup.
    dead_on_arrival: Option<ChildStatus>,
}

#[derive(Debug)]
struct FakePty {
    pid: u32,
    size: Size,
    status: watch::Sender<ChildStatus>,
}

impl FakeHolder {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(FakeHolder { state: Mutex::default(), spawns: watch::Sender::new(0) })
    }

    /// The test's side of the `index`-th spawned PTY, once it is spawned.
    pub(crate) async fn terminal(&self, index: usize) -> FakeTerminal {
        let mut spawns = self.spawns.subscribe();
        spawns.wait_for(|count| *count > index).await.unwrap();
        let (pty_id, stream) = self.state.lock().unwrap().terminals[index].take().unwrap();
        stream.set_nonblocking(true).unwrap();
        FakeTerminal { pty_id, stream: tokio::net::UnixStream::from_std(stream).unwrap() }
    }

    pub(crate) fn specs(&self) -> Vec<SpawnSpec> {
        self.state.lock().unwrap().specs.clone()
    }

    pub(crate) fn signals(&self) -> Vec<(PtyId, Signal, SignalTarget)> {
        self.state.lock().unwrap().signals.clone()
    }

    pub(crate) fn resizes(&self) -> Vec<(PtyId, Size)> {
        self.state.lock().unwrap().resizes.clone()
    }

    pub(crate) fn released(&self) -> Vec<PtyId> {
        self.state.lock().unwrap().released.clone()
    }

    /// Makes every later child end with `status` as soon as it is spawned.
    pub(crate) fn die_on_arrival(&self, status: ChildStatus) {
        self.state.lock().unwrap().dead_on_arrival = Some(status);
    }

    /// Ends the child of `pty_id` with `status`.
    pub(crate) fn end(&self, pty_id: PtyId, status: ChildStatus) {
        let state = self.state.lock().unwrap();
        state.ptys[&pty_id].status.send_replace(status);
    }
}

#[async_trait]
impl PtyHolder for FakeHolder {
    async fn spawn(&self, spec: SpawnSpec) -> Result<PtyHandle, HolderError> {
        spec.validate()?;
        let (ours, theirs) = UnixStream::pair().unwrap();
        let mut state = self.state.lock().unwrap();
        let pid = 1000 + u32::try_from(state.specs.len()).unwrap();
        let status = state.dead_on_arrival.unwrap_or(ChildStatus::Running);
        state.ptys.insert(
            spec.pty_id,
            FakePty { pid, size: spec.size, status: watch::Sender::new(status) },
        );
        state.terminals.push(Some((spec.pty_id, theirs)));
        let pty_id = spec.pty_id;
        state.specs.push(spec);
        let count = state.specs.len();
        drop(state);
        self.spawns.send_replace(count);
        Ok(PtyHandle { master: OwnedFd::from(ours), child_pid: pid, pty_id })
    }

    async fn resize(&self, pty_id: PtyId, size: Size) -> Result<(), HolderError> {
        let mut state = self.state.lock().unwrap();
        state.ptys.get_mut(&pty_id).ok_or(HolderError::NotFound { pty_id })?.size = size;
        state.resizes.push((pty_id, size));
        Ok(())
    }

    async fn signal(
        &self,
        pty_id: PtyId,
        signal: Signal,
        target: SignalTarget,
    ) -> Result<(), HolderError> {
        let mut state = self.state.lock().unwrap();
        let pty = state.ptys.get(&pty_id).ok_or(HolderError::NotFound { pty_id })?;
        if !pty.status.borrow().is_running() {
            return Err(HolderError::Exited { pty_id });
        }
        // A shell dies of these when they reach it; SIGINT only reaches its command.
        if matches!(signal, Signal::Hangup | Signal::Kill) && target == SignalTarget::Child {
            let number = if signal == Signal::Kill { 9 } else { 1 };
            pty.status.send_replace(ChildStatus::Signaled { signal: number });
        }
        state.signals.push((pty_id, signal, target));
        Ok(())
    }

    async fn list(&self) -> Result<Vec<PtyInfo>, HolderError> {
        let state = self.state.lock().unwrap();
        Ok(state
            .ptys
            .iter()
            .map(|(&pty_id, pty)| PtyInfo {
                pty_id,
                child_pid: pty.pid,
                size: pty.size,
                status: *pty.status.borrow(),
            })
            .collect())
    }

    async fn wait(&self, pty_id: PtyId) -> Result<ChildStatus, HolderError> {
        let mut status = {
            let state = self.state.lock().unwrap();
            state.ptys.get(&pty_id).ok_or(HolderError::NotFound { pty_id })?.status.subscribe()
        };
        match status.wait_for(|status| !status.is_running()).await {
            Ok(status) => Ok(*status),
            Err(_) => Err(HolderError::NotFound { pty_id }),
        }
    }

    async fn release(&self, pty_id: PtyId) -> Result<(), HolderError> {
        let mut state = self.state.lock().unwrap();
        state.ptys.remove(&pty_id).ok_or(HolderError::NotFound { pty_id })?;
        state.released.push(pty_id);
        Ok(())
    }
}

/// The test's end of a fake PTY: it reads what the session types and writes what the
/// "shell" prints.
#[derive(Debug)]
pub(crate) struct FakeTerminal {
    pub(crate) pty_id: PtyId,
    stream: tokio::net::UnixStream,
}

impl FakeTerminal {
    /// Prints `bytes` as the shell.
    pub(crate) async fn print(&mut self, bytes: &[u8]) {
        self.stream.write_all(bytes).await.unwrap();
    }

    /// Prints a marked primary prompt, which makes the session ready.
    pub(crate) async fn prompt(&mut self) {
        self.print(b"\x1b]133;A;cl=line\x07% \x1b]133;B\x07").await;
    }

    /// Reads what the session typed until it contains `needle`, and returns it all.
    pub(crate) async fn typed_until(&mut self, needle: &[u8]) -> Vec<u8> {
        let mut seen = Vec::new();
        let mut buf = [0; 4096];
        while !seen.windows(needle.len()).any(|window| window == needle) {
            let n = self.stream.read(&mut buf).await.unwrap();
            assert!(n > 0, "the session closed before typing {needle:?}; typed {seen:?}");
            seen.extend_from_slice(&buf[..n]);
        }
        seen
    }

    /// Reads one typed line, up to and including the Enter that ends it.
    pub(crate) async fn typed_line(&mut self) -> Vec<u8> {
        self.typed_until(b"\r").await
    }

    /// Plays a whole marked command: the echo, `C`, the output, `D`, the next prompt.
    pub(crate) async fn run(&mut self, output: &[u8], status: i32) {
        let mut bytes = b"\r\n\x1b]133;C\x07".to_vec();
        bytes.extend_from_slice(output);
        bytes.extend_from_slice(format!("\x1b]133;D;{status}\x07").as_bytes());
        self.print(&bytes).await;
        self.prompt().await;
    }
}

/// Screens over vt100, the backend of every test.
#[derive(Debug)]
pub(crate) struct Vt100Screens;

impl ScreenFactory for Vt100Screens {
    fn spawn(&self, name: &str, size: Size) -> Result<(ScreenHandle, ScreenEvents), ScreenError> {
        ScreenActor::spawn(name, efr_screen_vt100::factory(size), size)
    }
}

/// Screens over vt100 that remember every screen they started (its name, its size and
/// a handle), so a test sees which capture screens a run used and that they stopped.
#[derive(Debug, Default)]
pub(crate) struct CountingScreens {
    spawned: Mutex<Vec<(String, Size, ScreenHandle)>>,
}

impl CountingScreens {
    /// The capture screens started so far, oldest first.
    pub(crate) fn captures(&self) -> Vec<(Size, ScreenHandle)> {
        let spawned = self.spawned.lock().unwrap();
        spawned
            .iter()
            .filter(|(name, ..)| name.starts_with("replay-"))
            .map(|(_, size, handle)| (*size, handle.clone()))
            .collect()
    }
}

impl ScreenFactory for CountingScreens {
    fn spawn(&self, name: &str, size: Size) -> Result<(ScreenHandle, ScreenEvents), ScreenError> {
        let (handle, events) = ScreenActor::spawn(name, efr_screen_vt100::factory(size), size)?;
        self.spawned.lock().unwrap().push((name.to_owned(), size, handle.clone()));
        Ok((handle, events))
    }
}

/// A factory that cannot start a screen, as when the system refuses a thread.
#[derive(Debug)]
pub(crate) struct NoScreens;

impl ScreenFactory for NoScreens {
    fn spawn(&self, name: &str, _size: Size) -> Result<(ScreenHandle, ScreenEvents), ScreenError> {
        let source = StdxError::SpawnThread {
            name: name.to_owned(),
            source: std::io::Error::from(std::io::ErrorKind::OutOfMemory),
        };
        Err(ScreenError::Spawn { name: name.to_owned(), source })
    }
}

/// Screens whose backend panics on the first byte, which stops the actor.
#[derive(Debug)]
pub(crate) struct DyingScreens;

impl ScreenFactory for DyingScreens {
    fn spawn(&self, name: &str, size: Size) -> Result<(ScreenHandle, ScreenEvents), ScreenError> {
        ScreenActor::spawn(name, move || DyingScreen { size }, size)
    }
}

struct DyingScreen {
    size: Size,
}

impl Screen for DyingScreen {
    fn feed(&mut self, _bytes: &[u8], _sink: &mut dyn ScreenSink) {
        panic!("a backend bug");
    }

    fn resize(&mut self, cols: u16, rows: u16, _sink: &mut dyn ScreenSink) {
        self.size = Size { cols, rows };
    }

    fn snapshot(&mut self, _scrollback_rows: usize) -> ScreenSnapshot {
        ScreenSnapshot { size: self.size, ..ScreenSnapshot::default() }
    }

    fn row(&self, _index: usize) -> RowCells {
        RowCells::default()
    }

    fn cursor(&self) -> Cursor {
        Cursor::default()
    }

    fn title(&self) -> Option<&str> {
        None
    }

    fn pwd(&self) -> Option<&str> {
        None
    }
}

/// Screens that answer a primary device attributes query (`ESC [ c`), which vt100
/// never does, to test the reply path.
#[derive(Debug)]
pub(crate) struct AnsweringScreens;

impl ScreenFactory for AnsweringScreens {
    fn spawn(&self, name: &str, size: Size) -> Result<(ScreenHandle, ScreenEvents), ScreenError> {
        ScreenActor::spawn(name, move || AnsweringScreen { size }, size)
    }
}

struct AnsweringScreen {
    size: Size,
}

impl Screen for AnsweringScreen {
    fn feed(&mut self, bytes: &[u8], sink: &mut dyn ScreenSink) {
        if bytes.windows(3).any(|window| window == b"\x1b[c") {
            sink.pty_reply(b"\x1b[?62c");
        }
    }

    fn resize(&mut self, cols: u16, rows: u16, _sink: &mut dyn ScreenSink) {
        self.size = Size { cols, rows };
    }

    fn snapshot(&mut self, _scrollback_rows: usize) -> ScreenSnapshot {
        ScreenSnapshot { size: self.size, ..ScreenSnapshot::default() }
    }

    fn row(&self, _index: usize) -> RowCells {
        RowCells::default()
    }

    fn cursor(&self) -> Cursor {
        Cursor::default()
    }

    fn title(&self) -> Option<&str> {
        None
    }

    fn pwd(&self) -> Option<&str> {
        None
    }
}

/// Keeps every recorded chunk, and can hold one back as a slow store would.
#[derive(Debug, Default)]
pub(crate) struct Recorded {
    chunks: Mutex<Vec<(PtyId, Seq, Bytes)>>,
    gate: watch::Sender<Gate>,
}

/// Whether [`Recorded`] holds a chunk back.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
enum Gate {
    #[default]
    Open,
    /// The next chunk that contains these bytes is held.
    Armed(Vec<u8>),
    Holding,
}

impl Recorded {
    /// Every byte recorded for `pty_id`, checking that the chunks are back to back.
    pub(crate) fn stream(&self, pty_id: PtyId) -> Vec<u8> {
        let mut stream = Vec::new();
        for (id, start, bytes) in self.chunks.lock().unwrap().iter() {
            if *id == pty_id {
                assert_eq!(start.get(), stream.len() as u64, "chunks must be back to back");
                stream.extend_from_slice(bytes);
            }
        }
        stream
    }

    /// Holds the next chunk that contains `bytes` until [`release`](Self::release). The
    /// reader waits for the recording, so the session's actor does not see that chunk
    /// either, as with a store that is slow right then.
    pub(crate) fn hold_at(&self, bytes: &[u8]) {
        self.gate.send_replace(Gate::Armed(bytes.to_vec()));
    }

    /// Waits until a chunk is held.
    pub(crate) async fn holding(&self) {
        self.gate.subscribe().wait_for(|gate| *gate == Gate::Holding).await.unwrap();
    }

    /// Lets a held chunk go on, and holds no other.
    pub(crate) fn release(&self) {
        self.gate.send_replace(Gate::Open);
    }
}

#[async_trait]
impl RecordingSink for Recorded {
    async fn record(&self, pty_id: PtyId, start: Seq, bytes: Bytes) {
        self.chunks.lock().unwrap().push((pty_id, start, bytes.clone()));
        let hold = self.gate.send_if_modified(|gate| match gate {
            Gate::Armed(wanted) if bytes.windows(wanted.len()).any(|part| part == wanted) => {
                *gate = Gate::Holding;
                true
            }
            _ => false,
        });
        if hold {
            let mut gate = self.gate.subscribe();
            let _ = gate.wait_for(|gate| *gate != Gate::Holding).await;
        }
    }
}

/// Keeps every notice and lets a test wait for one.
#[derive(Debug)]
pub(crate) struct Notices {
    seen: watch::Sender<Vec<ShellNotice>>,
}

impl Notices {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Notices { seen: watch::Sender::new(Vec::new()) })
    }

    pub(crate) fn all(&self) -> Vec<ShellNotice> {
        self.seen.borrow().clone()
    }

    /// Waits until some notice matches.
    pub(crate) async fn wait_for(&self, matches: impl Fn(&ShellNotice) -> bool) {
        let mut seen = self.seen.subscribe();
        seen.wait_for(|notices| notices.iter().any(&matches)).await.unwrap();
    }
}

impl ShellObserver for Notices {
    fn notice(&self, notice: ShellNotice) {
        self.seen.send_modify(|notices| notices.push(notice));
    }
}

/// Terminal modes that the test sets, for a fake PTY that has none: a cooked terminal
/// (echo and line input on) until the test says otherwise.
#[derive(Debug)]
pub(crate) struct FakeModes {
    modes: Mutex<Option<InputModes>>,
    foreground: Mutex<u32>,
    reads: AtomicUsize,
}

/// The process group of a command's job in [`FakeModes`], never a fake shell's pid.
pub(crate) const JOB: u32 = 4242;

/// The process group of another job, such as a command that a precmd hook starts once
/// the command's job has ended; never a fake shell's pid either.
pub(crate) const OTHER_JOB: u32 = 4343;

impl Default for FakeModes {
    fn default() -> Self {
        FakeModes {
            modes: Mutex::new(Some(InputModes::new(true, true))),
            foreground: Mutex::new(JOB),
            reads: AtomicUsize::new(0),
        }
    }
}

impl FakeModes {
    /// Sets the modes; `None` makes every read fail, as on a socket.
    pub(crate) fn set(&self, modes: Option<InputModes>) {
        *self.modes.lock().unwrap() = modes;
    }

    /// Puts process group `group` in the foreground; [`JOB`] until then.
    pub(crate) fn set_foreground(&self, group: u32) {
        *self.foreground.lock().unwrap() = group;
    }

    /// How many times the modes were read.
    pub(crate) fn reads(&self) -> usize {
        self.reads.load(Ordering::SeqCst)
    }
}

impl TerminalModes for FakeModes {
    fn read(&self, _master: BorrowedFd<'_>) -> std::io::Result<InputModes> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        self.modes.lock().unwrap().ok_or_else(|| std::io::Error::from_raw_os_error(25))
    }

    fn foreground(&self, _master: BorrowedFd<'_>) -> std::io::Result<u32> {
        Ok(*self.foreground.lock().unwrap())
    }
}

/// A run listener that keeps the output and every change of the input wait, and says
/// whether someone can answer hidden input.
pub(crate) struct Listener {
    output: watch::Sender<OutputUpdate>,
    inputs: watch::Sender<Vec<InputWait>>,
    can_answer: Arc<AtomicBool>,
}

/// The test's side of a [`Listener`].
pub(crate) struct Heard {
    pub(crate) output: watch::Receiver<OutputUpdate>,
    pub(crate) inputs: watch::Receiver<Vec<InputWait>>,
    /// What the listener answers when it is asked whether someone can answer.
    pub(crate) can_answer: Arc<AtomicBool>,
}

impl Heard {
    /// Waits until the listener has heard exactly the changes `expected`.
    pub(crate) async fn inputs(&mut self, expected: &[InputWait]) {
        self.inputs.wait_for(|inputs| inputs.as_slice() == expected).await.unwrap();
    }

    /// Waits until the output's tail contains `text`.
    pub(crate) async fn output(&mut self, text: &str) {
        self.output.wait_for(|update| update.tail.contains(text)).await.unwrap();
    }
}

/// A listener that says `can_answer`, and the test's side of it.
pub(crate) fn listener(can_answer: bool) -> (Listener, Heard) {
    let (output, output_seen) = watch::channel(OutputUpdate::default());
    let (inputs, inputs_seen) = watch::channel(Vec::new());
    let can_answer = Arc::new(AtomicBool::new(can_answer));
    let heard =
        Heard { output: output_seen, inputs: inputs_seen, can_answer: Arc::clone(&can_answer) };
    (Listener { output, inputs, can_answer }, heard)
}

impl RunProgress for Listener {
    fn update(&mut self, update: &OutputUpdate) {
        self.output.send_replace(update.clone());
    }

    fn input_changed(&mut self, wait: InputWait) {
        self.inputs.send_modify(|inputs| inputs.push(wait));
    }

    fn can_answer_hidden(&mut self) -> bool {
        self.can_answer.load(Ordering::SeqCst)
    }
}

/// A manager over a [`FakeHolder`] with a manual clock.
pub(crate) struct Harness {
    pub(crate) sessions: ShellSessions,
    pub(crate) holder: Arc<FakeHolder>,
    pub(crate) clock: TestClock,
    pub(crate) recorded: Arc<Recorded>,
    pub(crate) notices: Arc<Notices>,
    pub(crate) modes: Arc<FakeModes>,
    pub(crate) dir: tempfile::TempDir,
}

impl Harness {
    /// A harness whose shell is `program` (a zsh gets the integration).
    pub(crate) fn new(program: &str) -> Self {
        Harness::with(program, Arc::new(Vt100Screens))
    }

    pub(crate) fn with(program: &str, screens: Arc<dyn ScreenFactory>) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let holder = FakeHolder::new();
        let clock = TestClock::new();
        let recorded = Arc::new(Recorded::default());
        let notices = Notices::new();
        let modes = Arc::new(FakeModes::default());
        let mut config = ShellConfig::new(
            dir.path().join("zsh"),
            BTreeMap::from([("PATH".to_owned(), "/usr/bin:/bin".to_owned())]),
        );
        config.program = Some(PathBuf::from(program));
        config.size = SIZE;
        config.startup_timeout = Duration::from_secs(10);
        let deps = ShellDeps::new(
            Arc::clone(&holder) as Arc<dyn PtyHolder>,
            screens,
            clock.shared(),
            Arc::new(TestRng::new(1)),
        )
        .with_recording(Arc::clone(&recorded) as Arc<dyn RecordingSink>)
        .with_observer(Arc::clone(&notices) as Arc<dyn ShellObserver>)
        .with_modes(Arc::clone(&modes) as Arc<dyn TerminalModes>);
        let sessions = ShellSessions::new(config, deps).unwrap();
        Harness { sessions, holder, clock, recorded, notices, modes, dir }
    }
}

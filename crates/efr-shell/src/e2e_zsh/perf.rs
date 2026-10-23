//! The harness of the hidden shell's benches: a manager over a real zsh on a real PTY,
//! as [`Zsh`](super::Zsh) builds it, but with a recording sink that only counts the
//! reader's chunks and a screen factory that counts the screens it starts. A bench
//! needs no `EFR_TEST_ZSH`: it is ignored and runs only when asked, so it skips with a
//! report line only when this machine has no zsh.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use bytes::Bytes;
use efr_holder::Size;
use efr_protocol::{ConversationId, PtyId, Seq};
use efr_pty::LocalPtyHolder;
use efr_screen::{ScreenActor, ScreenError, ScreenEvents, ScreenHandle};
use efr_test_support::{TestClock, TestRng};

use crate::{RecordingSink, ScreenFactory, ShellConfig, ShellDeps, ShellSessions};

/// Counts the chunks that the PTY reader hands on: one `record` per chunk, which is
/// one `Msg::Chunk` to the session and one feed of the screen.
#[derive(Debug, Default)]
pub(crate) struct ChunkCount {
    pub(crate) chunks: AtomicU64,
    pub(crate) bytes: AtomicU64,
    pub(crate) largest: AtomicU64,
}

impl ChunkCount {
    /// The chunks and the bytes so far.
    pub(crate) fn now(&self) -> (u64, u64) {
        (self.chunks.load(Ordering::SeqCst), self.bytes.load(Ordering::SeqCst))
    }
}

#[async_trait]
impl RecordingSink for ChunkCount {
    async fn record(&self, _pty_id: PtyId, _start: Seq, bytes: Bytes) {
        let len = bytes.len() as u64;
        self.chunks.fetch_add(1, Ordering::SeqCst);
        self.bytes.fetch_add(len, Ordering::SeqCst);
        self.largest.fetch_max(len, Ordering::SeqCst);
    }
}

/// Screens over vt100 that count the screens they start, by the kind of their name:
/// the shell's own screen, the capture screens (`replay-`) and the live tails
/// (`tail-`).
#[derive(Debug, Default)]
pub(crate) struct ScreenCount {
    pub(crate) replays: AtomicU64,
    pub(crate) others: AtomicU64,
}

impl ScreenFactory for ScreenCount {
    fn spawn(&self, name: &str, size: Size) -> Result<(ScreenHandle, ScreenEvents), ScreenError> {
        let counter = if name.starts_with("replay-") { &self.replays } else { &self.others };
        counter.fetch_add(1, Ordering::SeqCst);
        ScreenActor::spawn(name, efr_screen::vt100::factory(size), size)
    }
}

/// A manager over a real zsh in a throwaway home below the target dir.
pub(crate) struct PerfShell {
    pub(crate) sessions: ShellSessions,
    pub(crate) chunks: Arc<ChunkCount>,
    pub(crate) screens: Arc<ScreenCount>,
    pub(crate) conversation: ConversationId,
    pub(crate) root: tempfile::TempDir,
}

/// The zsh of this machine, if it has one where the hidden shell looks.
pub(crate) fn zsh() -> Option<PathBuf> {
    ["/usr/bin/zsh", "/bin/zsh"].into_iter().map(PathBuf::from).find(|path| path.is_file())
}

/// The target dir's `tmp` dir, from the path of this test binary
/// (`<target>/<profile>/deps/<binary>`): big fixtures go to the disk, not to `/tmp`,
/// which is a RAM-backed tmpfs here.
pub(crate) fn target_tmp() -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    let target = exe.ancestors().nth(3).unwrap_or(Path::new("."));
    target.join("tmp")
}

impl PerfShell {
    /// The harness, or `None` when this machine has no zsh.
    pub(crate) fn start() -> Option<PerfShell> {
        let zsh = zsh()?;
        let parent = target_tmp().join("efr-shell-bench");
        std::fs::create_dir_all(&parent).unwrap();
        let root = tempfile::tempdir_in(&parent).unwrap();
        let home = root.path().join("home");
        std::fs::create_dir(&home).unwrap();
        for file in [".zshenv", ".zprofile", ".zshrc", ".zlogin"] {
            std::fs::write(home.join(file), "").unwrap();
        }
        let base_env = BTreeMap::from([
            ("HOME".to_owned(), home.to_string_lossy().into_owned()),
            ("PATH".to_owned(), "/usr/bin:/bin".to_owned()),
            ("LANG".to_owned(), "C.UTF-8".to_owned()),
            ("SHELL".to_owned(), zsh.to_string_lossy().into_owned()),
        ]);
        let mut config = ShellConfig::new(root.path().join("zsh"), base_env);
        config.program = Some(zsh);
        config.login = false;
        let clock = TestClock::new();
        let chunks = Arc::new(ChunkCount::default());
        let screens = Arc::new(ScreenCount::default());
        let deps = ShellDeps::new(
            Arc::new(LocalPtyHolder::new()),
            Arc::clone(&screens) as Arc<dyn ScreenFactory>,
            clock.shared(),
            Arc::new(TestRng::new(42)),
        )
        .with_recording(Arc::clone(&chunks) as Arc<dyn RecordingSink>);
        let sessions = ShellSessions::new(config, deps).unwrap();
        let conversation = "01920000-0000-7000-8000-00000000be0c".parse().unwrap();
        Some(PerfShell { sessions, chunks, screens, conversation, root })
    }

    /// The throwaway tree, where the shell starts.
    pub(crate) fn dir(&self) -> &Path {
        self.root.path()
    }
}

#[cfg(test)]
mod tests;

//! Test fixtures: a throwaway tree with a home and a working directory, the context
//! of a call in it, and a fake shell.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use efr_protocol::{ConversationId, InputWait};
use efr_scope::Home;
use efr_shell::{CommandResult, CommandRunner, OutputUpdate, RunProgress, RunRequest, ShellError};
use efr_test_support::TestClock;

use crate::{CallIds, MemoryJournal, ToolContext, WriteJournal};

pub(crate) fn ids() -> CallIds {
    CallIds {
        conversation_id: "01920000-0000-7000-8000-000000000001".parse().unwrap(),
        turn_id: "01920000-0000-7000-8000-000000000002".parse().unwrap(),
        call_id: "01920000-0000-7000-8000-000000000003".parse().unwrap(),
    }
}

/// `root/home` and `root/work` in a temporary directory, with symbolic links
/// resolved, so a path built from them is real.
pub(crate) struct Fixture {
    /// Kept for its drop, which removes the tree.
    _dir: tempfile::TempDir,
    root: PathBuf,
    pub(crate) journal: Arc<MemoryJournal>,
}

impl Fixture {
    pub(crate) fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        std::fs::create_dir(root.join("home")).unwrap();
        std::fs::create_dir(root.join("work")).unwrap();
        Fixture { _dir: dir, root, journal: Arc::new(MemoryJournal::new()) }
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    pub(crate) fn home(&self) -> PathBuf {
        self.root.join("home")
    }

    pub(crate) fn cwd(&self) -> PathBuf {
        self.root.join("work")
    }

    /// The context of a call whose user sits in `work`.
    pub(crate) fn context(&self) -> ToolContext {
        ToolContext::new(
            ids(),
            self.cwd(),
            self.root.join("scratch"),
            Home::new(self.home()).unwrap(),
            TestClock::new().shared(),
            Arc::clone(&self.journal) as Arc<dyn WriteJournal>,
        )
    }
}

/// A shell that answers every run with the next scripted outcome, reports scripted
/// progress and input waits first, and remembers the requests and what the listener
/// said when it was asked whether someone can answer hidden input.
#[derive(Debug, Default)]
pub(crate) struct FakeRunner {
    outcomes: Mutex<Vec<Result<CommandResult, ShellError>>>,
    pub(crate) requests: Mutex<Vec<(ConversationId, RunRequest)>>,
    pub(crate) progress: Vec<OutputUpdate>,
    pub(crate) inputs: Vec<InputWait>,
    pub(crate) answerable: Mutex<Vec<bool>>,
}

impl FakeRunner {
    pub(crate) fn answering(outcome: Result<CommandResult, ShellError>) -> Arc<Self> {
        Arc::new(FakeRunner { outcomes: Mutex::new(vec![outcome]), ..FakeRunner::default() })
    }

    pub(crate) fn with_progress(outcome: CommandResult, progress: Vec<OutputUpdate>) -> Arc<Self> {
        Arc::new(FakeRunner {
            outcomes: Mutex::new(vec![Ok(outcome)]),
            progress,
            ..FakeRunner::default()
        })
    }

    pub(crate) fn with_inputs(outcome: CommandResult, inputs: Vec<InputWait>) -> Arc<Self> {
        Arc::new(FakeRunner {
            outcomes: Mutex::new(vec![Ok(outcome)]),
            inputs,
            ..FakeRunner::default()
        })
    }

    pub(crate) fn last_request(&self) -> RunRequest {
        self.requests.lock().unwrap().last().unwrap().1.clone()
    }
}

#[async_trait]
impl CommandRunner for FakeRunner {
    async fn run_command(
        &self,
        conversation: ConversationId,
        request: RunRequest,
        progress: &mut dyn RunProgress,
    ) -> Result<CommandResult, ShellError> {
        self.requests.lock().unwrap().push((conversation, request));
        for update in &self.progress {
            progress.update(update);
        }
        for wait in &self.inputs {
            progress.input_changed(*wait);
            if *wait == InputWait::Hidden {
                let answerable = progress.can_answer_hidden();
                self.answerable.lock().unwrap().push(answerable);
            }
        }
        self.outcomes.lock().unwrap().remove(0)
    }
}

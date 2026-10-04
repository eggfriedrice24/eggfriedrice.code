//! Where a tool call happens.

use std::path::PathBuf;
use std::sync::Arc;

use efr_protocol::{CallId, ConversationId, Origin, Scope, TurnId};
use efr_scope::Home;
use efr_stdx::time::Clock;

use crate::WriteJournal;

/// The ids of one tool call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CallIds {
    /// The conversation; it also names the hidden shell a command runs in.
    pub conversation_id: ConversationId,
    /// The turn.
    pub turn_id: TurnId,
    /// The call.
    pub call_id: CallId,
}

/// Everything a tool call may know about where it runs. The conversation builds one
/// per call; a tool gets it by value.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ToolContext {
    /// The call's ids.
    pub ids: CallIds,
    /// The user's working directory when the prompt was sent: relative paths in a
    /// call resolve against it, and a new hidden shell starts in it.
    pub cwd: PathBuf,
    /// Where the conversation's hidden shell is now, when one runs: a relative path in
    /// a command resolves against it. A new shell starts in `cwd`.
    pub shell_cwd: Option<PathBuf>,
    /// The conversation's `$SCRATCH` directory.
    pub scratch: PathBuf,
    /// The turn's scope.
    pub scope: Scope,
    /// The surface the turn came from.
    pub origin: Origin,
    /// The user's home directory: `~` in a path means it, and a home reached through
    /// a symbolic link (`/home` to `/var/home`) does not count as a link.
    pub home: Home,
    /// The clock, for tools that wait.
    pub clock: Arc<dyn Clock>,
    /// Where a tool that changes a file records the original first.
    pub journal: Arc<dyn WriteJournal>,
}

impl ToolContext {
    /// A context for the call `ids`, in the machine scope from the shell origin until
    /// [`with_scope`](Self::with_scope) and [`with_origin`](Self::with_origin) say
    /// otherwise.
    pub fn new(
        ids: CallIds,
        cwd: impl Into<PathBuf>,
        scratch: impl Into<PathBuf>,
        home: Home,
        clock: Arc<dyn Clock>,
        journal: Arc<dyn WriteJournal>,
    ) -> Self {
        ToolContext {
            ids,
            cwd: cwd.into(),
            shell_cwd: None,
            scratch: scratch.into(),
            scope: Scope::Machine,
            origin: Origin::Shell,
            home,
            clock,
            journal,
        }
    }

    /// Sets the turn's scope.
    #[must_use]
    pub fn with_scope(mut self, scope: Scope) -> Self {
        self.scope = scope;
        self
    }

    /// Sets the surface the turn came from.
    #[must_use]
    pub fn with_origin(mut self, origin: Origin) -> Self {
        self.origin = origin;
        self
    }

    /// Sets where the conversation's hidden shell is now.
    #[must_use]
    pub fn with_shell_cwd(mut self, shell_cwd: Option<PathBuf>) -> Self {
        self.shell_cwd = shell_cwd;
        self
    }

    /// The directory a command of this call runs in: the hidden shell's, or the user's
    /// when no shell runs yet.
    pub fn command_dir(&self) -> &std::path::Path {
        self.shell_cwd.as_deref().unwrap_or(&self.cwd)
    }
}

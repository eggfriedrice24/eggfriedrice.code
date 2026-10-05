//! The one public error type of the crate.

use std::io;
use std::path::PathBuf;

use efr_holder::{ChildStatus, HolderError};
use efr_protocol::ConversationId;
use efr_screen::ScreenError;
use efr_stdx::StdxError;

/// Every way a hidden shell operation can fail.
///
/// No variant carries a command line or an environment value: both can hold secrets,
/// and errors end up in logs.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ShellError {
    /// No shell program was configured and `zsh` is not on the `PATH` of the
    /// environment the daemon passed in.
    #[error("{program} was not found on the PATH of the shell environment")]
    ProgramNotFound {
        /// The program that was looked for.
        program: String,
        /// Why the search failed.
        #[source]
        source: which::Error,
    },

    /// The integration files could not be written to their directory.
    #[error("could not write the zsh integration files to {}", .dir.display())]
    Integration {
        /// The directory of the files.
        dir: PathBuf,
        /// The error from the file write.
        #[source]
        source: StdxError,
    },

    /// The holder could not start the conversation's shell.
    #[error("could not start the hidden shell of conversation {conversation}")]
    Spawn {
        /// The conversation.
        conversation: ConversationId,
        /// The holder's error.
        #[source]
        source: HolderError,
    },

    /// The PTY master could not be set up for asynchronous IO.
    #[error("could not set up the PTY master of conversation {conversation}")]
    Master {
        /// The conversation.
        conversation: ConversationId,
        /// The error from the operating system or the runtime.
        #[source]
        source: io::Error,
    },

    /// The conversation's screen could not be started or did not answer.
    #[error("the screen of conversation {conversation} failed")]
    Screen {
        /// The conversation.
        conversation: ConversationId,
        /// The screen's error.
        #[source]
        source: ScreenError,
    },

    /// The holder refused a resize or a signal.
    #[error("the holder refused an operation on the hidden shell of conversation {conversation}")]
    Holder {
        /// The conversation.
        conversation: ConversationId,
        /// The holder's error.
        #[source]
        source: HolderError,
    },

    /// The shell is busy: another run of the conversation is under way, or an
    /// unfinished command line waits at a continuation prompt.
    #[error("the hidden shell of conversation {conversation} is busy")]
    Busy {
        /// The conversation.
        conversation: ConversationId,
    },

    /// The shell did not reach a prompt before the run's timeout (it was starting, or an
    /// earlier command still ran), so the command was never typed.
    #[error("the hidden shell of conversation {conversation} did not reach a prompt in time")]
    NotReady {
        /// The conversation.
        conversation: ConversationId,
    },

    /// The conversation has no hidden shell.
    #[error("conversation {conversation} has no hidden shell")]
    NoShell {
        /// The conversation.
        conversation: ConversationId,
    },

    /// The shell exited, before or during the operation.
    #[error("the hidden shell of conversation {conversation} exited")]
    Exited {
        /// The conversation.
        conversation: ConversationId,
        /// How it ended, when the holder could tell.
        status: Option<ChildStatus>,
    },

    /// The command cannot be typed into a shell as written.
    #[error("the command cannot be typed into the shell: {reason}")]
    InvalidCommand {
        /// What is wrong with it.
        reason: &'static str,
    },

    /// No command runs in the conversation's shell for an answer to reach.
    #[error("no command of conversation {conversation} runs in its hidden shell")]
    NoCall {
        /// The conversation.
        conversation: ConversationId,
    },

    /// The call's command does not wait for that input now, so the answer was not
    /// written.
    #[error("the command does not wait for this input: {reason}")]
    NotWaiting {
        /// The conversation.
        conversation: ConversationId,
        /// Why not.
        reason: &'static str,
    },

    /// An answer is not one line that can be typed. The reason never quotes the text.
    #[error("the answer cannot be typed: {reason}")]
    InvalidAnswer {
        /// What is wrong with it.
        reason: &'static str,
    },

    /// The terminal's modes could not be read, or the answer could not be written.
    #[error("the terminal of conversation {conversation} failed")]
    Terminal {
        /// The conversation.
        conversation: ConversationId,
        /// The error from the operating system.
        #[source]
        source: io::Error,
    },
}

#[cfg(test)]
mod tests;

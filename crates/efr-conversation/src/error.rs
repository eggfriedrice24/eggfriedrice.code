//! The one public error type of the crate.

use std::io;
use std::path::PathBuf;
use std::sync::Arc;

use efr_protocol::{CallId, ConversationId, Origin, QuestionId, TurnId};
use efr_provider::ProviderError;
use efr_stdx::StdxError;
use efr_store::StoreError;
use efr_store::receipts::Receipt;

/// Every way a request to a conversation, or the work of a turn, can fail.
///
/// A failure that the model or the user should see, such as a provider error or a
/// denied tool call, is not an error: it is recorded as an event (`turn_failed`, a
/// failed `tool_call_completed`). These variants are what the caller of a
/// [`ConversationHandle`](crate::ConversationHandle) method acts on; the daemon maps
/// them to wire errors.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ConversationError {
    /// The event log could not be read or written.
    #[error("the event log could not be read or written")]
    Store {
        /// The store's error.
        #[source]
        source: StoreError,
    },

    /// The command id already has a receipt, so the command did not run again. The
    /// caller answers with the stored outcome.
    #[error("the command {} already has a receipt", .receipt.command_id)]
    DuplicateCommand {
        /// The stored receipt.
        receipt: Box<Receipt>,
    },

    /// The request names another conversation than the one whose handle received it.
    #[error("the request names the conversation {requested}, not {conversation_id}")]
    WrongConversation {
        /// The conversation of the handle.
        conversation_id: ConversationId,
        /// The conversation the request named.
        requested: ConversationId,
    },

    /// Steering or interrupting needs a running turn, and there is none.
    #[error("the conversation {conversation_id} has no running turn")]
    NoRunningTurn {
        /// The conversation.
        conversation_id: ConversationId,
    },

    /// The request names a turn that is not the running one.
    #[error("the turn {requested} is not the running turn {running}")]
    TurnMismatch {
        /// The running turn.
        running: TurnId,
        /// The turn the request named.
        requested: TurnId,
    },

    /// The prompt of this turn no longer waits in the queue: it started, ended or was
    /// withdrawn, so it cannot be withdrawn.
    #[error("the prompt of the turn {turn_id} no longer waits in the queue")]
    PromptNotWaiting {
        /// The turn of the prompt.
        turn_id: TurnId,
    },

    /// The conversation never queued a prompt for this turn.
    #[error("the conversation {conversation_id} has no turn {turn_id}")]
    UnknownTurn {
        /// The conversation.
        conversation_id: ConversationId,
        /// The turn the request named.
        turn_id: TurnId,
    },

    /// The request asks for a choice that this build does not know, such as a kind of
    /// `if_late` or of a withdraw target from a newer protocol.
    #[error("this daemon does not support this {what}")]
    Unsupported {
        /// What the request chose, such as `"withdraw target"`.
        what: &'static str,
    },

    /// No prompt from this terminal waits in the queue.
    #[error("no prompt from the terminal {tty} waits in the queue")]
    NoQueuedPrompt {
        /// The terminal the request named.
        tty: String,
    },

    /// No turn of this conversation waits for an answer about this call: it was never
    /// asked here, it is answered already, or it expired.
    #[error("the call {call_id} has no pending approval")]
    ApprovalNotPending {
        /// The call.
        call_id: CallId,
    },

    /// No turn of this conversation waits for an answer to this quarantine question:
    /// it was never asked here, it is answered already, or it expired.
    #[error("the question {question_id} is not pending")]
    QuestionNotPending {
        /// The question.
        question_id: QuestionId,
    },

    /// A quarantine question takes an answer only from the user's own machine: an
    /// answer from a phone leaves the changes in quarantine.
    #[error("an answer from {origin:?} cannot take changes out of quarantine")]
    RemoteSurfaceAnswer {
        /// The surface that answered.
        origin: Origin,
    },

    /// The judge of an exit (the classifier, from phase 3) could not answer; the user
    /// is asked instead.
    #[error("the judge of an exit could not answer")]
    Judge {
        /// The judge's error.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// The prompt queue is full; the prompt was not recorded.
    #[error("the conversation {conversation_id} already has {limit} queued prompts")]
    QueueFull {
        /// The conversation.
        conversation_id: ConversationId,
        /// The most prompts that may wait.
        limit: usize,
    },

    /// A turn setting cannot work: the model is not in the model list, or the effort is
    /// not one that the model takes. Nothing was recorded for a prompt; a turn that
    /// starts with it fails.
    #[error("{}", invalid_setting(.setting, .value, .model.as_deref(), .choices, *.from_config))]
    InvalidSetting {
        /// `model` or `effort`.
        setting: &'static str,
        /// The value that does not fit.
        value: String,
        /// For an effort, the model whose efforts it was checked against.
        model: Option<String>,
        /// The values that would fit; empty when any lowercase word would.
        choices: Vec<String>,
        /// True when the value is the config's default rather than the prompt's own.
        from_config: bool,
    },

    /// The scratch root could not be created.
    #[error("could not create the scratch root {}", .path.display())]
    CreateScratchRoot {
        /// The root.
        path: PathBuf,
        /// The error from the file system.
        #[source]
        source: io::Error,
    },

    /// The conversation's scratch directory could not be claimed or marked.
    #[error("could not claim the scratch directory {}", .path.display())]
    ClaimScratch {
        /// The directory.
        path: PathBuf,
        /// The error from the file system.
        #[source]
        source: StdxError,
    },

    /// Every name the conversation's scratch directory may take is used by something
    /// else.
    #[error("no free name is left for a scratch directory under {}", .root.display())]
    ScratchNamesExhausted {
        /// The scratch root.
        root: PathBuf,
    },

    /// The result of a command could not be encoded for its receipt.
    #[error("the result of {method} could not be encoded for its receipt")]
    EncodeReceipt {
        /// The wire method, such as `prompt.send`.
        method: &'static str,
        /// The serialiser's error.
        #[source]
        source: serde_json::Error,
    },

    /// A turn of the conversation runs, or a compaction does, so a manual compaction
    /// cannot run now. The turn compacts on its own when it needs to.
    #[error("the conversation {conversation_id} is busy with a turn or a compaction")]
    CompactionBusy {
        /// The conversation.
        conversation_id: ConversationId,
    },

    /// Nothing lies before the verbatim tail of the conversation's history, so a
    /// compaction would free no room.
    #[error("the conversation {conversation_id} has nothing to compact")]
    NothingToCompact {
        /// The conversation.
        conversation_id: ConversationId,
    },

    /// The summary request of a compaction failed; nothing was recorded.
    #[error("the summary request of the compaction failed")]
    Summary {
        /// The provider's error, shared with the retries of the same command.
        #[source]
        source: Arc<ProviderError>,
    },

    /// A manual compaction failed with an error that only its first caller gets, such
    /// as a store error; a retry of the same command that waited for it gets this.
    /// Nothing was recorded.
    #[error("the compaction of this command failed; nothing was recorded")]
    CompactionFailed,

    /// The model answered the summary request of a compaction without text; nothing was
    /// recorded.
    #[error("the model wrote no summary")]
    EmptySummary,

    /// The model stopped the summary of a compaction before its end: it reached the
    /// output limit, or the provider stopped it for its content. Nothing was recorded.
    #[error("the summary was cut off before its end")]
    IncompleteSummary,

    /// The conversation's actor has stopped, so it takes no more requests.
    #[error("the conversation actor has stopped")]
    Stopped,

    /// A task of the conversation panicked before it answered.
    #[error("the conversation's {task} task panicked")]
    TaskPanicked {
        /// The task, such as `"scratch"`.
        task: &'static str,
    },
}

/// The message of [`ConversationError::InvalidSetting`]: what does not fit, and the
/// choices.
fn invalid_setting(
    setting: &str,
    value: &str,
    model: Option<&str>,
    choices: &[String],
    from_config: bool,
) -> String {
    let what = if from_config {
        format!("the config's default {setting} {value}")
    } else {
        format!("the {setting} {value}")
    };
    let problem = match (setting, model) {
        ("effort", Some(model)) if choices.is_empty() => {
            format!("{what} is not a word of lowercase letters, digits, - and _ (for {model})")
        }
        ("effort", Some(model)) => format!("{what} is not an effort of {model}"),
        _ => format!("{what} is not in the model list"),
    };
    if choices.is_empty() {
        problem
    } else {
        format!("{problem}; choose one of: {}", choices.join(", "))
    }
}

impl ConversationError {
    /// This error once more, for a retry of the same command that waited for it: the
    /// same variant when it can be copied, else [`CompactionFailed`](Self::CompactionFailed).
    pub(crate) fn again(&self) -> Self {
        match self {
            ConversationError::NothingToCompact { conversation_id } => {
                ConversationError::NothingToCompact { conversation_id: *conversation_id }
            }
            ConversationError::Summary { source } => {
                ConversationError::Summary { source: Arc::clone(source) }
            }
            ConversationError::EmptySummary => ConversationError::EmptySummary,
            ConversationError::IncompleteSummary => ConversationError::IncompleteSummary,
            ConversationError::DuplicateCommand { receipt } => {
                ConversationError::DuplicateCommand { receipt: receipt.clone() }
            }
            ConversationError::Stopped => ConversationError::Stopped,
            _ => ConversationError::CompactionFailed,
        }
    }

    /// The store's error, with a duplicate command id and an approval that is no
    /// longer pending turned into the variants the caller acts on.
    pub(crate) fn from_store(source: StoreError) -> Self {
        match source {
            StoreError::DuplicateCommand { receipt } => {
                ConversationError::DuplicateCommand { receipt }
            }
            StoreError::ApprovalNotPending { call_id, .. } => {
                ConversationError::ApprovalNotPending { call_id }
            }
            source => ConversationError::Store { source },
        }
    }

    /// The data of the wire error, for an [`InvalidSetting`](Self::InvalidSetting): the
    /// setting, its value, the choices and, for an effort, the model, so a client can
    /// offer the choices. `None` for every other error.
    pub fn data(&self) -> Option<serde_json::Value> {
        let ConversationError::InvalidSetting { setting, value, model, choices, .. } = self else {
            return None;
        };
        let mut data =
            serde_json::json!({ "setting": setting, "value": value, "choices": choices });
        if let (Some(model), serde_json::Value::Object(members)) = (model, &mut data) {
            members.insert("model".to_owned(), serde_json::Value::String(model.clone()));
        }
        Some(data)
    }
}

#[cfg(test)]
mod tests;

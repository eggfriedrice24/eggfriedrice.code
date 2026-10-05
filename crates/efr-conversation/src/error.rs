//! The one public error type of the crate.

use std::io;
use std::path::PathBuf;

use efr_protocol::{CallId, ConversationId, TurnId};
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

    /// No turn of this conversation waits for an answer about this call: it was never
    /// asked here, it is answered already, or it expired.
    #[error("the call {call_id} has no pending approval")]
    ApprovalNotPending {
        /// The call.
        call_id: CallId,
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

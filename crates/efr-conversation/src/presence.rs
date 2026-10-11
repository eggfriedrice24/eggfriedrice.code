//! Presence as a conversation reads it: whether a person is there to answer a question.
//!
//! The daemon owns the state of each conversation (the input of its attached clients,
//! `efr away`, the idle timeout) and implements [`PresenceReader`] over it. A turn reads
//! it when it parks a question, and again every [`PRESENCE_CHECK`] while the question
//! waits: while the user is away, the question waits only
//! [`ConversationConfig::away_wait`](crate::ConversationConfig::away_wait) from the
//! moment the turn sees it, and then the call does not run. An answer always wins
//! during the wait.

use std::fmt;
use std::time::Duration;

use efr_protocol::{CallId, ConversationId, Presence};

/// How often a waiting question reads the presence again. The daemon runs no timer per
/// conversation, so this read is also what finds that the idle limit passed.
pub(crate) const PRESENCE_CHECK: Duration = Duration::from_secs(15);

/// Reads the presence of a conversation, and sends a question that waits while the user
/// is away to the user's phones.
///
/// [`ConversationDeps`](crate::ConversationDeps) is one value for every conversation,
/// so each call names its conversation.
pub trait PresenceReader: Send + Sync + fmt::Debug {
    /// Whether a person is at `conversation_id` now.
    fn presence(&self, conversation_id: ConversationId) -> Presence;

    /// Sends the question of `call_id` to the user's paired phones, once per question.
    /// No phone client exists yet, so the default does nothing.
    fn push(&self, _conversation_id: ConversationId, _call_id: CallId) {}
}

/// A reader that counts a person present at every conversation, so every question
/// waits for an answer, as before phase A.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AlwaysPresent;

impl PresenceReader for AlwaysPresent {
    fn presence(&self, _conversation_id: ConversationId) -> Presence {
        Presence::Present
    }
}

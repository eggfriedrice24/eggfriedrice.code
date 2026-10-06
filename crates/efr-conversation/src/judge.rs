//! The judge of an exit: who answers a question about an action that leaves the `auto`
//! sandbox before the user does.
//!
//! In phase 1 the user answers every exit, so no judge is configured and the turn never
//! calls one. From phase 3 the daemon implements [`ExitJudge`] over a classifier
//! request, and `Turn::ask` asks it first when every deciding reason is an exit that a
//! classifier may judge. The record it reads ([`ExitRecord`]) holds user messages, the
//! action and facts that efr collected itself, never tool output or the model's reason.

use std::fmt;

use async_trait::async_trait;
use efr_protocol::{ExitRecord, Judgement};

use crate::ConversationError;

/// Judges one exit from its record: allow, deny or ask the user.
///
/// An error, a timeout or an answer that does not fit asks the user; a judge never
/// allows by failing.
#[async_trait]
pub trait ExitJudge: Send + Sync + fmt::Debug {
    /// The verdict about the exit that `record` describes. An error asks the user, so
    /// the daemon wraps the judge's own failure in [`ConversationError::Judge`].
    async fn judge(&self, record: &ExitRecord) -> Result<Judgement, ConversationError>;
}

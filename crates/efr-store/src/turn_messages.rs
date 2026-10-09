//! The exact messages of finished turns, with the provider's own items.
//!
//! A provider hands back items that no event holds, such as encrypted reasoning and the
//! ids of its output items, and wants them back unchanged on the next request to the
//! same model. The conversation saves a finished turn's messages here, in the batch
//! that records the turn's end, so a conversation that goes on after a restart sends
//! the same request as without the restart. The store keeps each message as opaque
//! JSON with the provider and the model that answered the turn; which messages go back
//! to which model is the conversation's decision.
//!
//! The table is not a projection: the event log cannot rebuild it. It keeps every turn
//! since the newest summary, because every request sends those turns word for word: a
//! turn that left the table would come back from its events, different, and change the
//! start of every later request. A compaction with a summary drops the turns before its
//! cut (`forget_compacted`): the summary takes their place in every later request.

use efr_protocol::{Compaction, ConversationId, Seq, TurnId};
use rusqlite::{Connection, OptionalExtension as _, params};
use serde_json::Value;

use crate::{StoreError, sql};

const TABLE: &str = "turn_messages";

/// The messages of one finished turn, saved with the batch that records its end.
#[derive(Debug, Clone, PartialEq)]
pub struct NewTurnMessages {
    conversation_id: ConversationId,
    turn_id: TurnId,
    provider: String,
    model: String,
    messages: Vec<Value>,
}

impl NewTurnMessages {
    /// The `messages` of `turn_id`, answered by `model` of `provider`.
    pub fn new(
        conversation_id: ConversationId,
        turn_id: TurnId,
        provider: impl Into<String>,
        model: impl Into<String>,
        messages: Vec<Value>,
    ) -> Self {
        NewTurnMessages {
            conversation_id,
            turn_id,
            provider: provider.into(),
            model: model.into(),
            messages,
        }
    }
}

/// The saved messages of one turn.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct TurnMessages {
    /// The turn.
    pub turn_id: TurnId,
    /// The provider that answered it.
    pub provider: String,
    /// The model that answered it.
    pub model: String,
    /// The messages, in order.
    pub messages: Vec<Value>,
}

/// Saves `item` as the messages of a turn whose terminal event has `turn_seq`. Saving
/// a turn again replaces it.
pub(crate) fn save(
    conn: &Connection,
    item: &NewTurnMessages,
    turn_seq: Seq,
) -> Result<(), StoreError> {
    let conversation_id = item.conversation_id.to_string();
    let turn_id = item.turn_id.to_string();
    conn.execute("DELETE FROM turn_messages WHERE turn_id = ?1", [&turn_id])?;
    let mut insert = conn.prepare(
        "INSERT INTO turn_messages (conversation_id, turn_id, position, provider, model, \
         message, turn_seq) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
    )?;
    for (position, message) in item.messages.iter().enumerate() {
        insert.execute(params![
            conversation_id,
            turn_id,
            i64::try_from(position).unwrap_or(i64::MAX),
            item.provider,
            item.model,
            sql::to_json(message, "turn message")?,
            sql::seq(turn_seq),
        ])?;
    }
    Ok(())
}

/// Drops the saved messages that no request needs after `compaction` of
/// `conversation_id`: the turns before its cut, and its `through_turn` too when the cut
/// covers that whole turn. Only a compaction with a summary drops anything; after a
/// pruning alone, the history still sends every turn. Runs in the batch that records
/// the compaction, after the projections, so the `turns` row of `through_turn` says
/// where that turn stands in the log. A turn that the projection does not know drops
/// nothing.
pub(crate) fn forget_compacted(
    conn: &Connection,
    conversation_id: ConversationId,
    compaction: &Compaction,
) -> Result<(), StoreError> {
    if compaction.summary.is_none() {
        return Ok(());
    }
    let through = compaction.through_turn.to_string();
    let bound: Option<i64> = conn
        .query_row("SELECT last_seq FROM turns WHERE id = ?1", [&through], |row| row.get(0))
        .optional()?;
    let Some(bound) = bound else {
        return Ok(());
    };
    // NOTE: the turns of a conversation never overlap, so a turn that ended before the
    // newest event of `through_turn` came before that turn; a running `through_turn` has
    // saved nothing yet.
    conn.execute(
        "DELETE FROM turn_messages WHERE conversation_id = ?1 AND (turn_seq < ?2 OR (?3 AND \
         turn_id = ?4))",
        params![conversation_id.to_string(), bound, compaction.through_message.is_none(), through],
    )?;
    Ok(())
}

/// The saved turns of a conversation, oldest first.
pub fn of_conversation(
    conn: &Connection,
    conversation_id: ConversationId,
) -> Result<Vec<TurnMessages>, StoreError> {
    let mut stmt = conn.prepare(
        "SELECT turn_id, provider, model, message FROM turn_messages WHERE conversation_id = ?1 \
         ORDER BY turn_seq, turn_id, position",
    )?;
    let rows = stmt
        .query_map([conversation_id.to_string()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let mut turns: Vec<TurnMessages> = Vec::new();
    for (turn_id, provider, model, message) in rows {
        let turn_id: TurnId = sql::parse(&turn_id, TABLE, "turn_id")?;
        let message: Value = sql::from_json(&message, TABLE, "message")?;
        match turns.last_mut() {
            Some(turn) if turn.turn_id == turn_id => turn.messages.push(message),
            _ => turns.push(TurnMessages { turn_id, provider, model, messages: vec![message] }),
        }
    }
    Ok(turns)
}

#[cfg(test)]
mod tests;

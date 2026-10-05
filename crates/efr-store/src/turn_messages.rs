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
//! The table is not a projection: the event log cannot rebuild it. It is bounded per
//! conversation to the newest turns that the history may carry.

use efr_protocol::{ConversationId, Seq, TurnId};
use rusqlite::{Connection, params};
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
    keep: usize,
}

impl NewTurnMessages {
    /// The `messages` of `turn_id`, answered by `model` of `provider`. Saving them
    /// keeps the newest `keep` turns of the conversation and drops older ones; with a
    /// `keep` of 0 the conversation keeps none.
    pub fn new(
        conversation_id: ConversationId,
        turn_id: TurnId,
        provider: impl Into<String>,
        model: impl Into<String>,
        messages: Vec<Value>,
        keep: usize,
    ) -> Self {
        NewTurnMessages {
            conversation_id,
            turn_id,
            provider: provider.into(),
            model: model.into(),
            messages,
            keep,
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

/// Saves `item` as the messages of a turn whose terminal event has `turn_seq`, then
/// drops the turns of its conversation beyond the newest `keep`. Saving a turn again
/// replaces it.
pub(crate) fn save(
    conn: &Connection,
    item: &NewTurnMessages,
    turn_seq: Seq,
) -> Result<(), StoreError> {
    let conversation_id = item.conversation_id.to_string();
    let turn_id = item.turn_id.to_string();
    conn.execute("DELETE FROM turn_messages WHERE turn_id = ?1", [&turn_id])?;
    if item.keep == 0 {
        conn.execute("DELETE FROM turn_messages WHERE conversation_id = ?1", [&conversation_id])?;
        return Ok(());
    }
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
    conn.execute(
        "DELETE FROM turn_messages WHERE conversation_id = ?1 AND turn_seq < ( \
           SELECT MIN(turn_seq) FROM ( \
             SELECT DISTINCT turn_seq FROM turn_messages WHERE conversation_id = ?1 \
             ORDER BY turn_seq DESC LIMIT ?2))",
        params![conversation_id, i64::try_from(item.keep).unwrap_or(i64::MAX)],
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

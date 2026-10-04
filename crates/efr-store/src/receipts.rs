//! Command receipts: what a client's command id already did.
//!
//! Every write that a client may retry (`prompt.send`, `approval.respond`, and the
//! other methods with a `command_id`) is answered from its receipt when it has one:
//! an accepted command returns its stored result, a rejected one stays rejected, and
//! nothing runs a second time. Nothing is replayed on the client's behalf after a
//! reconnect; a retry is always the client's choice.
//!
//! A new command records its receipt in the batch that carries its events, so the
//! receipt exists exactly when the events do. Two racing requests with one command id
//! cannot both commit: the later batch fails with [`StoreError::DuplicateCommand`],
//! which carries the earlier receipt for the caller to answer with.

use efr_protocol::{CommandId, ErrorBody, EventEnvelope, Seq};
use jiff::Timestamp;
use rusqlite::{Connection, OptionalExtension as _, Row, params};
use serde_json::Value;

use crate::{StoreError, sql};

const TABLE: &str = "receipts";
const ACCEPTED: &str = "accepted";
const REJECTED: &str = "rejected";

/// What happened to a command.
#[derive(Debug, Clone, PartialEq)]
pub enum ReceiptOutcome {
    /// The command ran.
    Accepted {
        /// The method's result, as the handler chose to store it.
        result: Value,
    },
    /// The command was refused, and a retry gets the same refusal.
    Rejected {
        /// The error the client received.
        error: ErrorBody,
    },
}

/// A receipt to record with a batch.
#[derive(Debug, Clone, PartialEq)]
pub struct NewReceipt {
    command_id: CommandId,
    method: String,
    outcome: ReceiptOutcome,
    /// The position of the event whose sequence number the receipt records, among the
    /// batch's events; `None` for the last one.
    seq_of_event: Option<usize>,
}

impl NewReceipt {
    /// A receipt for a command that ran with `result`.
    ///
    /// The receipt's `seq` is filled in at commit with the sequence number of the
    /// batch's last event, or of the event that [`NewReceipt::seq_of_event`] names. A
    /// result that reports that number (such as `PromptSendResult::seq`) is stored
    /// without it and completed from `seq` when it is answered again.
    pub fn accepted(command_id: CommandId, method: impl Into<String>, result: Value) -> Self {
        NewReceipt {
            command_id,
            method: method.into(),
            outcome: ReceiptOutcome::Accepted { result },
            seq_of_event: None,
        }
    }

    /// A receipt for a command that was refused with `error`.
    pub fn rejected(command_id: CommandId, method: impl Into<String>, error: ErrorBody) -> Self {
        NewReceipt {
            command_id,
            method: method.into(),
            outcome: ReceiptOutcome::Rejected { error },
            seq_of_event: None,
        }
    }

    /// Records the sequence number of the batch's event at `index`, counted from 0 in
    /// the order the events were added, instead of the last event's.
    ///
    /// A result reports one event, and other events may follow it in the same batch:
    /// `prompt.send` on an idle conversation commits `prompt_queued` and then
    /// `turn_started`, while `PromptSendResult::seq` is the number of `prompt_queued`.
    /// Naming that event here makes a retry get the number the first answer had. The
    /// batch fails, and writes nothing, with [`StoreError::ReceiptEventMissing`] when
    /// it has no event at `index`.
    pub fn seq_of_event(mut self, index: usize) -> Self {
        self.seq_of_event = Some(index);
        self
    }

    /// The command id the receipt is for.
    pub fn command_id(&self) -> CommandId {
        self.command_id
    }

    /// The sequence number to record, from the batch's committed events.
    pub(crate) fn seq_in(&self, events: &[EventEnvelope]) -> Result<Option<Seq>, StoreError> {
        let Some(index) = self.seq_of_event else {
            return Ok(events.last().map(|envelope| envelope.seq));
        };
        match events.get(index) {
            Some(envelope) => Ok(Some(envelope.seq)),
            None => Err(StoreError::ReceiptEventMissing {
                command_id: self.command_id,
                index,
                events: events.len(),
            }),
        }
    }
}

/// A stored receipt.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Receipt {
    /// The command.
    pub command_id: CommandId,
    /// The wire method that carried it, such as `prompt.send`.
    pub method: String,
    /// What happened.
    pub outcome: ReceiptOutcome,
    /// The sequence number of the event the receipt records: the last event committed
    /// with it, or the one that [`NewReceipt::seq_of_event`] named; `None` when its
    /// batch had no events.
    pub seq: Option<Seq>,
    /// When the receipt was recorded.
    pub created_at: Timestamp,
}

/// The receipt of `command_id`, if the command ran or was refused before.
pub fn lookup(conn: &Connection, command_id: CommandId) -> Result<Option<Receipt>, StoreError> {
    conn.query_row(
        "SELECT command_id, method, outcome, result, seq, created_at FROM receipts \
         WHERE command_id = ?1",
        [command_id.to_string()],
        RawReceipt::from_row,
    )
    .optional()?
    .map(RawReceipt::decode)
    .transpose()
}

/// Records `receipt` inside the writer's transaction, or fails with
/// [`StoreError::DuplicateCommand`] when the command id already has one.
pub(crate) fn record(
    conn: &Connection,
    receipt: &NewReceipt,
    seq: Option<Seq>,
    at: Timestamp,
) -> Result<(), StoreError> {
    if let Some(existing) = lookup(conn, receipt.command_id)? {
        return Err(StoreError::DuplicateCommand { receipt: Box::new(existing) });
    }
    let (outcome, result) = match &receipt.outcome {
        ReceiptOutcome::Accepted { result } => (ACCEPTED, sql::to_json(result, "receipt result")?),
        ReceiptOutcome::Rejected { error } => (REJECTED, sql::to_json(error, "receipt error")?),
    };
    conn.execute(
        "INSERT INTO receipts (command_id, method, outcome, result, seq, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            receipt.command_id.to_string(),
            receipt.method,
            outcome,
            result,
            seq.map(sql::seq),
            sql::micros(at)
        ],
    )?;
    Ok(())
}

struct RawReceipt {
    command_id: String,
    method: String,
    outcome: String,
    result: String,
    seq: Option<i64>,
    created_at: i64,
}

impl RawReceipt {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(RawReceipt {
            command_id: row.get(0)?,
            method: row.get(1)?,
            outcome: row.get(2)?,
            result: row.get(3)?,
            seq: row.get(4)?,
            created_at: row.get(5)?,
        })
    }

    fn decode(self) -> Result<Receipt, StoreError> {
        let outcome = match self.outcome.as_str() {
            ACCEPTED => {
                ReceiptOutcome::Accepted { result: sql::from_json(&self.result, TABLE, "result")? }
            }
            REJECTED => {
                ReceiptOutcome::Rejected { error: sql::from_json(&self.result, TABLE, "result")? }
            }
            _ => return Err(sql::decode_error(TABLE, "outcome", sql::UnknownName(self.outcome))),
        };
        Ok(Receipt {
            command_id: sql::parse(&self.command_id, TABLE, "command_id")?,
            method: self.method,
            outcome,
            seq: self.seq.map(|value| sql::to_seq(value, TABLE, "seq")).transpose()?,
            created_at: sql::to_timestamp(self.created_at, TABLE, "created_at")?,
        })
    }
}

#[cfg(test)]
mod tests;

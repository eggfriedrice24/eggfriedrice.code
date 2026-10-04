//! Command receipts on the daemon's side: answering a retried write from its receipt,
//! and recording a refusal so that a retry gets the same one.
//!
//! The conversation stores an accepted command's result without its `seq`, and the
//! store records the number with the receipt, so the stored result plus `receipt.seq`
//! is the first answer exactly. A refused command gets its receipt here, because the
//! daemon owns the mapping to wire errors.

use efr_protocol::{CommandId, ErrorCode, ErrorFrame};
use efr_store::receipts::{NewReceipt, Receipt, ReceiptOutcome};
use efr_store::{Batch, StoreError};
use serde_json::Value;

use crate::DaemonError;
use crate::state::State;

/// The stored receipt of `command_id`, if the command ran or was refused before.
pub(crate) async fn lookup(
    state: &State,
    command_id: CommandId,
) -> Result<Option<Receipt>, DaemonError> {
    Ok(state.readers.with(move |conn| efr_store::receipts::lookup(conn, command_id)).await?)
}

/// The first answer of a command of `method` from its receipt: the stored result with
/// its `seq`, or the stored refusal as an error.
pub(crate) fn replay(method: &'static str, receipt: Receipt) -> Result<Value, DaemonError> {
    if receipt.method != method {
        return Err(DaemonError::CommandReused {
            command_id: receipt.command_id,
            method: receipt.method,
        });
    }
    match receipt.outcome {
        ReceiptOutcome::Accepted { mut result } => {
            if let (Value::Object(members), Some(seq)) = (&mut result, receipt.seq) {
                members.insert("seq".to_owned(), Value::from(seq.get()));
            }
            Ok(result)
        }
        ReceiptOutcome::Rejected { error } => Err(DaemonError::Rejected { body: error }),
    }
}

/// Whether a refusal with this code is final, so a retry must get it again. A busy
/// store, a full queue, a cancel or a daemon fault may pass, so those are not kept.
pub(crate) fn is_final(code: ErrorCode) -> bool {
    matches!(code, ErrorCode::Invalid | ErrorCode::NotFound | ErrorCode::Conflict)
}

/// Records the refusal of `command_id` when it is final, and returns the error to
/// answer with either way.
pub(crate) async fn refuse(
    state: &State,
    command_id: CommandId,
    method: &'static str,
    error: DaemonError,
) -> DaemonError {
    if !is_final(error.code()) {
        return error;
    }
    let body = ErrorFrame::from(error).error;
    let batch = Batch::new().receipt(NewReceipt::rejected(command_id, method, body.clone()));
    match state.writer.append(batch).await {
        Ok(_) => {}
        // A racing retry recorded its outcome first; that one stands.
        Err(StoreError::DuplicateCommand { .. }) => {}
        Err(failure) => {
            tracing::warn!(error = %failure, %command_id, "the refusal could not be recorded");
        }
    }
    DaemonError::Rejected { body }
}

#[cfg(test)]
mod tests;

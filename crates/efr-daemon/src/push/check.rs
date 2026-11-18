//! The test notice of `admin.push_test` (`efr push test`): one push to each phone
//! that has a token, then the receipts after [`RECEIPT_WAITS`], so the user sees what
//! became of each one in about 14 seconds.

use std::collections::HashMap;
use std::time::Duration;

use efr_http::{ExpoAnswer, PushCode, Receipt, TicketId};
use efr_protocol::{PushTestResult, PushTestRow};

use super::{Message, NOTICE_TTL, Sent, refused, send};
use crate::DaemonError;
use crate::state::State;

/// The body of the test notice, under the title `efr`.
const BODY: &str = "test";

/// The waits before each read of the receipts. A ticket with no receipt after the last
/// one is `no_receipt`.
const RECEIPT_WAITS: [Duration; 3] =
    [Duration::from_secs(2), Duration::from_secs(4), Duration::from_secs(8)];

/// Sends the test notice to each phone that has a token, the oldest pairing first, and
/// tells what became of each one. A dead token goes, as after any push.
pub(crate) async fn check(state: &State) -> Result<Vec<PushTestRow>, DaemonError> {
    let Some(expo) = &state.push.expo else {
        return Err(DaemonError::InvalidParams {
            reason: "EFR_EXPO_PUSH_URL is not an http or https URL, so this efrd sends no push",
        });
    };
    let targets = state.readers.with(|conn| efr_store::device_push::targets(conn, None)).await?;
    let daemon = state.daemon_id.to_string();
    let message = Message { body: BODY, ttl: NOTICE_TTL, conversation: None, daemon: &daemon };
    let mut results = Vec::with_capacity(targets.len());
    // NOTE: the index of each phone whose ticket waits for its receipt.
    let mut open: HashMap<TicketId, usize> = HashMap::new();
    for (index, sent) in send(expo, &targets, &message).await.into_iter().enumerate() {
        let result = match sent {
            Sent::Ticket(id) => {
                open.insert(id, index);
                (PushTestResult::NoReceipt, None)
            }
            Sent::Failed(code) => {
                let target = &targets[index];
                refused(state, target.device_id, target.token.clone(), code.clone()).await;
                (PushTestResult::NotSent, code)
            }
            Sent::Refused(code) => (PushTestResult::NotSent, code),
            Sent::NoAnswer => (PushTestResult::NotSent, None),
        };
        results.push(result);
    }
    for wait in RECEIPT_WAITS {
        if open.is_empty() {
            break;
        }
        state.clock.sleep(wait).await;
        let ids: Vec<TicketId> = open.keys().cloned().collect();
        let Ok(ExpoAnswer::Data(receipts)) = expo.receipts(&ids).await else {
            continue;
        };
        for (id, receipt) in receipts {
            let Some(index) = open.remove(&id) else {
                continue;
            };
            results[index] = match receipt {
                Receipt::Failed(code) => {
                    let target = &targets[index];
                    refused(state, target.device_id, target.token.clone(), code.clone()).await;
                    (PushTestResult::NotSent, code)
                }
                _ => (PushTestResult::Sent, None),
            };
        }
    }
    let rows = targets.into_iter().zip(results).map(|(target, (result, code))| PushTestRow {
        name: target.name,
        result,
        code: code.as_ref().map(|code| PushCode::as_str(code).to_owned()),
    });
    Ok(rows.collect())
}

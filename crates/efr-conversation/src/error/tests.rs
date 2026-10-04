use efr_protocol::{CallId, ConversationId};
use efr_stdx::id::uuid_v7;
use efr_store::StoreError;
use efr_test_support::{TestClock, TestRng};

use super::ConversationError;

#[test]
fn an_approval_that_is_no_longer_pending_keeps_its_call() {
    let clock = TestClock::new();
    let call_id = CallId::from_uuid(uuid_v7(&clock, &TestRng::new(1)));
    let conversation_id = ConversationId::from_uuid(uuid_v7(&clock, &TestRng::new(2)));
    let error =
        ConversationError::from_store(StoreError::ApprovalNotPending { conversation_id, call_id });
    assert!(
        matches!(error, ConversationError::ApprovalNotPending { call_id: id } if id == call_id)
    );
}

#[test]
fn other_store_errors_keep_their_source() {
    let error = ConversationError::from_store(StoreError::WriterStopped);
    assert!(matches!(error, ConversationError::Store { source: StoreError::WriterStopped }));
    assert_eq!(error.to_string(), "the event log could not be read or written");
    assert!(std::error::Error::source(&error).is_some());
}

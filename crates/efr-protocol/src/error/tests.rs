use std::collections::BTreeSet;

use pretty_assertions::assert_eq;
use serde_json::json;

use crate::{ErrorBody, ErrorCode, ErrorFrame, ProtocolError, RequestId, Seq};

#[test]
fn every_code_serializes_as_its_wire_name() {
    for code in ErrorCode::ALL {
        assert_eq!(serde_json::to_value(code).unwrap(), json!(code.as_str()));
        assert_eq!(code.to_string(), code.as_str());
    }
}

#[test]
fn every_code_reads_back_from_its_wire_name() {
    for code in ErrorCode::ALL {
        let back: ErrorCode = serde_json::from_value(json!(code.as_str())).unwrap();
        assert_eq!(back, code);
    }
}

#[test]
fn the_code_set_has_ten_distinct_names() {
    let names: BTreeSet<_> = ErrorCode::ALL.iter().map(|code| code.as_str()).collect();
    assert_eq!(names.len(), 10);
}

#[test]
fn an_unknown_code_is_rejected_because_the_set_is_closed() {
    assert!(serde_json::from_value::<ErrorCode>(json!("teapot")).is_err());
}

#[test]
fn a_body_without_data_omits_the_data_member() {
    let body = ErrorBody::new(ErrorCode::NotFound, "no such conversation");
    assert_eq!(
        serde_json::to_value(&body).unwrap(),
        json!({ "code": "not_found", "message": "no such conversation" })
    );
}

#[test]
fn overflow_carries_the_last_seq_to_resume_from() {
    let body = ErrorBody::overflow(Seq::new(41));
    assert_eq!(body.code, ErrorCode::Overflow);
    assert_eq!(body.data, Some(json!({ "last_seq": 41 })));
    assert_eq!(body.last_seq(), Some(Seq::new(41)));
}

#[test]
fn last_seq_is_absent_when_the_data_has_none() {
    assert_eq!(ErrorBody::new(ErrorCode::Busy, "busy").last_seq(), None);
    let other = ErrorBody::new(ErrorCode::Busy, "busy").with_data(json!({ "retry": true }));
    assert_eq!(other.last_seq(), None);
}

#[test]
fn protocol_mismatch_names_both_versions() {
    let body = ErrorBody::protocol_mismatch(1, 2);
    assert_eq!(body.code, ErrorCode::ProtocolMismatch);
    assert_eq!(body.data, Some(json!({ "client": 2, "daemon": 1 })));
}

#[test]
fn an_error_frame_without_a_request_id_omits_the_id_member() {
    let frame = ErrorFrame { id: None, error: ErrorBody::new(ErrorCode::Invalid, "bad frame") };
    assert_eq!(
        serde_json::to_value(&frame).unwrap(),
        json!({ "error": { "code": "invalid", "message": "bad frame" } })
    );
}

#[test]
fn an_error_frame_names_its_request() {
    let frame = ErrorFrame {
        id: Some(RequestId::new(3)),
        error: ErrorBody::new(ErrorCode::Cancelled, "cancelled"),
    };
    let back: ErrorFrame = serde_json::from_value(serde_json::to_value(&frame).unwrap()).unwrap();
    assert_eq!(back, frame);
}

#[test]
fn messages_say_what_failed_in_one_sentence() {
    let too_large = ProtocolError::FrameTooLarge { len: 20, max: 10 };
    assert_eq!(too_large.to_string(), "a frame of 20 bytes is larger than the limit of 10 bytes");
    let truncated = ProtocolError::Truncated { buffered: 3 };
    assert_eq!(truncated.to_string(), "the stream ended inside a frame, after 3 bytes of it");
}

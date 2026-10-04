use pretty_assertions::assert_eq;
use serde_json::json;

use crate::{
    AdminStatus, ClientFrame, ErrorBody, ErrorCode, ErrorFrame, LeaseReportResult, Method,
    ProtocolError, RequestId, Seq, ServerFrame,
};

fn id(value: u64) -> RequestId {
    RequestId::new(value)
}

fn status_request() -> ClientFrame {
    ClientFrame::Request { id: id(1), method: Method::AdminStatus(AdminStatus {}) }
}

#[test]
fn a_request_is_id_then_method_then_params() {
    assert_eq!(
        serde_json::to_string(&status_request()).unwrap(),
        r#"{"id":1,"method":"admin.status","params":{}}"#
    );
}

#[test]
fn a_cancel_names_the_request() {
    let frame = ClientFrame::Cancel { id: id(4) };
    assert_eq!(serde_json::to_string(&frame).unwrap(), r#"{"cancel":4}"#);
    assert_eq!(frame.request_id(), id(4));
}

#[test]
fn client_frames_read_back_as_themselves() {
    for frame in [status_request(), ClientFrame::Cancel { id: id(9) }] {
        let back: ClientFrame =
            serde_json::from_slice(&serde_json::to_vec(&frame).unwrap()).unwrap();
        assert_eq!(back, frame);
    }
}

#[test]
fn a_request_without_an_id_is_rejected() {
    assert!(
        serde_json::from_value::<ClientFrame>(json!({ "method": "admin.status", "params": {} }))
            .is_err()
    );
}

#[test]
fn a_frame_with_both_cancel_and_method_is_rejected() {
    let frame = json!({ "cancel": 1, "id": 2, "method": "admin.status", "params": {} });
    assert!(serde_json::from_value::<ClientFrame>(frame).is_err());
}

#[test]
fn a_bad_request_keeps_its_id_so_the_daemon_can_answer_it() {
    let err = ClientFrame::from_json(br#"{"id":7,"method":"shell.exec","params":{}}"#).unwrap_err();
    assert!(matches!(err, ProtocolError::Decode { id: Some(found), .. } if found == id(7)));
}

#[test]
fn a_payload_that_is_not_json_has_no_id() {
    let err = ClientFrame::from_json(b"{\"id\":").unwrap_err();
    assert!(matches!(err, ProtocolError::Decode { id: None, .. }));
}

#[test]
fn server_frames_have_their_wire_shapes() {
    let item = ServerFrame::item(id(3), &LeaseReportResult { ttl_secs: 45 }).unwrap();
    let error = ServerFrame::error(Some(id(3)), ErrorBody::overflow(Seq::new(41)));
    let cases = [
        (item, r#"{"id":3,"item":{"ttl_secs":45}}"#),
        (ServerFrame::end(id(3)), r#"{"id":3,"end":true}"#),
        (
            error,
            r#"{"id":3,"error":{"code":"overflow","message":"the subscriber fell behind; resume after seq 41","data":{"last_seq":41}}}"#,
        ),
        (ServerFrame::Ack { id: id(3) }, r#"{"ack":3}"#),
    ];
    for (frame, text) in cases {
        assert_eq!(serde_json::to_string(&frame).unwrap(), text);
        assert_eq!(ServerFrame::from_json(text.as_bytes()).unwrap(), frame);
    }
}

#[test]
fn an_error_without_an_id_reads_back() {
    let text = br#"{"error":{"code":"invalid","message":"not a frame"}}"#;
    let frame = ServerFrame::from_json(text).unwrap();
    assert_eq!(
        frame,
        ServerFrame::Error(ErrorFrame {
            id: None,
            error: ErrorBody::new(ErrorCode::Invalid, "not a frame"),
        })
    );
    assert_eq!(frame.request_id(), None);
}

#[test]
fn every_frame_with_an_id_reports_it() {
    for frame in [
        ServerFrame::end(id(5)),
        ServerFrame::Ack { id: id(5) },
        ServerFrame::Item { id: id(5), item: json!(null) },
        ServerFrame::error(Some(id(5)), ErrorBody::new(ErrorCode::Busy, "busy")),
    ] {
        assert_eq!(frame.request_id(), Some(id(5)));
    }
}

#[test]
fn a_server_frame_needs_exactly_one_shape() {
    for text in [
        r#"{"id":1}"#,
        r#"{"id":1,"item":{},"end":true}"#,
        r#"{"ack":1,"error":{"code":"busy","message":"busy"}}"#,
    ] {
        assert!(ServerFrame::from_json(text.as_bytes()).is_err(), "{text}");
    }
}

#[test]
fn end_must_be_true() {
    assert!(ServerFrame::from_json(br#"{"id":1,"end":false}"#).is_err());
    assert!(ServerFrame::from_json(br#"{"id":1,"end":null}"#).is_err());
}

#[test]
fn an_item_without_an_id_keeps_no_id_in_the_error() {
    let err = ServerFrame::from_json(br#"{"item":{}}"#).unwrap_err();
    assert!(matches!(err, ProtocolError::Decode { id: None, .. }));
}

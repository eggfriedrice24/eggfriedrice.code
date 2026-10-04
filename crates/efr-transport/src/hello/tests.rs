use efr_protocol::{
    AdminStatus, Capabilities, ConversationsList, ErrorBody, ErrorCode, Hello, Method, Origin,
};
use pretty_assertions::assert_eq;

use super::{Gate, gate};

const DAEMON: u32 = 1;

fn hello(protocol: u32) -> Method {
    Method::Hello(Hello {
        protocol,
        origin: Origin::Cli,
        client: Some("efr 0.1.0".to_owned()),
        capabilities: Capabilities::default(),
        tty: None,
        pid: None,
        device_id: None,
    })
}

fn list() -> Method {
    Method::ConversationsList(ConversationsList { cursor: None, limit: None })
}

fn code(gate: &Gate) -> Option<ErrorCode> {
    match gate {
        Gate::Refuse(body) | Gate::RefuseAndClose(body) => Some(body.code),
        Gate::Hello(_) | Gate::Dispatch(_) => None,
    }
}

#[test]
fn the_decision_table() {
    // (hello done, request, expected kind, expected code)
    let cases: [(bool, Method, &str, Option<ErrorCode>); 7] = [
        (false, hello(DAEMON), "hello", None),
        (false, hello(DAEMON + 1), "refuse_and_close", Some(ErrorCode::ProtocolMismatch)),
        (false, hello(0), "refuse_and_close", Some(ErrorCode::ProtocolMismatch)),
        (false, list(), "refuse", Some(ErrorCode::Unauthorized)),
        (false, Method::AdminStatus(AdminStatus {}), "refuse", Some(ErrorCode::Unauthorized)),
        (true, hello(DAEMON), "refuse", Some(ErrorCode::Conflict)),
        (true, list(), "dispatch", None),
    ];
    for (hello_done, method, kind, expected_code) in cases {
        let name = method.name();
        let decided = gate(hello_done, method, DAEMON);
        let decided_kind = match decided {
            Gate::Hello(_) => "hello",
            Gate::Dispatch(_) => "dispatch",
            Gate::Refuse(_) => "refuse",
            Gate::RefuseAndClose(_) => "refuse_and_close",
        };
        assert_eq!(
            (decided_kind, code(&decided)),
            (kind, expected_code),
            "hello done: {hello_done}, method: {name}"
        );
    }
}

#[test]
fn a_mismatch_carries_both_versions() {
    let Gate::RefuseAndClose(body) = gate(false, hello(7), DAEMON) else {
        panic!("a mismatched hello must close the connection");
    };
    assert_eq!(body, ErrorBody::protocol_mismatch(DAEMON, 7));
    assert_eq!(body.data.unwrap(), serde_json::json!({ "client": 7, "daemon": 1 }));
}

#[test]
fn hello_and_dispatch_pass_the_method_through_unchanged() {
    assert_eq!(gate(true, list(), DAEMON), Gate::Dispatch(list()));
    let Method::Hello(params) = hello(DAEMON) else { unreachable!() };
    assert_eq!(gate(false, hello(DAEMON), DAEMON), Gate::Hello(params));
}

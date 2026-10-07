use std::io;
use std::path::PathBuf;

use efr_protocol::{PtyId, Size};
use pretty_assertions::assert_eq;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use super::{
    HOLDER_PROTOCOL_VERSION, HolderErrorCode, HolderRequest, HolderResponse, RequestFrame,
    ResponseFrame, WireError,
};
use crate::{ChildStatus, HolderError, PtyInfo, Signal, SignalTarget, SpawnSpec};

const PTY: &str = "01920000-0000-7000-8000-000000000001";

fn pty_id() -> PtyId {
    PTY.parse().unwrap()
}

fn size() -> Size {
    Size { cols: 80, rows: 24 }
}

/// Encodes `value`, compares it with `wire`, and decodes `wire` back to `value`.
fn assert_wire<T>(value: &T, wire: Value)
where
    T: Serialize + DeserializeOwned + PartialEq + std::fmt::Debug,
{
    assert_eq!(serde_json::to_value(value).unwrap(), wire);
    assert_eq!(&serde_json::from_value::<T>(wire).unwrap(), value);
}

#[test]
fn requests_have_a_kind_tag() {
    let spec = SpawnSpec::new(pty_id(), "/usr/bin/zsh", "/home/user", size()).arg("-i");
    let cases = [
        (
            HolderRequest::Hello { protocol: HOLDER_PROTOCOL_VERSION },
            json!({"kind": "hello", "protocol": 1}),
        ),
        (
            HolderRequest::Spawn { spec },
            json!({"kind": "spawn", "spec": {
                "pty_id": PTY,
                "program": "/usr/bin/zsh",
                "args": ["-i"],
                "cwd": "/home/user",
                "size": {"cols": 80, "rows": 24},
            }}),
        ),
        (
            HolderRequest::Resize { pty_id: pty_id(), size: Size { cols: 100, rows: 30 } },
            json!({"kind": "resize", "pty_id": PTY, "size": {"cols": 100, "rows": 30}}),
        ),
        (
            HolderRequest::Signal {
                pty_id: pty_id(),
                signal: Signal::Interrupt,
                target: SignalTarget::ForegroundGroup,
            },
            json!({"kind": "signal", "pty_id": PTY, "signal": "interrupt", "target": "foreground_group"}),
        ),
        (HolderRequest::List, json!({"kind": "list"})),
        (
            HolderRequest::Foreground { pty_id: pty_id() },
            json!({"kind": "foreground", "pty_id": PTY}),
        ),
        (HolderRequest::Wait { pty_id: pty_id() }, json!({"kind": "wait", "pty_id": PTY})),
        (HolderRequest::Release { pty_id: pty_id() }, json!({"kind": "release", "pty_id": PTY})),
    ];
    for (request, wire) in cases {
        assert_wire(&request, wire);
    }
}

#[test]
fn responses_have_a_kind_tag() {
    let info =
        PtyInfo { pty_id: pty_id(), child_pid: 4242, size: size(), status: ChildStatus::Running };
    let cases = [
        (HolderResponse::Hello { protocol: 1 }, json!({"kind": "hello", "protocol": 1})),
        (
            HolderResponse::Spawned { pty_id: pty_id(), child_pid: 4242 },
            json!({"kind": "spawned", "pty_id": PTY, "child_pid": 4242}),
        ),
        (HolderResponse::Done, json!({"kind": "done"})),
        (
            HolderResponse::Listed { ptys: vec![info] },
            json!({"kind": "listed", "ptys": [{
                "pty_id": PTY,
                "child_pid": 4242,
                "size": {"cols": 80, "rows": 24},
                "status": {"kind": "running"},
            }]}),
        ),
        (
            HolderResponse::Foreground { pty_id: pty_id(), group: Some(4242) },
            json!({"kind": "foreground", "pty_id": PTY, "group": 4242}),
        ),
        (
            HolderResponse::Foreground { pty_id: pty_id(), group: None },
            json!({"kind": "foreground", "pty_id": PTY}),
        ),
        (
            HolderResponse::Exited { pty_id: pty_id(), status: ChildStatus::Exited { code: 2 } },
            json!({"kind": "exited", "pty_id": PTY, "status": {"kind": "exited", "code": 2}}),
        ),
        (
            HolderResponse::Exited {
                pty_id: pty_id(),
                status: ChildStatus::Signaled { signal: 9 },
            },
            json!({"kind": "exited", "pty_id": PTY, "status": {"kind": "signaled", "signal": 9}}),
        ),
        (
            HolderResponse::Error(WireError {
                code: HolderErrorCode::NotFound,
                message: "no such PTY".to_owned(),
            }),
            json!({"kind": "error", "code": "not_found", "message": "no such PTY"}),
        ),
    ];
    for (response, wire) in cases {
        assert_wire(&response, wire);
    }
}

#[test]
fn a_spawn_request_carries_the_child_subreaper() {
    let spec = SpawnSpec::new(pty_id(), "/usr/bin/zsh", "/home/user", size()).child_subreaper(true);
    assert_wire(
        &HolderRequest::Spawn { spec },
        json!({"kind": "spawn", "spec": {
            "pty_id": PTY,
            "program": "/usr/bin/zsh",
            "cwd": "/home/user",
            "size": {"cols": 80, "rows": 24},
            "child_subreaper": true,
        }}),
    );
}

#[test]
fn frames_carry_the_request_id() {
    assert_wire(
        &RequestFrame { id: 7, request: HolderRequest::List },
        json!({"id": 7, "request": {"kind": "list"}}),
    );
    assert_wire(
        &ResponseFrame { id: 7, response: HolderResponse::Done },
        json!({"id": 7, "response": {"kind": "done"}}),
    );
}

#[test]
fn unknown_fields_are_ignored_and_unknown_kinds_are_refused() {
    let request: HolderRequest =
        serde_json::from_value(json!({"kind": "release", "pty_id": PTY, "reason": "idle"}))
            .unwrap();
    assert_eq!(request, HolderRequest::Release { pty_id: pty_id() });

    assert!(serde_json::from_value::<HolderRequest>(json!({"kind": "attach"})).is_err());
    assert!(serde_json::from_value::<HolderResponse>(json!({"kind": "maybe"})).is_err());
}

#[test]
fn only_a_spawned_response_carries_a_descriptor() {
    let error = WireError { code: HolderErrorCode::Internal, message: String::new() };
    let cases = [
        (HolderResponse::Hello { protocol: 1 }, 0),
        (HolderResponse::Spawned { pty_id: pty_id(), child_pid: 1 }, 1),
        (HolderResponse::Done, 0),
        (HolderResponse::Listed { ptys: Vec::new() }, 0),
        (HolderResponse::Foreground { pty_id: pty_id(), group: Some(1) }, 0),
        (HolderResponse::Exited { pty_id: pty_id(), status: ChildStatus::Exited { code: 0 } }, 0),
        (HolderResponse::Error(error), 0),
    ];
    for (response, fds) in cases {
        assert_eq!(response.fd_count(), fds, "{response:?}");
    }
}

#[test]
fn into_result_turns_an_error_response_into_a_remote_error() {
    let ok = HolderResponse::Done.into_result().unwrap();
    assert_eq!(ok, HolderResponse::Done);

    let failed = HolderResponse::Error(WireError {
        code: HolderErrorCode::Exited,
        message: "the child has exited".to_owned(),
    })
    .into_result()
    .unwrap_err();
    assert_eq!(failed.code(), HolderErrorCode::Exited);
    assert_eq!(
        failed.to_string(),
        "the holder service refused the request (exited): the child has exited"
    );
}

#[test]
fn every_error_has_a_code() {
    let os = || io::Error::other("os");
    let cases = [
        (
            HolderError::ProgramNotAbsolute { program: PathBuf::from("zsh") },
            HolderErrorCode::InvalidSpec,
        ),
        (HolderError::CwdNotAbsolute { cwd: PathBuf::from("src") }, HolderErrorCode::InvalidSpec),
        (HolderError::EmptySize { size: Size::default() }, HolderErrorCode::InvalidSpec),
        (HolderError::InvalidEnvName { name: String::new() }, HolderErrorCode::InvalidSpec),
        (HolderError::NulInEnvValue { name: "A".to_owned() }, HolderErrorCode::InvalidSpec),
        (HolderError::NulByte { field: "cwd" }, HolderErrorCode::InvalidSpec),
        (HolderError::AlreadyExists { pty_id: pty_id() }, HolderErrorCode::AlreadyExists),
        (HolderError::NotFound { pty_id: pty_id() }, HolderErrorCode::NotFound),
        (HolderError::Exited { pty_id: pty_id() }, HolderErrorCode::Exited),
        (
            HolderError::Spawn { program: PathBuf::from("/bin/sh"), source: os() },
            HolderErrorCode::SpawnFailed,
        ),
        (HolderError::Resize { pty_id: pty_id(), source: os() }, HolderErrorCode::Os),
        (
            HolderError::Signal { pty_id: pty_id(), signal: Signal::Kill, source: os() },
            HolderErrorCode::Os,
        ),
        (HolderError::Foreground { pty_id: pty_id(), source: os() }, HolderErrorCode::Os),
        (HolderError::Release { pty_id: pty_id(), source: os() }, HolderErrorCode::Os),
        (HolderError::ProtocolMismatch { ours: 1, theirs: 2 }, HolderErrorCode::ProtocolMismatch),
        (
            HolderError::Remote { code: HolderErrorCode::Internal, message: String::new() },
            HolderErrorCode::Internal,
        ),
    ];
    for (error, code) in cases {
        assert_eq!(error.code(), code, "{error:?}");
    }
}

#[test]
fn a_local_error_goes_out_with_its_code_and_message() {
    let error = HolderError::NotFound { pty_id: pty_id() };
    let wire = WireError::from(&error);
    assert_eq!(
        wire,
        WireError {
            code: HolderErrorCode::NotFound,
            message: format!("the holder holds no PTY {PTY}"),
        }
    );
}

#[test]
fn a_remote_error_is_forwarded_without_a_second_wrapper() {
    let original = WireError { code: HolderErrorCode::SpawnFailed, message: "no zsh".to_owned() };
    let received = HolderError::from(original.clone());
    assert_eq!(WireError::from(&received), original);
}

#[test]
fn error_codes_display_their_wire_names() {
    let codes = [
        HolderErrorCode::InvalidSpec,
        HolderErrorCode::AlreadyExists,
        HolderErrorCode::NotFound,
        HolderErrorCode::Exited,
        HolderErrorCode::SpawnFailed,
        HolderErrorCode::Os,
        HolderErrorCode::ProtocolMismatch,
        HolderErrorCode::Internal,
    ];
    for code in codes {
        assert_eq!(serde_json::to_value(code).unwrap(), json!(code.to_string()));
    }
}

#[test]
fn a_protocol_mismatch_names_both_versions() {
    let error = HolderError::ProtocolMismatch { ours: 1, theirs: 2 };
    assert_eq!(error.to_string(), "the holder service speaks protocol 2, this build speaks 1");
}

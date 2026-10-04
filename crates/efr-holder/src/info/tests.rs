use efr_protocol::Size;
use pretty_assertions::assert_eq;
use serde_json::json;

use super::{ChildStatus, PtyInfo};

const PTY: &str = "01920000-0000-7000-8000-000000000001";

#[test]
fn status_reports_running_and_exit_codes() {
    let cases = [
        (ChildStatus::Running, true, None),
        (ChildStatus::Exited { code: 0 }, false, Some(0)),
        (ChildStatus::Exited { code: 130 }, false, Some(130)),
        (ChildStatus::Signaled { signal: 9 }, false, None),
    ];
    for (status, running, code) in cases {
        assert_eq!(status.is_running(), running, "{status:?}");
        assert_eq!(status.exit_code(), code, "{status:?}");
    }
}

#[test]
fn status_has_a_kind_tag_on_the_wire() {
    let cases = [
        (ChildStatus::Running, json!({"kind": "running"})),
        (ChildStatus::Exited { code: 1 }, json!({"kind": "exited", "code": 1})),
        (ChildStatus::Signaled { signal: 15 }, json!({"kind": "signaled", "signal": 15})),
    ];
    for (status, wire) in cases {
        assert_eq!(serde_json::to_value(status).unwrap(), wire);
        assert_eq!(serde_json::from_value::<ChildStatus>(wire).unwrap(), status);
    }
}

#[test]
fn info_round_trips_through_json() {
    let info = PtyInfo {
        pty_id: PTY.parse().unwrap(),
        child_pid: 4242,
        size: Size { cols: 120, rows: 40 },
        status: ChildStatus::Exited { code: 0 },
    };
    let wire = json!({
        "pty_id": PTY,
        "child_pid": 4242,
        "size": {"cols": 120, "rows": 40},
        "status": {"kind": "exited", "code": 0},
    });
    assert_eq!(serde_json::to_value(info).unwrap(), wire);
    assert_eq!(serde_json::from_value::<PtyInfo>(wire).unwrap(), info);
}

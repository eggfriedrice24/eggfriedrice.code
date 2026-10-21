use pretty_assertions::assert_eq;
use serde_json::json;

use super::{ForwardOutcome, HeldReason, JobId, JobState, JobStopBy, NoticeKind, PortForward};

#[test]
fn forward_outcome_reads_unknown_value() {
    for outcome in ForwardOutcome::ALL {
        let text = serde_json::to_value(outcome).unwrap();
        assert_eq!(text, json!(outcome.as_str()));
        assert_eq!(serde_json::from_value::<ForwardOutcome>(text).unwrap(), outcome);
    }
    let later: ForwardOutcome = serde_json::from_value(json!("remapped")).unwrap();
    assert_eq!(later, ForwardOutcome::Unknown);
    let forward = PortForward { listen: "::1".to_owned(), port: 5173 };
    assert_eq!(serde_json::to_value(&forward).unwrap(), json!({ "listen": "::1", "port": 5173 }));
}

#[test]
fn job_id_round_trip_and_refuses_bad_ids() {
    for n in [1, 3, 10, 999_999_999] {
        let id = JobId::nth(n);
        assert_eq!(id.as_str(), format!("j{n}"));
        assert_eq!(id.number(), Some(n));
        let text = serde_json::to_string(&id).unwrap();
        assert_eq!(serde_json::from_str::<JobId>(&text).unwrap(), id);
        assert_eq!(id.to_string().parse::<JobId>().unwrap(), id);
    }
    assert_eq!(JobId::nth(0), JobId::nth(1), "the first job is j1");
    for bad in ["", "j", "j0", "j01", "J3", "3", "j3 ", "j-3", "j1234567890", "j3a", "jj3", "j３"]
    {
        assert!(serde_json::from_value::<JobId>(json!(bad)).is_err(), "{bad:?} is refused");
        assert!(bad.parse::<JobId>().is_err(), "{bad:?} does not parse");
    }
}

#[test]
fn job_state_reads_unknown_value() {
    for state in JobState::ALL {
        let text = serde_json::to_value(state).unwrap();
        assert_eq!(text, json!(state.as_str()));
        assert_eq!(serde_json::from_value::<JobState>(text).unwrap(), state);
    }
    let later: JobState = serde_json::from_value(json!("paused")).unwrap();
    assert_eq!(later, JobState::Unknown);
    let later: JobStopBy = serde_json::from_value(json!("conversation_end")).unwrap();
    assert_eq!(later, JobStopBy::Unknown);
    for stop in JobStopBy::ALL {
        assert_eq!(serde_json::to_value(stop).unwrap(), json!(stop.as_str()));
    }
    assert!(serde_json::from_value::<JobState>(json!(3)).is_err(), "a number is no state");
    assert!(JobState::TimedOut.has_ended() && !JobState::Running.has_ended());
}

#[test]
fn held_reason_reads_unknown_value() {
    for reason in HeldReason::ALL {
        let text = serde_json::to_value(reason).unwrap();
        assert_eq!(text, json!(reason.as_str()));
        assert_eq!(serde_json::from_value::<HeldReason>(text).unwrap(), reason);
    }
    let later: HeldReason = serde_json::from_value(json!("quiet_hours")).unwrap();
    assert_eq!(later, HeldReason::Unknown);
    assert_eq!(serde_json::to_value(NoticeKind::Watch).unwrap(), json!("watch"));
}

/// An older client reads a `job_notice` of a kind that a newer efrd added: the event
/// still parses.
#[test]
fn notice_kind_reads_unknown_value() {
    let later: NoticeKind = serde_json::from_value(json!("guard")).unwrap();
    assert_eq!(later, NoticeKind::Unknown);
    for kind in NoticeKind::ALL {
        let text = serde_json::to_value(kind).unwrap();
        assert_eq!(serde_json::from_value::<NoticeKind>(text).unwrap(), kind);
    }
}

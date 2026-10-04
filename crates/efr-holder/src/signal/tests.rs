use pretty_assertions::assert_eq;
use serde_json::json;

use super::{Signal, SignalTarget};

#[test]
fn signals_have_wire_names_and_display_names() {
    let cases = [
        (Signal::Hangup, "hangup", "SIGHUP"),
        (Signal::Interrupt, "interrupt", "SIGINT"),
        (Signal::Quit, "quit", "SIGQUIT"),
        (Signal::Terminate, "terminate", "SIGTERM"),
        (Signal::Kill, "kill", "SIGKILL"),
    ];
    for (signal, wire, display) in cases {
        assert_eq!(serde_json::to_value(signal).unwrap(), json!(wire));
        assert_eq!(serde_json::from_value::<Signal>(json!(wire)).unwrap(), signal);
        assert_eq!(signal.to_string(), display);
    }
}

#[test]
fn targets_have_wire_names() {
    let cases =
        [(SignalTarget::Child, "child"), (SignalTarget::ForegroundGroup, "foreground_group")];
    for (target, wire) in cases {
        assert_eq!(serde_json::to_value(target).unwrap(), json!(wire));
        assert_eq!(serde_json::from_value::<SignalTarget>(json!(wire)).unwrap(), target);
    }
}

#[test]
fn unknown_signals_are_rejected() {
    assert!(serde_json::from_value::<Signal>(json!("SIGHUP")).is_err());
    assert!(serde_json::from_value::<Signal>(json!(9)).is_err());
}

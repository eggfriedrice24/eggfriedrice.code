use pretty_assertions::assert_eq;
use serde_json::json;

use crate::{
    BusKind, CacheMode, EffectiveSettings, Event, ExitKind, Grant, Launch, Mode, ModeFallback,
    Needs, NetworkMode, ProtocolError, SandboxStatus, SandboxSummary,
};

#[test]
fn a_routine_contained_launch_leaves_out_its_empty_grants() {
    assert_eq!(serde_json::to_value(Launch::contained()).unwrap(), json!({ "kind": "contained" }));
    assert_eq!(serde_json::to_value(Launch::Direct).unwrap(), json!({ "kind": "direct" }));
    let back: Launch = serde_json::from_value(json!({ "kind": "contained" })).unwrap();
    assert_eq!(back, Launch::contained());
}

#[test]
fn a_launch_names_its_grants_and_whether_it_uses_the_launcher() {
    let open = Launch::Contained { grants: vec![Grant::OpenNetwork] };
    assert_eq!(open.grants(), [Grant::OpenNetwork]);
    assert!(open.uses_launcher());
    assert!(Launch::Unsandboxed.uses_launcher());
    assert!(!Launch::Direct.uses_launcher());
    assert!(Launch::Unsandboxed.grants().is_empty());
}

#[test]
fn grants_are_tagged_by_kind() {
    assert_eq!(
        serde_json::to_value(Grant::Bus { bus: BusKind::Session }).unwrap(),
        json!({ "kind": "bus", "bus": "session" })
    );
    assert_eq!(
        serde_json::to_value(Grant::Host { host: "crates.io".to_owned(), port: 443 }).unwrap(),
        json!({ "kind": "host", "host": "crates.io", "port": 443 })
    );
    assert_eq!(Grant::Unmask { path: "/p/.env".into() }.path(), Some("/p/.env".as_ref()));
    assert_eq!(Grant::OpenNetwork.path(), None);
}

#[test]
fn every_exit_kind_reads_back_from_its_wire_name() {
    for kind in ExitKind::ALL {
        let value = serde_json::to_value(kind).unwrap();
        assert_eq!(value, json!(kind.as_str()));
        assert_eq!(kind.to_string(), kind.as_str());
        assert_eq!(serde_json::from_value::<ExitKind>(value).unwrap(), kind);
    }
}

#[test]
fn floors_are_never_user_only_or_unsandboxed() {
    for kind in ExitKind::ALL.into_iter().filter(|kind| kind.is_floor()) {
        assert!(!kind.user_only(), "{kind}");
        assert!(!kind.runs_unsandboxed(), "{kind}");
    }
    assert_eq!(
        ExitKind::ALL.into_iter().filter(|kind| kind.is_floor()).collect::<Vec<_>>(),
        [ExitKind::Secret, ExitKind::Config]
    );
}

#[test]
fn privilege_upload_and_persistence_are_user_only_and_run_outside() {
    for kind in [ExitKind::Privilege, ExitKind::Upload, ExitKind::Persistence] {
        assert!(kind.user_only(), "{kind}");
        assert!(kind.runs_unsandboxed(), "{kind}");
    }
    assert!(ExitKind::MaskedRead.user_only());
    assert!(!ExitKind::MaskedRead.runs_unsandboxed());
    assert!(!ExitKind::Write.user_only());
    assert!(!ExitKind::Destructive.runs_unsandboxed());
    // The classifier may judge an outside exit whose line touches no write root.
    assert!(!ExitKind::Outside.user_only());
    assert!(ExitKind::Outside.runs_unsandboxed());
}

#[test]
fn needs_leave_out_empty_members_and_a_reason_alone_asks_for_nothing() {
    assert_eq!(serde_json::to_value(Needs::default()).unwrap(), json!({}));
    let reason = Needs { reason: Some("the build needs it".to_owned()), ..Needs::default() };
    assert!(reason.is_empty());
    let outside = Needs { outside: true, ..Needs::default() };
    assert!(!outside.is_empty());
    assert_eq!(serde_json::to_value(&outside).unwrap(), json!({ "outside": true }));
    let back: Needs =
        serde_json::from_value(json!({ "write": ["~/notes"], "bus": "system" })).unwrap();
    assert_eq!(back.write, ["~/notes"]);
    assert_eq!(back.bus, Some(BusKind::System));
}

#[test]
fn needs_check_holds_the_limits_of_the_schema() {
    let too_many = Needs { sockets: vec!["/a".to_owned(); 3], ..Needs::default() };
    assert!(matches!(
        too_many.check(),
        Err(ProtocolError::InvalidNeeds { field: "sockets", max: 2 })
    ));
    let long = Needs { reason: Some("x".repeat(301)), ..Needs::default() };
    assert!(matches!(long.check(), Err(ProtocolError::InvalidNeeds { field: "reason", .. })));
    let fine = Needs {
        write: vec!["/a".to_owned(); 8],
        hosts: vec!["h".to_owned(); 8],
        unmask: vec!["/b".to_owned(); 4],
        reason: Some("é".repeat(300)),
        ..Needs::default()
    };
    assert!(fine.check().is_ok());
}

#[test]
fn the_needs_input_schema_is_closed_and_names_every_member() {
    let schema = Needs::input_schema();
    assert_eq!(schema["additionalProperties"], json!(false));
    let names: Vec<&str> =
        schema["properties"].as_object().unwrap().keys().map(String::as_str).collect();
    let mut expected =
        vec!["write", "hosts", "sockets", "bus", "device", "unmask", "outside", "reason"];
    let mut sorted = names.clone();
    sorted.sort_unstable();
    expected.sort_unstable();
    assert_eq!(sorted, expected);
    assert_eq!(schema["properties"]["sockets"]["maxItems"], json!(2));
    assert_eq!(schema["properties"]["reason"]["maxLength"], json!(300));
}

#[test]
fn a_status_before_the_first_probe_is_unavailable_with_a_reason() {
    let status = SandboxStatus::unavailable("the sandbox was not probed yet");
    assert!(!status.available);
    assert_eq!(
        serde_json::to_value(&status).unwrap(),
        json!({
            "available": false,
            "reason": "the sandbox was not probed yet",
            "cache_mode": "overlay",
            "network_mode": "none",
        })
    );
    assert_eq!(CacheMode::Readonly.as_str(), "readonly");
    assert_eq!(serde_json::to_value(CacheMode::Readonly).unwrap(), json!("readonly"));
    assert_eq!(NetworkMode::Proxy.as_str(), "proxy");
}

#[test]
fn an_empty_summary_holds_only_whether_it_was_confined() {
    let summary = SandboxSummary { confined: true, ..SandboxSummary::default() };
    assert_eq!(serde_json::to_value(&summary).unwrap(), json!({ "confined": true }));
}

#[test]
fn events_from_before_the_sandbox_still_load() {
    let old = json!({
        "kind": "tool_call_completed",
        "turn_id": "019a9b1c-3d00-7a10-8b20-000000000002",
        "call_id": "019a9b1c-3d00-7a10-8b20-000000000004",
        "output": "ok",
        "truncated": false,
        "is_error": false,
    });
    let event: Event = serde_json::from_value(old.clone()).unwrap();
    assert!(matches!(event, Event::ToolCallCompleted { sandbox: None, .. }));
    assert_eq!(serde_json::to_value(&event).unwrap(), old);
}

#[test]
fn a_fallback_says_which_mode_was_asked_for_and_why() {
    let settings = EffectiveSettings {
        mode: Mode::Cautious,
        model: "gpt-5.5".to_owned(),
        effort: None,
        overridden: Default::default(),
        fallback: Some(ModeFallback {
            asked: Mode::Auto,
            reason: "auto cannot use your home directory as a project".to_owned(),
        }),
    };
    assert_eq!(
        serde_json::to_value(&settings).unwrap(),
        json!({
            "mode": "cautious",
            "model": "gpt-5.5",
            "fallback": {
                "asked": "auto",
                "reason": "auto cannot use your home directory as a project",
            },
        })
    );
}

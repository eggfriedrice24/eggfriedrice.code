use pretty_assertions::assert_eq;
use serde_json::json;

use super::{AttachHow, NewReason, RouteScope, Switch};

#[test]
fn a_route_scope_is_adjacently_tagged_like_a_scope() {
    let project: RouteScope = serde_json::from_value(
        json!({"kind": "project", "value": "019a9b1c-3d00-7a10-8b20-000000000008"}),
    )
    .unwrap();
    assert!(matches!(project, RouteScope::Project(_)));
    assert_eq!(serde_json::to_value(RouteScope::Outside).unwrap(), json!({"kind": "outside"}));
    let outside: RouteScope = serde_json::from_value(json!({"kind": "outside"})).unwrap();
    assert_eq!(outside, RouteScope::Outside);
}

#[test]
fn an_unknown_route_scope_reads_as_unknown_with_or_without_a_value() {
    let bare: RouteScope = serde_json::from_value(json!({"kind": "galaxy"})).unwrap();
    assert_eq!(bare, RouteScope::Unknown);
    let valued: Result<RouteScope, serde_json::Error> =
        serde_json::from_value(json!({"kind": "galaxy", "value": {"far": true}}));
    assert_eq!(valued.unwrap(), RouteScope::Unknown);
    let broken: Result<RouteScope, _> = serde_json::from_value(json!({"kind": "project"}));
    assert!(broken.is_err(), "a project needs its id");
}

#[test]
fn unknown_switches_reasons_and_ways_read_as_unknown() {
    let switch: Switch = serde_json::from_value(json!({"kind": "teleported"})).unwrap();
    assert_eq!(switch, Switch::Unknown);
    let reason: NewReason = serde_json::from_value(json!({"kind": "asked"})).unwrap();
    assert_eq!(reason, NewReason::Unknown);
    let how: AttachHow = serde_json::from_value(json!("borrowed")).unwrap();
    assert_eq!(how, AttachHow::Unknown);
}

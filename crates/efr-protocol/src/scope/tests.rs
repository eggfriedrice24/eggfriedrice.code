use std::collections::BTreeSet;
use std::path::PathBuf;
use std::str::FromStr;

use pretty_assertions::assert_eq;
use serde_json::json;

use crate::{Origin, ProjectId, Scope, ScopeName};

const PROJECT: &str = "01928c4e-7a3b-7c1d-8e2f-0123456789ab";

#[test]
fn machine_scope_is_a_bare_kind() {
    assert_eq!(serde_json::to_value(Scope::Machine).unwrap(), json!({ "kind": "machine" }));
}

#[test]
fn path_scope_carries_the_path_as_its_value() {
    let scope = Scope::Path(PathBuf::from("/etc/nixos"));
    assert_eq!(
        serde_json::to_value(&scope).unwrap(),
        json!({ "kind": "path", "value": "/etc/nixos" })
    );
}

#[test]
fn project_scope_carries_the_project_id_as_its_value() {
    let scope = Scope::Project(ProjectId::from_str(PROJECT).unwrap());
    assert_eq!(
        serde_json::to_value(&scope).unwrap(),
        json!({ "kind": "project", "value": PROJECT })
    );
}

#[test]
fn every_scope_reads_back_as_itself() {
    let scopes = [
        Scope::Machine,
        Scope::Path(PathBuf::from("/home/me/p/foo")),
        Scope::Project(ProjectId::from_str(PROJECT).unwrap()),
    ];
    for scope in scopes {
        let back: Scope = serde_json::from_value(serde_json::to_value(&scope).unwrap()).unwrap();
        assert_eq!(back, scope);
    }
}

#[test]
fn a_scope_of_an_unknown_kind_is_rejected() {
    assert!(serde_json::from_value::<Scope>(json!({ "kind": "galaxy" })).is_err());
}

#[test]
fn origins_are_snake_case_names() {
    let names: Vec<_> = [Origin::Shell, Origin::Cli, Origin::Proxy, Origin::Phone]
        .into_iter()
        .map(|origin| serde_json::to_value(origin).unwrap())
        .collect();
    assert_eq!(names, [json!("shell"), json!("cli"), json!("proxy"), json!("phone")]);
}

#[test]
fn every_scope_name_serializes_as_its_wire_name() {
    for scope in ScopeName::ALL {
        assert_eq!(serde_json::to_value(scope).unwrap(), json!(scope.as_str()));
        assert_eq!(scope.to_string(), scope.as_str());
        let back: ScopeName = serde_json::from_value(json!(scope.as_str())).unwrap();
        assert_eq!(back, scope);
    }
}

#[test]
fn there_are_exactly_the_five_scopes() {
    let names: BTreeSet<_> = ScopeName::ALL.iter().map(|scope| scope.as_str()).collect();
    assert_eq!(names, BTreeSet::from(["admin", "approve", "operate", "read", "terminal"]));
}

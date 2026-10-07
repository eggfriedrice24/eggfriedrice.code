use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};

use crate::testing::{TestCall, private_file};

use super::*;

fn problem(result: Result<CallDir, SbxError>) -> CallDirProblem {
    match result {
        Err(SbxError::CallDir { problem, .. }) => problem,
        other => panic!("expected a call dir problem, got {other:?}"),
    }
}

#[test]
fn a_call_dir_that_efrd_made_opens() {
    let call = TestCall::new();
    let opened = CallDir::open(call.call_dir()).unwrap();
    assert_eq!(opened.spec, call.spec);
}

#[test]
fn a_relative_or_unnormal_path_is_refused() {
    assert_eq!(problem(CallDir::open(Path::new("r/sbx/x"))), CallDirProblem::NotAbsolute);
    assert_eq!(problem(CallDir::open(Path::new("/tmp/../tmp/x"))), CallDirProblem::NotAbsolute);
}

#[test]
fn a_linked_call_dir_is_refused() {
    let call = TestCall::new();
    let link = call.root.join("link");
    symlink(call.call_dir(), &link).unwrap();
    assert_eq!(problem(CallDir::open(&link)), CallDirProblem::Link);
}

#[test]
fn a_call_dir_open_to_others_is_refused() {
    let call = TestCall::new();
    fs::set_permissions(call.call_dir(), fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(problem(CallDir::open(call.call_dir())), CallDirProblem::Mode);
}

#[test]
fn a_spec_open_to_others_is_refused() {
    let call = TestCall::new();
    let spec = call.call_dir().join(SPEC_FILE);
    fs::set_permissions(&spec, fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(problem(CallDir::open(call.call_dir())), CallDirProblem::Mode);
}

#[test]
fn a_linked_spec_is_refused() {
    let call = TestCall::new();
    let spec = call.call_dir().join(SPEC_FILE);
    let elsewhere = call.root.join("spec.json");
    fs::rename(&spec, &elsewhere).unwrap();
    symlink(&elsewhere, &spec).unwrap();
    assert_eq!(problem(CallDir::open(call.call_dir())), CallDirProblem::Link);
}

#[test]
fn a_missing_line_is_refused() {
    let call = TestCall::new();
    fs::remove_file(call.call_dir().join(LINE_FILE)).unwrap();
    assert_eq!(problem(CallDir::open(call.call_dir())), CallDirProblem::File);
}

#[test]
fn a_spec_of_another_call_dir_is_refused() {
    let call = TestCall::new();
    let other = call.spec.runtime.shell_dir.join("0192f0c1-0000-7000-8000-0000000000ff");
    crate::testing::private_dir(&other);
    private_file(&other.join(SPEC_FILE), &call.spec.to_json().unwrap());
    private_file(&other.join(LINE_FILE), b"true\n");
    assert_eq!(problem(CallDir::open(&other)), CallDirProblem::Ids);
}

#[test]
fn write_atomic_replaces_with_a_private_file() {
    let call = TestCall::new();
    let dir = PrivateDir::open(call.call_dir()).unwrap();
    dir.write_atomic("result.json", b"one").unwrap();
    dir.write_atomic("result.json", b"two").unwrap();
    let path = call.call_dir().join("result.json");
    assert_eq!(fs::read(&path).unwrap(), b"two");
    assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
    assert!(!call.call_dir().join(".result.json.tmp").exists());
}

#[test]
fn write_atomic_never_follows_a_planted_link() {
    let call = TestCall::new();
    let dir = PrivateDir::open(call.call_dir()).unwrap();
    let target = call.root.join("target");
    symlink(&target, call.call_dir().join(".apply.tmp")).unwrap();
    dir.write_atomic("apply", b"x").unwrap();
    assert!(!target.exists());
}

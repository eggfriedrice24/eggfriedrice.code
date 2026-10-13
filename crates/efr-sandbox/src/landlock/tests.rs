use std::path::{Path, PathBuf};

use efr_protocol::Grant;
use pretty_assertions::assert_eq;

use crate::landlock::{FsAccess, LandlockPolicy, LandlockScope};
use crate::plan::MountPlan;
use crate::testing::{PROJECT, spec, world};

#[test]
fn landlock_policy_has_no_resolve_unix_in_phase1() {
    let policy = MountPlan::build(&spec(), &world()).unwrap().landlock();
    assert!(policy.resolve_unix.is_empty());
    for rule in policy.rules() {
        assert!(!rule.access.contains(&FsAccess::ResolveUnix), "{rule:?}");
    }
    assert!(FsAccess::HANDLED.contains(&FsAccess::ResolveUnix));
    assert!(!FsAccess::WRITE_DIR.contains(&FsAccess::ResolveUnix));
    assert!(!FsAccess::WRITE_DIR.contains(&FsAccess::IoctlDev));
    assert!(!FsAccess::WRITE_DIR.contains(&FsAccess::MakeChar));
    assert!(!FsAccess::WRITE_DIR.contains(&FsAccess::MakeBlock));
    assert_eq!(policy.scopes, [LandlockScope::AbstractUnixSocket, LandlockScope::Signal]);
    assert_eq!(policy.min_abi, 9);
}

#[test]
fn netview_policy_has_deny_tcp() {
    let contained = MountPlan::build(&spec(), &world()).unwrap().landlock();
    assert!(!contained.deny_tcp, "phase 1 keeps TCP for loopback servers");
    let mut viewed = spec();
    viewed.grants.push(Grant::NetView);
    let policy = MountPlan::build(&viewed, &world()).unwrap().landlock();
    assert!(policy.deny_tcp);
    // The rest of the rule set is the one of the call without the view.
    assert_eq!(LandlockPolicy { deny_tcp: false, ..policy.clone() }, contained);
    assert_eq!(policy.scopes, [LandlockScope::AbstractUnixSocket, LandlockScope::Signal]);
    // An open network keeps TCP: the network view's layers do not apply there.
    viewed.grants.push(Grant::OpenNetwork);
    assert!(!MountPlan::build(&viewed, &world()).unwrap().landlock().deny_tcp);
    // A policy of an older launcher, with no such field, reads as false.
    let mut json = serde_json::to_value(&contained).unwrap();
    json.as_object_mut().unwrap().remove("deny_tcp");
    assert_eq!(serde_json::from_value::<LandlockPolicy>(json).unwrap(), contained);
}

#[test]
fn a_socket_grant_is_the_only_resolve_unix_rule() {
    let mut spec = spec();
    spec.grants.push(Grant::Socket { path: "/run/user/1000/bus".into() });
    let policy = MountPlan::build(&spec, &world()).unwrap().landlock();
    assert_eq!(policy.resolve_unix, [PathBuf::from("/run/user/1000/bus")]);
}

#[test]
fn write_rules_cover_the_roots_the_private_tmp_and_the_caches_once() {
    let spec = spec();
    let policy = MountPlan::build(&spec, &world()).unwrap().landlock();
    let mut write = policy.write.clone();
    write.sort();
    let mut expected: Vec<PathBuf> = vec![
        PROJECT.into(),
        spec.runtime.scratch.clone(),
        "/home/u/.cargo".into(),
        "/tmp".into(),
        "/var/tmp".into(),
        "/dev/shm".into(),
    ];
    expected.sort();
    assert_eq!(write, expected);
    assert_eq!(policy.read_exec, [PathBuf::from("/")]);
    assert!(policy.devices.contains(&PathBuf::from("/dev/pts")));
    assert_eq!(policy.tty_fds, [0, 1, 2]);
}

#[test]
fn a_file_grant_gets_file_rights_only() {
    let mut spec = spec();
    spec.grants.push(Grant::Write { path: "/home/u/notes.txt".into() });
    let mut fs = world();
    fs.file("/home/u/notes.txt", "");
    let policy = MountPlan::build(&spec, &fs).unwrap().landlock();
    assert_eq!(policy.write_files, [PathBuf::from("/home/u/notes.txt")]);
    let rule = policy
        .rules()
        .into_iter()
        .find(|rule| rule.path == Path::new("/home/u/notes.txt"))
        .unwrap();
    assert_eq!(rule.access, FsAccess::WRITE_FILE);
}

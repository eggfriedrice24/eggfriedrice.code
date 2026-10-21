use std::path::{Path, PathBuf};

use efr_protocol::Grant;
use pretty_assertions::assert_eq;

use crate::landlock::{FsAccess, LandlockPolicy, LandlockScope};
use crate::plan::MountPlan;
use crate::spec::NetworkPlan;
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
    viewed.network = NetworkPlan::Open;
    assert!(!MountPlan::build(&viewed, &world()).unwrap().landlock().deny_tcp);
    // A policy of an older launcher, with no such field, reads as false.
    let mut json = serde_json::to_value(&contained).unwrap();
    json.as_object_mut().unwrap().remove("deny_tcp");
    assert_eq!(serde_json::from_value::<LandlockPolicy>(json).unwrap(), contained);
}

#[test]
fn a_socket_grant_is_the_only_resolve_unix_rule() {
    let mut spec = spec();
    spec.grants.push(Grant::Socket { path: "/run/user/1000/mpd/socket".into() });
    let mut world = world();
    world.socket("/run/user/1000/mpd/socket");
    let policy = MountPlan::build(&spec, &world).unwrap().landlock();
    assert_eq!(policy.resolve_unix, [PathBuf::from("/run/user/1000/mpd/socket")]);
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

#[test]
fn bus_proxy_landlock_policy_shape() {
    let program = Path::new("/usr/bin/xdg-dbus-proxy");
    let dir = Path::new("/run/user/1000/efr/sbx/c/call/bus");
    let upstream =
        [PathBuf::from("/run/dbus/system_bus_socket"), PathBuf::from("/run/user/1000/bus")];
    let policy = LandlockPolicy::bus_proxy(program, dir, &upstream);
    let rules = policy.rules();
    // Read and run: the library dirs, the program and the loader's cache, each file
    // with file rights only; never `/`.
    let reads: Vec<(&Path, &[FsAccess])> = rules
        .iter()
        .filter(|rule| rule.access.contains(&FsAccess::ReadFile))
        .map(|rule| (rule.path.as_path(), rule.access.as_slice()))
        .collect();
    assert_eq!(
        reads,
        [
            (Path::new("/usr/lib"), &FsAccess::READ_EXEC[..]),
            (Path::new("/usr/lib64"), &FsAccess::READ_EXEC[..]),
            (Path::new("/lib"), &FsAccess::READ_EXEC[..]),
            (Path::new("/lib64"), &FsAccess::READ_EXEC[..]),
            (program, &FsAccess::READ_FILE[..]),
            (Path::new("/etc/ld.so.cache"), &FsAccess::READ_FILE[..]),
        ]
    );
    // Connect: the two buses; make, remove and connect: the socket dir only.
    let sockets: Vec<(&Path, &[FsAccess])> = rules
        .iter()
        .filter(|rule| !rule.access.contains(&FsAccess::ReadFile))
        .map(|rule| (rule.path.as_path(), rule.access.as_slice()))
        .collect();
    assert_eq!(
        sockets,
        [
            (upstream[0].as_path(), &FsAccess::SOCKET[..]),
            (upstream[1].as_path(), &FsAccess::SOCKET[..]),
            (dir, &FsAccess::SOCKET_DIR[..]),
        ]
    );
    for rule in &rules {
        for right in [
            FsAccess::WriteFile,
            FsAccess::MakeReg,
            FsAccess::MakeDir,
            FsAccess::MakeSym,
            FsAccess::Truncate,
            FsAccess::IoctlDev,
        ] {
            assert!(!rule.access.contains(&right), "{rule:?}");
        }
    }
    assert!(policy.deny_tcp && policy.tty_fds.is_empty() && policy.dev_rw.is_empty());
    assert_eq!(policy.scopes, [LandlockScope::AbstractUnixSocket, LandlockScope::Signal]);
    let text = serde_json::to_string(&policy).unwrap();
    assert_eq!(serde_json::from_str::<LandlockPolicy>(&text).unwrap(), policy);
    // No call gets a socket dir rule.
    assert!(MountPlan::build(&spec(), &world()).unwrap().landlock().socket_dirs.is_empty());
    assert!(LandlockPolicy::bridge(dir).socket_dirs.is_empty());
}

#[test]
fn bridge_landlock_policy_shape() {
    let socket = Path::new("/run/user/1000/efr-proxy.sock");
    let policy = LandlockPolicy::bridge(socket);
    // One rule: RESOLVE_UNIX on the socket file itself; no read, write or exec right
    // anywhere, no terminal, and TCP bind and connect handled with no rule.
    let rules = policy.rules();
    assert_eq!(rules.len(), 1, "{rules:?}");
    assert_eq!(rules[0].path, socket);
    assert_eq!(rules[0].access, [FsAccess::ResolveUnix]);
    for rule in &rules {
        for right in
            [FsAccess::WriteFile, FsAccess::MakeReg, FsAccess::RemoveFile, FsAccess::Truncate]
        {
            assert!(!rule.access.contains(&right), "{rule:?}");
        }
    }
    assert!(policy.read_exec.is_empty() && policy.tty_fds.is_empty());
    assert!(policy.deny_tcp);
    assert_eq!(policy.scopes, [LandlockScope::AbstractUnixSocket, LandlockScope::Signal]);
    let call = MountPlan::build(&spec(), &world()).unwrap().landlock();
    assert_eq!((policy.min_abi, policy.required_errata), (call.min_abi, call.required_errata));
    let text = serde_json::to_string(&policy).unwrap();
    assert_eq!(serde_json::from_str::<LandlockPolicy>(&text).unwrap(), policy);
}

#[test]
fn forward_landlock_policy_handles_tcp_bind_only() {
    let socket = Path::new("/run/user/1000/efr-forward.sock");
    let policy = LandlockPolicy::forward(socket);
    // One rule: RESOLVE_UNIX on the socket; no file right anywhere, no terminal.
    let rules = policy.rules();
    assert_eq!(rules.len(), 1, "{rules:?}");
    assert_eq!(rules[0].path, socket);
    assert_eq!(rules[0].access, [FsAccess::ResolveUnix]);
    assert!(policy.read_exec.is_empty() && policy.tty_fds.is_empty());
    // The bind right is handled with no rule; connect stays, for the job's servers.
    assert!(policy.deny_tcp_bind);
    assert!(!policy.deny_tcp);
    assert_eq!(policy.scopes, [LandlockScope::AbstractUnixSocket, LandlockScope::Signal]);
    let call = MountPlan::build(&spec(), &world()).unwrap().landlock();
    assert_eq!((policy.min_abi, policy.required_errata), (call.min_abi, call.required_errata));
    let text = serde_json::to_string(&policy).unwrap();
    assert_eq!(serde_json::from_str::<LandlockPolicy>(&text).unwrap(), policy);
    // No other domain handles the bind right alone, and a policy of an older build
    // reads with it off.
    assert!(!call.deny_tcp_bind && !LandlockPolicy::bridge(socket).deny_tcp_bind);
    let mut old: serde_json::Value = serde_json::to_value(&call).unwrap();
    old.as_object_mut().unwrap().remove("deny_tcp_bind");
    assert!(!serde_json::from_value::<LandlockPolicy>(old).unwrap().deny_tcp_bind);
}

#[test]
fn reach_landlock_policy_handles_tcp_connect_only() {
    let socket = Path::new("/run/user/1000/efr-reach.sock");
    let policy = LandlockPolicy::reach(socket);
    // One rule: RESOLVE_UNIX on the socket; no file right anywhere, no terminal.
    let rules = policy.rules();
    assert_eq!(rules.len(), 1, "{rules:?}");
    assert_eq!(rules[0].path, socket);
    assert_eq!(rules[0].access, [FsAccess::ResolveUnix]);
    assert!(policy.read_exec.is_empty() && policy.tty_fds.is_empty());
    assert!(policy.read_files.is_empty() && policy.socket_dirs.is_empty());
    // The connect right is handled with no rule; bind stays, for the ports that come
    // during the call.
    assert!(policy.deny_tcp_connect);
    assert!(!policy.deny_tcp && !policy.deny_tcp_bind);
    assert_eq!(policy.scopes, [LandlockScope::AbstractUnixSocket, LandlockScope::Signal]);
    let call = MountPlan::build(&spec(), &world()).unwrap().landlock();
    assert_eq!((policy.min_abi, policy.required_errata), (call.min_abi, call.required_errata));
    let text = serde_json::to_string(&policy).unwrap();
    assert_eq!(serde_json::from_str::<LandlockPolicy>(&text).unwrap(), policy);
    // No other domain handles the connect right alone, and a policy of an older build
    // reads with it off.
    assert!(!call.deny_tcp_connect);
    for other in [
        LandlockPolicy::bridge(socket),
        LandlockPolicy::forward(socket),
        LandlockPolicy::bus_proxy(Path::new("/usr/bin/xdg-dbus-proxy"), socket, &[]),
    ] {
        assert!(!other.deny_tcp_connect, "{other:?}");
    }
    let mut old: serde_json::Value = serde_json::to_value(&call).unwrap();
    old.as_object_mut().unwrap().remove("deny_tcp_connect");
    assert!(!serde_json::from_value::<LandlockPolicy>(old).unwrap().deny_tcp_connect);
}

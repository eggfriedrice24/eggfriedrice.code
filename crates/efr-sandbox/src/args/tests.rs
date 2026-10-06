use std::collections::BTreeSet;
use std::ffi::OsString;
use std::path::Path;

use efr_protocol::Grant;
use pretty_assertions::assert_eq;

use crate::args::{FdTable, LaunchFds, encode_args};
use crate::plan::MountPlan;
use crate::spec::{Mask, MaskKind};
use crate::testing::{PROJECT, spec, world};

const LAUNCH: LaunchFds = LaunchFds { status: 9, policy: 5 };

fn strings(args: &[OsString]) -> Vec<String> {
    args.iter().map(|arg| arg.to_string_lossy().into_owned()).collect()
}

#[test]
fn args_never_reuse_an_fd() {
    let mut spec = spec();
    // The private tmp is bound twice from one source: two descriptors.
    spec.masks.push(Mask { path: "/home/u/p/app/.env".into(), kind: MaskKind::ProjectEnv });
    let mut fs = world();
    fs.file("/home/u/p/app/.env", "KEY=1");
    let plan = MountPlan::build(&spec, &fs).unwrap();
    let mut fds = FdTable::new(&fs);
    let args = strings(&plan.bwrap_args(&mut fds, &LAUNCH, Path::new(PROJECT)).unwrap());

    let mut used = Vec::new();
    for pair in args.windows(2) {
        if pair[0] == "--bind-fd" || pair[0] == "--ro-bind-fd" {
            used.push(pair[1].clone());
        }
    }
    let unique: BTreeSet<&String> = used.iter().collect();
    assert_eq!(unique.len(), used.len(), "{args:?}");
    assert_eq!(used.len(), fds.numbers().len());
    let tmp_binds = args
        .windows(3)
        .filter(|w| w[0] == "--bind-fd" && (w[2] == "/tmp" || w[2] == "/var/tmp"))
        .count();
    assert_eq!(tmp_binds, 2);
}

#[test]
fn args_hold_no_env_values() {
    let mut spec = spec();
    spec.env.deny = vec!["SECRET_VALUE_NAME".to_owned()];
    spec.grants.push(Grant::OpenNetwork);
    let fs = world();
    let plan = MountPlan::build(&spec, &fs).unwrap();
    let mut fds = FdTable::new(&fs);
    let args = strings(&plan.bwrap_args(&mut fds, &LAUNCH, Path::new(PROJECT)).unwrap());
    for forbidden in [
        "--setenv",
        "--unsetenv",
        "--clearenv",
        "TMPDIR",
        "EFR_SANDBOX",
        "SECRET_VALUE_NAME",
        "/bin/false",
    ] {
        assert!(!args.iter().any(|arg| arg.contains(forbidden)), "{forbidden} in {args:?}");
    }
    let filter = crate::EnvFilter::new(&spec, &plan);
    let env = [
        (OsString::from("GITHUB_TOKEN"), OsString::from("ghp_value")),
        (OsString::from("CCACHE_DIR"), OsString::from("/srv/ccache-value")),
    ]
    .into_iter()
    .collect();
    let (out, _) = filter.apply(&env);
    for value in ["ghp_value", "/srv/ccache-value", "/home/u/.cache/ccache"] {
        assert!(!args.iter().any(|arg| arg.contains(value)), "{value}");
    }
    assert!(out.values().any(|value| value == "/home/u/.cache/ccache"));
}

#[test]
fn args_start_with_the_namespaces_and_end_with_the_inner_stage() {
    let fs = world();
    let plan = MountPlan::build(&spec(), &fs).unwrap();
    let mut fds = FdTable::new(&fs);
    let args = strings(&plan.bwrap_args(&mut fds, &LAUNCH, Path::new(PROJECT)).unwrap());
    assert_eq!(
        args[..10],
        [
            "--unshare-user",
            "--disable-userns",
            "--unshare-pid",
            "--unshare-net",
            "--unshare-ipc",
            "--unshare-uts",
            "--unshare-cgroup",
            "--die-with-parent",
            "--cap-drop",
            "ALL",
        ]
    );
    assert!(!args.contains(&"--new-session".to_owned()));
    assert!(!args.contains(&"--proc".to_owned()));
    let tail = &args[args.len() - 7..];
    assert_eq!(
        tail,
        ["--chdir", PROJECT, "--", "/run/user/1000/efr-sbx/efr-sbx", "inner", "--policy-fd", "5"]
    );
    assert!(args.windows(2).any(|w| w == ["--json-status-fd", "9"]));
    assert!(args.windows(3).any(|w| w == ["--perms", "0700", "--tmpfs"]));
    assert!(args.windows(4).any(|w| w == ["--perms", "1777", "--tmpfs", "/dev/shm"]));
}

#[test]
fn an_open_network_grant_drops_only_the_network_namespace() {
    let mut spec = spec();
    spec.grants.push(Grant::OpenNetwork);
    let fs = world();
    let plan = MountPlan::build(&spec, &fs).unwrap();
    let args =
        strings(&plan.bwrap_args(&mut FdTable::new(&fs), &LAUNCH, Path::new(PROJECT)).unwrap());
    assert!(!args.contains(&"--unshare-net".to_owned()));
    assert!(args.contains(&"--disable-userns".to_owned()));
}

#[test]
fn a_bind_source_reached_through_a_link_is_refused() {
    let mut fs = world();
    let plan = MountPlan::build(&spec(), &fs).unwrap();
    // The project became a link after the plan was built.
    fs.remove(PROJECT).dir("/srv/elsewhere").link(PROJECT, "/srv/elsewhere");
    let mut fds = FdTable::new(&fs);
    assert!(plan.bwrap_args(&mut fds, &LAUNCH, Path::new(PROJECT)).is_err());
}

#[test]
fn encoded_args_end_each_argument_with_a_nul() {
    let bytes = encode_args(&[OsString::from("--ro-bind"), OsString::from("/")]);
    assert_eq!(bytes, b"--ro-bind\0/\0");
}

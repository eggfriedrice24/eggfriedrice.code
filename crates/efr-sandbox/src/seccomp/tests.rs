use pretty_assertions::assert_eq;

use crate::seccomp::{
    AF_INET, AF_INET6, AF_NETLINK, AF_UNIX, EAFNOSUPPORT, ENOSYS, EPERM, EPROTONOSUPPORT,
    NETLINK_ROUTE, NETLINK_SOCK_DIAG, SeccompAction, SeccompArch, SeccompProfile, SyscallRule,
    TIOCLINUX, TIOCSTI,
};

#[test]
fn seccomp_profile_lists_every_rule() {
    let profile = SeccompProfile::phase1();
    assert_eq!(profile.default_action, SeccompAction::Allow);
    assert_eq!(profile.arches, [SeccompArch::X86_64, SeccompArch::Aarch64]);
    assert_eq!(profile.other_arch_action, SeccompAction::KillProcess);
    let names = profile.syscalls();
    for syscall in [
        "ioctl",
        "io_uring_setup",
        "io_uring_enter",
        "io_uring_register",
        "socket",
        "socketpair",
        "ptrace",
        "process_vm_readv",
        "process_vm_writev",
        "keyctl",
        "add_key",
        "request_key",
        "bpf",
        "perf_event_open",
        "userfaultfd",
        "mount",
        "umount2",
        "pivot_root",
        "move_mount",
        "open_tree",
        "fsopen",
        "fsconfig",
        "fsmount",
        "fspick",
        "mount_setattr",
        "setns",
        "unshare",
        "clone",
        "clone3",
        "open_by_handle_at",
        "name_to_handle_at",
        "kexec_load",
        "kexec_file_load",
        "init_module",
        "finit_module",
        "delete_module",
        "reboot",
        "swapon",
        "swapoff",
        "acct",
        "iopl",
        "ioperm",
    ] {
        assert!(names.contains(&syscall), "{syscall}");
    }
    assert!(
        profile
            .rules
            .contains(&SyscallRule::DenyIoctl { requests: vec![TIOCSTI, TIOCLINUX], errno: EPERM })
    );
    assert!(profile.rules.contains(&SyscallRule::AllowSocketFamilies {
        allowed: vec![AF_UNIX, AF_INET, AF_INET6, AF_NETLINK],
        errno: EAFNOSUPPORT,
    }));
    assert!(
        profile.rules.contains(&SyscallRule::Deny { syscall: "clone3".to_owned(), errno: ENOSYS })
    );
    assert!(profile.rules.iter().any(
        |rule| matches!(rule, SyscallRule::DenyCloneFlags { mask, .. } if mask & 0x1000_0000 != 0)
    ));
}

#[test]
fn the_profile_reads_back_from_json() {
    let profile = SeccompProfile::phase1();
    let text = serde_json::to_string(&profile).unwrap();
    assert_eq!(serde_json::from_str::<SeccompProfile>(&text).unwrap(), profile);
    assert!(text.contains("\"kind\":\"errno\"") || text.contains("\"allow\""), "{text}");
}

/// The socket family rules of `profile`.
fn families(profile: &SeccompProfile) -> Vec<&SyscallRule> {
    profile
        .rules
        .iter()
        .filter(|rule| matches!(rule, SyscallRule::AllowSocketFamilies { .. }))
        .collect()
}

#[test]
fn net_view_profile_allows_unix_and_netlink_only() {
    let profile = SeccompProfile::net_view();
    assert_eq!(
        families(&profile),
        [&SyscallRule::AllowSocketFamilies {
            allowed: vec![AF_UNIX, AF_NETLINK],
            errno: EAFNOSUPPORT,
        }]
    );
    // Every other rule of phase 1 stays: the profile only adds refusals.
    let phase1 = SeccompProfile::phase1();
    let phase1_families = families(&phase1);
    for rule in phase1.rules.iter().filter(|rule| !phase1_families.contains(rule)) {
        assert!(profile.rules.contains(rule), "{rule:?}");
    }
    assert_eq!(profile.default_action, phase1.default_action);
    assert_eq!(profile.arches, phase1.arches);
    assert_eq!(profile.other_arch_action, SeccompAction::KillProcess);
    let text = serde_json::to_string(&profile).unwrap();
    assert_eq!(serde_json::from_str::<SeccompProfile>(&text).unwrap(), profile);
}

#[test]
fn net_view_profile_limits_netlink_protocols() {
    let profile = SeccompProfile::net_view();
    let rules: Vec<&SyscallRule> = profile
        .rules
        .iter()
        .filter(|rule| matches!(rule, SyscallRule::AllowNetlinkProtocols { .. }))
        .collect();
    assert_eq!(
        rules,
        [&SyscallRule::AllowNetlinkProtocols {
            allowed: vec![NETLINK_ROUTE, NETLINK_SOCK_DIAG],
            errno: EPROTONOSUPPORT,
        }]
    );
    assert_eq!((NETLINK_ROUTE, NETLINK_SOCK_DIAG, EPROTONOSUPPORT), (0, 4, 93));
    assert!(
        !SeccompProfile::phase1()
            .rules
            .iter()
            .any(|rule| matches!(rule, SyscallRule::AllowNetlinkProtocols { .. }))
    );
}

#[test]
fn net_view_profile_refuses_listen_and_pidfd_getfd() {
    let profile = SeccompProfile::net_view();
    let phase1 = SeccompProfile::phase1();
    for syscall in ["listen", "pidfd_getfd"] {
        let rule = SyscallRule::Deny { syscall: syscall.to_owned(), errno: EPERM };
        assert!(profile.rules.contains(&rule), "{syscall}");
        assert!(!phase1.rules.contains(&rule), "phase 1 keeps {syscall}");
        assert!(profile.syscalls().contains(&syscall));
    }
    let io_uring = SyscallRule::Deny { syscall: "io_uring_setup".to_owned(), errno: EPERM };
    assert!(profile.rules.contains(&io_uring));
    for rule in families(&profile) {
        let SyscallRule::AllowSocketFamilies { allowed, .. } = rule else { unreachable!() };
        assert!(!allowed.contains(&AF_INET) && !allowed.contains(&AF_INET6), "{rule:?}");
    }
}

#[test]
fn bridge_profile_allows_unix_only() {
    let profile = SeccompProfile::bridge();
    assert_eq!(
        families(&profile),
        [&SyscallRule::AllowSocketFamilies { allowed: vec![AF_UNIX], errno: EAFNOSUPPORT }]
    );
    // Every other rule of phase 1 stays: the profile only adds refusals.
    let phase1 = SeccompProfile::phase1();
    let phase1_families = families(&phase1);
    for rule in phase1.rules.iter().filter(|rule| !phase1_families.contains(rule)) {
        assert!(profile.rules.contains(rule), "{rule:?}");
    }
    assert_eq!(profile.default_action, phase1.default_action);
    assert_eq!(profile.arches, phase1.arches);
    assert_eq!(profile.other_arch_action, SeccompAction::KillProcess);
    let text = serde_json::to_string(&profile).unwrap();
    assert_eq!(serde_json::from_str::<SeccompProfile>(&text).unwrap(), profile);
}

#[test]
fn bus_proxy_profile_allows_unix_only() {
    let profile = SeccompProfile::bus_proxy();
    assert_eq!(
        families(&profile),
        [&SyscallRule::AllowSocketFamilies { allowed: vec![AF_UNIX], errno: EAFNOSUPPORT }]
    );
    let phase1 = SeccompProfile::phase1();
    let phase1_families = families(&phase1);
    for rule in phase1.rules.iter().filter(|rule| !phase1_families.contains(rule)) {
        assert!(profile.rules.contains(rule), "{rule:?}");
    }
    for syscall in ["ptrace", "pidfd_getfd", "process_vm_readv", "io_uring_setup"] {
        let rule = SyscallRule::Deny { syscall: syscall.to_owned(), errno: EPERM };
        assert_eq!(profile.rules.iter().filter(|known| **known == rule).count(), 1, "{syscall}");
    }
    // The proxy binds, listens, accepts and starts GLib's threads; the wrapper's own
    // exec of the program runs after the filter.
    for syscall in ["bind", "listen", "accept4", "connect", "clone", "socket", "execve"] {
        let denied = profile.rules.iter().any(
            |rule| matches!(rule, SyscallRule::Deny { syscall: named, .. } if named == syscall),
        );
        assert!(!denied, "{syscall}");
    }
    assert_eq!(profile.other_arch_action, SeccompAction::KillProcess);
    let text = serde_json::to_string(&profile).unwrap();
    assert_eq!(serde_json::from_str::<SeccompProfile>(&text).unwrap(), profile);
}

#[test]
fn bridge_profile_refuses_bind_listen_exec() {
    let profile = SeccompProfile::bridge();
    for syscall in ["bind", "listen", "execve", "execveat", "ptrace", "pidfd_getfd"] {
        let rule = SyscallRule::Deny { syscall: syscall.to_owned(), errno: EPERM };
        assert_eq!(profile.rules.iter().filter(|known| **known == rule).count(), 1, "{syscall}");
    }
    // The bridge's threads still start: clone3 gives ENOSYS, and libc falls back to
    // clone, whose flags seccomp can read.
    assert!(
        profile.rules.contains(&SyscallRule::Deny { syscall: "clone3".to_owned(), errno: ENOSYS })
    );
    for syscall in ["connect", "accept4", "clone", "socket"] {
        let denied = profile.rules.iter().any(
            |rule| matches!(rule, SyscallRule::Deny { syscall: named, .. } if named == syscall),
        );
        assert!(!denied, "{syscall}");
    }
}

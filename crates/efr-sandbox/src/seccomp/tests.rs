use pretty_assertions::assert_eq;

use crate::seccomp::{
    AF_INET, AF_INET6, AF_NETLINK, AF_UNIX, EAFNOSUPPORT, ENOSYS, EPERM, SeccompAction,
    SeccompArch, SeccompProfile, SyscallRule, TIOCLINUX, TIOCSTI,
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

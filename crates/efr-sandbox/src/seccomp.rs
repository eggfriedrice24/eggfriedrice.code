//! [`SeccompProfile`]: the fixed deny list that `efr-sbx inner` installs after
//! Landlock, as data. The default action is allow; the list removes kernel surface that
//! no build or test needs and stops typing into the shared terminal.

use serde::{Deserialize, Serialize};

/// `EPERM`.
pub const EPERM: u32 = 1;
/// `ENOSYS`.
pub const ENOSYS: u32 = 38;
/// `EAFNOSUPPORT`.
pub const EAFNOSUPPORT: u32 = 97;

/// `TIOCSTI`: push a byte into a terminal's input.
pub const TIOCSTI: u64 = 0x5412;
/// `TIOCLINUX`: the console's own requests, one of which pastes the selection.
pub const TIOCLINUX: u64 = 0x541C;

/// `AF_UNIX`.
pub const AF_UNIX: u64 = 1;
/// `AF_INET`.
pub const AF_INET: u64 = 2;
/// `AF_INET6`.
pub const AF_INET6: u64 = 10;
/// `AF_NETLINK`.
pub const AF_NETLINK: u64 = 16;

/// Every `CLONE_NEW*` flag: time, mount, cgroup, uts, ipc, user, pid and net.
pub const CLONE_NEW_MASK: u64 = 0x0000_0080
    | 0x0002_0000
    | 0x0200_0000
    | 0x0400_0000
    | 0x0800_0000
    | 0x1000_0000
    | 0x2000_0000
    | 0x4000_0000;

/// What the filter does with a call that matches nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "errno", rename_all = "snake_case")]
#[non_exhaustive]
pub enum SeccompAction {
    /// Let it run.
    Allow,
    /// Fail it with this errno.
    Errno(u32),
    /// Kill the process.
    KillProcess,
}

/// A CPU architecture of the filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum SeccompArch {
    /// x86_64, without the x32 ABI.
    X86_64,
    /// aarch64.
    Aarch64,
}

/// One rule of the deny list.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum SyscallRule {
    /// The call fails with `errno` whatever its arguments.
    Deny {
        /// The system call's name, such as `io_uring_setup`.
        syscall: String,
        /// The errno.
        errno: u32,
    },
    /// `ioctl` with one of these requests fails.
    DenyIoctl {
        /// The request numbers.
        requests: Vec<u64>,
        /// The errno.
        errno: u32,
    },
    /// `socket` and `socketpair` with a family outside `allowed` fail.
    AllowSocketFamilies {
        /// The families that stay allowed.
        allowed: Vec<u64>,
        /// The errno for every other family.
        errno: u32,
    },
    /// `clone` with any flag of `mask` fails.
    DenyCloneFlags {
        /// The flags.
        mask: u64,
        /// The errno.
        errno: u32,
    },
}

/// The seccomp filter of a contained call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct SeccompProfile {
    /// The action of a call that no rule matches.
    pub default_action: SeccompAction,
    /// The architectures the filter covers.
    pub arches: Vec<SeccompArch>,
    /// The action for a call of any other architecture, the x32 ABI included.
    pub other_arch_action: SeccompAction,
    /// The rules.
    pub rules: Vec<SyscallRule>,
}

/// The calls that fail with `EPERM` whatever their arguments, grouped by why.
const DENIED: &[&str] = &[
    // io_uring can make AF_VSOCK sockets without a socket() call.
    "io_uring_setup",
    "io_uring_enter",
    "io_uring_register",
    // A second layer on top of Landlock's domain; debuggers need an outside exit.
    "ptrace",
    "process_vm_readv",
    "process_vm_writev",
    // Kernel keyrings can hold credentials that no mask covers.
    "keyctl",
    "add_key",
    "request_key",
    // Kernel surface with no build or test use.
    "bpf",
    "perf_event_open",
    "userfaultfd",
    // No capability remains and --disable-userns stops new user namespaces; a third
    // layer.
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
    // File handles go around path checks.
    "open_by_handle_at",
    "name_to_handle_at",
    // They need capabilities anyway.
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
];

impl SeccompProfile {
    /// The deny list of phase 1 (the spec's section 3.6).
    pub fn phase1() -> SeccompProfile {
        let mut rules = vec![
            SyscallRule::DenyIoctl { requests: vec![TIOCSTI, TIOCLINUX], errno: EPERM },
            SyscallRule::AllowSocketFamilies {
                allowed: vec![AF_UNIX, AF_INET, AF_INET6, AF_NETLINK],
                errno: EAFNOSUPPORT,
            },
            SyscallRule::DenyCloneFlags { mask: CLONE_NEW_MASK, errno: EPERM },
            // Its flags lie in memory that seccomp cannot read; libc then uses clone.
            SyscallRule::Deny { syscall: "clone3".to_owned(), errno: ENOSYS },
        ];
        rules.extend(
            DENIED
                .iter()
                .map(|syscall| SyscallRule::Deny { syscall: (*syscall).to_owned(), errno: EPERM }),
        );
        SeccompProfile {
            default_action: SeccompAction::Allow,
            arches: vec![SeccompArch::X86_64, SeccompArch::Aarch64],
            other_arch_action: SeccompAction::KillProcess,
            rules,
        }
    }

    /// The names of the calls that a rule names, each once.
    pub fn syscalls(&self) -> Vec<&str> {
        let mut names: Vec<&str> = Vec::new();
        for rule in &self.rules {
            let named: &[&str] = match rule {
                SyscallRule::Deny { syscall, .. } => &[syscall.as_str()],
                SyscallRule::DenyIoctl { .. } => &["ioctl"],
                SyscallRule::AllowSocketFamilies { .. } => &["socket", "socketpair"],
                SyscallRule::DenyCloneFlags { .. } => &["clone"],
            };
            for name in named {
                if !names.contains(name) {
                    names.push(name);
                }
            }
        }
        names
    }
}

#[cfg(test)]
mod tests;

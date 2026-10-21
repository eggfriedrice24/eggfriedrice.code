use efr_sandbox::{
    AF_INET, AF_INET6, AF_PACKET, AF_UNIX, EAFNOSUPPORT, ENOSYS, EPERM, EPROTONOSUPPORT,
    NETLINK_ROUTE, NETLINK_SOCK_DIAG, TIOCSTI,
};

use super::*;

#[test]
fn rules_are_grouped_by_errno() {
    let json = filters_json(&SeccompProfile::phase1(), SeccompArch::X86_64).unwrap();
    let names: Vec<&String> = json.as_object().unwrap().keys().collect();
    assert_eq!(
        names,
        [format!("errno_{EPERM}"), format!("errno_{ENOSYS}"), format!("errno_{EAFNOSUPPORT}")]
            .iter()
            .collect::<Vec<_>>()
    );
    let enosys = &json[format!("errno_{ENOSYS}")]["filter"];
    assert_eq!(enosys, &json!([{ "syscall": "clone3" }]));
}

#[test]
fn ioctl_rule_compares_the_low_word_of_the_request() {
    let json = filters_json(&SeccompProfile::phase1(), SeccompArch::X86_64).unwrap();
    let rules = json[format!("errno_{EPERM}")]["filter"].as_array().unwrap();
    let ioctl = rules.iter().find(|rule| rule["syscall"] == "ioctl").unwrap();
    assert_eq!(
        ioctl["args"][0],
        json!({ "index": 1, "type": "dword", "op": "eq", "val": TIOCSTI })
    );
}

#[test]
fn socket_rule_lists_every_allowed_family_as_not_equal() {
    let json = filters_json(&SeccompProfile::phase1(), SeccompArch::X86_64).unwrap();
    let rules = json[format!("errno_{EAFNOSUPPORT}")]["filter"].as_array().unwrap();
    assert_eq!(rules.len(), 2);
    assert_eq!(rules[0]["args"].as_array().unwrap().len(), 4);
}

#[test]
fn clone_rule_has_one_condition_per_namespace_flag() {
    let json = filters_json(&SeccompProfile::phase1(), SeccompArch::X86_64).unwrap();
    let rules = json[format!("errno_{EPERM}")]["filter"].as_array().unwrap();
    let clones = rules.iter().filter(|rule| rule["syscall"] == "clone").count();
    assert_eq!(clones, 8);
}

#[test]
fn port_calls_are_left_out_on_aarch64_only() {
    let x86 = filters_json(&SeccompProfile::phase1(), SeccompArch::X86_64).unwrap();
    let arm = filters_json(&SeccompProfile::phase1(), SeccompArch::Aarch64).unwrap();
    let has_iopl = |json: &Value| {
        json[format!("errno_{EPERM}")]["filter"]
            .as_array()
            .unwrap()
            .iter()
            .any(|rule| rule["syscall"] == "iopl")
    };
    assert!(has_iopl(&x86));
    assert!(!has_iopl(&arm));
}

#[test]
fn the_profile_compiles_for_this_machine() {
    let programs = compile(&SeccompProfile::phase1()).unwrap();
    let expected = if std::env::consts::ARCH == "x86_64" { 4 } else { 3 };
    assert_eq!(programs.len(), expected);
    assert!(programs.iter().all(|program| !program.is_empty()));
}

#[test]
fn x32_filter_kills_only_x32_numbers() {
    let program = x32_filter();
    assert_eq!(program.len(), 6);
    assert_eq!(program[3].k, X32_SYSCALL_BIT);
    assert_eq!(program[4].k, RET_KILL_PROCESS);
    assert_eq!(program[5].k, RET_ALLOW);
}

#[test]
fn a_profile_this_build_cannot_install_names_what_it_asks_for() {
    let mut profile = SeccompProfile::phase1();
    profile.default_action = SeccompAction::KillProcess;
    let error = filters_json(&profile, SeccompArch::X86_64).unwrap_err();
    assert!(
        matches!(
            error,
            SbxError::SeccompActions {
                default: SeccompAction::KillProcess,
                other_arch: SeccompAction::KillProcess,
            }
        ),
        "{error:?}"
    );
    let mut profile = SeccompProfile::phase1();
    profile.arches.clear();
    assert!(matches!(target(&profile), Err(SbxError::SeccompArchNotCovered { .. })));
}

#[test]
fn netlink_protocol_rule_compiles() {
    let profile = SeccompProfile::net_view();
    for arch in [SeccompArch::X86_64, SeccompArch::Aarch64] {
        let json = filters_json(&profile, arch).unwrap();
        let rules = json[format!("errno_{EPROTONOSUPPORT}")]["filter"].as_array().unwrap();
        assert_eq!(
            rules,
            &[json!({ "syscall": "socket", "args": [
                { "index": 0, "type": "dword", "op": "eq", "val": AF_NETLINK },
                { "index": 2, "type": "dword", "op": "ne", "val": NETLINK_ROUTE },
                { "index": 2, "type": "dword", "op": "ne", "val": NETLINK_SOCK_DIAG },
            ] })]
        );
        let families = json[format!("errno_{EAFNOSUPPORT}")]["filter"].as_array().unwrap();
        for rule in families {
            assert_eq!(
                rule["args"],
                json!([
                    { "index": 0, "type": "dword", "op": "ne", "val": AF_UNIX },
                    { "index": 0, "type": "dword", "op": "ne", "val": AF_NETLINK },
                ])
            );
        }
        let denied = json[format!("errno_{EPERM}")]["filter"].as_array().unwrap();
        for syscall in ["listen", "pidfd_getfd"] {
            assert!(denied.contains(&json!({ "syscall": syscall })), "{syscall}");
        }
    }
    let programs = compile(&profile).unwrap();
    let expected = if std::env::consts::ARCH == "x86_64" { 5 } else { 4 };
    assert_eq!(programs.len(), expected);
}

/// `AUDIT_ARCH_*` of the architectures of the cases.
const ARCH_X86_64: u32 = AUDIT_ARCH_X86_64;
const ARCH_I386: u32 = 3 | 0x4000_0000;
const ARCH_AARCH64: u32 = 183 | 0x8000_0000 | 0x4000_0000;
const ARCH_ARM: u32 = 40 | 0x4000_0000;

/// `SECCOMP_RET_ERRNO` with `errno`.
fn errno(errno: u32) -> u32 {
    0x0005_0000 | errno
}

/// A `struct seccomp_data`, little-endian like both architectures: `nr`, `arch`, the
/// instruction pointer and six arguments.
fn seccomp_data(nr: u32, arch: u32, args: [u64; 3]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(64);
    bytes.extend(nr.to_le_bytes());
    bytes.extend(arch.to_le_bytes());
    bytes.extend(0_u64.to_le_bytes());
    for arg in args.into_iter().chain([0; 3]) {
        bytes.extend(arg.to_le_bytes());
    }
    bytes
}

/// Runs one classic BPF program on `data`, as the kernel's seccomp does.
fn run_bpf(program: &[sock_filter], data: &[u8]) -> u32 {
    let (mut a, mut x, mut mem, mut pc) = (0_u32, 0_u32, [0_u32; 16], 0_usize);
    let word = |offset: u32| {
        let at = offset as usize;
        u32::from_le_bytes(data[at..at + 4].try_into().unwrap())
    };
    loop {
        let op = &program[pc];
        pc += 1;
        let src = if op.code & 0x08 == 0 { op.k } else { x };
        match op.code & 0x07 {
            // LD and LDX: only words, by an absolute offset, a constant or the scratch.
            class @ (0x00 | 0x01) => {
                let value = match op.code & 0xe0 {
                    0x00 => op.k,
                    0x20 => word(op.k),
                    0x60 => mem[op.k as usize],
                    mode => panic!("load mode {mode:#x}"),
                };
                assert_eq!(op.code & 0x18, 0, "only word loads");
                if class == 0 { a = value } else { x = value }
            }
            0x02 => mem[op.k as usize] = a,
            0x03 => mem[op.k as usize] = x,
            0x04 => {
                a = match op.code & 0xf0 {
                    0x00 => a.wrapping_add(src),
                    0x10 => a.wrapping_sub(src),
                    0x40 => a | src,
                    0x50 => a & src,
                    0x60 => a << src,
                    0x70 => a >> src,
                    0xa0 => a ^ src,
                    alu => panic!("alu op {alu:#x}"),
                }
            }
            0x05 => {
                let taken = match op.code & 0xf0 {
                    0x00 => {
                        pc += op.k as usize;
                        continue;
                    }
                    0x10 => a == src,
                    0x20 => a > src,
                    0x30 => a >= src,
                    0x40 => a & src != 0,
                    jump => panic!("jump op {jump:#x}"),
                };
                pc += usize::from(if taken { op.jt } else { op.jf });
            }
            0x06 => return if op.code & 0x18 == 0x10 { a } else { op.k },
            _ => {
                if op.code & 0xf8 == 0 {
                    x = a
                } else {
                    a = x
                }
            }
        }
    }
}

/// The answer of every program for one call, as the kernel combines them: the last
/// one installed runs first, and the strictest action wins.
fn verdict(programs: &[BpfProgram], data: &[u8]) -> u32 {
    let action = |ret: u32| (ret & 0xffff_0000).cast_signed();
    programs.iter().rev().fold(RET_ALLOW, |ret, program| {
        let this = run_bpf(program, data);
        if action(this) < action(ret) { this } else { ret }
    })
}

/// The system call numbers of the cases on one architecture.
struct Numbers {
    arch: SeccompArch,
    target: TargetArch,
    audit: u32,
    socket: u32,
    socketpair: u32,
    listen: u32,
    pidfd_getfd: u32,
    getpid: u32,
    /// The 32-bit architecture that the same CPU runs, and its socket call.
    compat: (u32, u32),
}

#[test]
fn net_view_filter_runs_on_bpf_cases() {
    let x86_64 = Numbers {
        arch: SeccompArch::X86_64,
        target: TargetArch::x86_64,
        audit: ARCH_X86_64,
        socket: 41,
        socketpair: 53,
        listen: 50,
        pidfd_getfd: 438,
        getpid: 39,
        // socketcall through int 0x80.
        compat: (ARCH_I386, 102),
    };
    let aarch64 = Numbers {
        arch: SeccompArch::Aarch64,
        target: TargetArch::aarch64,
        audit: ARCH_AARCH64,
        socket: 198,
        socketpair: 199,
        listen: 201,
        pidfd_getfd: 438,
        getpid: 172,
        compat: (ARCH_ARM, 281),
    };
    let high = 1_u64 << 32;
    let (raw, stream, dgram) = (3, 1, 2);
    for numbers in [x86_64, aarch64] {
        let programs =
            compile_for(&SeccompProfile::net_view(), numbers.target, numbers.arch).unwrap();
        let call =
            |nr: u32, args: [u64; 3]| verdict(&programs, &seccomp_data(nr, numbers.audit, args));
        let what = format!("{:?}", numbers.arch);
        // The families: only AF_UNIX and AF_NETLINK, also with garbage in the high word.
        assert_eq!(call(numbers.socket, [AF_INET, stream, 0]), errno(EAFNOSUPPORT), "{what}");
        assert_eq!(call(numbers.socket, [AF_INET6, dgram, 0]), errno(EAFNOSUPPORT), "{what}");
        assert_eq!(call(numbers.socket, [AF_PACKET, raw, 0]), errno(EAFNOSUPPORT), "{what}");
        assert_eq!(call(numbers.socket, [AF_INET | high, stream, 0]), errno(EAFNOSUPPORT));
        assert_eq!(call(numbers.socketpair, [AF_INET, stream, 0]), errno(EAFNOSUPPORT));
        for family in [31, 38, 40, 44, 45, 46, 200] {
            assert_eq!(call(numbers.socket, [family, stream, 0]), errno(EAFNOSUPPORT), "{family}");
        }
        assert_eq!(call(numbers.socket, [AF_UNIX, stream, 0]), RET_ALLOW, "{what}");
        assert_eq!(call(numbers.socketpair, [AF_UNIX, dgram, 0]), RET_ALLOW, "{what}");
        // The netlink protocols: only ROUTE and SOCK_DIAG, compared as the kernel's int.
        assert_eq!(call(numbers.socket, [AF_NETLINK, raw, NETLINK_ROUTE]), RET_ALLOW);
        assert_eq!(call(numbers.socket, [AF_NETLINK, dgram, NETLINK_SOCK_DIAG]), RET_ALLOW);
        assert_eq!(call(numbers.socket, [AF_NETLINK, raw, high]), RET_ALLOW, "{what}");
        for protocol in [9, 15, 16, 31, 32] {
            assert_eq!(
                call(numbers.socket, [AF_NETLINK, raw, protocol]),
                errno(EPROTONOSUPPORT),
                "{what} {protocol}"
            );
        }
        assert_eq!(call(numbers.socket, [AF_NETLINK | high, raw, 16]), errno(EPROTONOSUPPORT));
        assert_eq!(call(numbers.socket, [AF_NETLINK, raw, 16 | high]), errno(EPROTONOSUPPORT));
        // listen and pidfd_getfd, and a call that no rule names.
        assert_eq!(call(numbers.listen, [3, 1, 0]), errno(EPERM), "{what}");
        assert_eq!(call(numbers.pidfd_getfd, [3, 0, 0]), errno(EPERM), "{what}");
        assert_eq!(call(numbers.getpid, [0, 0, 0]), RET_ALLOW, "{what}");
        // Another architecture of the same CPU is killed, whatever the call.
        let (arch, socket) = numbers.compat;
        let other = verdict(&programs, &seccomp_data(socket, arch, [AF_INET, stream, 0]));
        assert_eq!(other, RET_KILL_PROCESS, "{what}");
        if numbers.arch == SeccompArch::X86_64 {
            let x32 = numbers.socket | X32_SYSCALL_BIT;
            assert_eq!(call(x32, [AF_UNIX, stream, 0]), RET_KILL_PROCESS);
        }
    }
    // Phase 1 keeps TCP and the other netlink protocols, and listen.
    let phase1 =
        compile_for(&SeccompProfile::phase1(), TargetArch::x86_64, SeccompArch::X86_64).unwrap();
    let call = |nr: u32, args: [u64; 3]| verdict(&phase1, &seccomp_data(nr, ARCH_X86_64, args));
    assert_eq!(call(41, [AF_INET, stream, 0]), RET_ALLOW);
    assert_eq!(call(41, [AF_NETLINK, raw, 16]), RET_ALLOW);
    assert_eq!(call(50, [3, 1, 0]), RET_ALLOW);
    assert_eq!(call(41, [AF_PACKET, raw, 0]), errno(EAFNOSUPPORT));
}

/// The system call numbers of the bridge's cases on one architecture.
struct BridgeNumbers {
    arch: SeccompArch,
    target: TargetArch,
    audit: u32,
    socket: u32,
    socketpair: u32,
    /// bind, listen, execve, execveat, ptrace, pidfd_getfd.
    denied: [u32; 6],
    /// connect, accept4, read, write, getpid.
    allowed: [u32; 5],
    clone: u32,
    clone3: u32,
}

#[test]
fn bridge_filter_runs_on_bpf_cases() {
    let x86_64 = BridgeNumbers {
        arch: SeccompArch::X86_64,
        target: TargetArch::x86_64,
        audit: ARCH_X86_64,
        socket: 41,
        socketpair: 53,
        denied: [49, 50, 59, 322, 101, 438],
        allowed: [42, 288, 0, 1, 39],
        clone: 56,
        clone3: 435,
    };
    let aarch64 = BridgeNumbers {
        arch: SeccompArch::Aarch64,
        target: TargetArch::aarch64,
        audit: ARCH_AARCH64,
        socket: 198,
        socketpair: 199,
        denied: [200, 201, 221, 281, 117, 438],
        allowed: [203, 242, 63, 64, 172],
        clone: 220,
        clone3: 435,
    };
    let high = 1_u64 << 32;
    let (stream, dgram) = (1, 2);
    // The flags of a thread that std starts: CLONE_VM, FS, FILES, SIGHAND, THREAD,
    // SYSVSEM, SETTLS, PARENT_SETTID and CHILD_CLEARTID.
    let thread_flags = 0x003d_0f00;
    for numbers in [x86_64, aarch64] {
        let programs =
            compile_for(&SeccompProfile::bridge(), numbers.target, numbers.arch).unwrap();
        let call =
            |nr: u32, args: [u64; 3]| verdict(&programs, &seccomp_data(nr, numbers.audit, args));
        let what = format!("{:?}", numbers.arch);
        // Only Unix sockets, also with garbage in the high word.
        assert_eq!(call(numbers.socket, [AF_UNIX, stream, 0]), RET_ALLOW, "{what}");
        assert_eq!(call(numbers.socketpair, [AF_UNIX, dgram, 0]), RET_ALLOW, "{what}");
        for family in [AF_INET, AF_INET6, AF_PACKET, AF_NETLINK, 40, AF_INET | high] {
            let refused = errno(EAFNOSUPPORT);
            assert_eq!(call(numbers.socket, [family, stream, 0]), refused, "{what} {family}");
            assert_eq!(call(numbers.socketpair, [family, stream, 0]), refused, "{what}");
        }
        for nr in numbers.denied {
            assert_eq!(call(nr, [3, 0, 0]), errno(EPERM), "{what} {nr}");
        }
        for nr in numbers.allowed {
            assert_eq!(call(nr, [3, 0, 0]), RET_ALLOW, "{what} {nr}");
        }
        // Threads start: clone3 gives ENOSYS, and clone with a thread's flags runs; a
        // new namespace does not.
        assert_eq!(call(numbers.clone3, [0, 0, 0]), errno(ENOSYS), "{what}");
        assert_eq!(call(numbers.clone, [thread_flags, 0, 0]), RET_ALLOW, "{what}");
        assert_eq!(call(numbers.clone, [0x1000_0000, 0, 0]), errno(EPERM), "{what}");
        if numbers.arch == SeccompArch::X86_64 {
            let x32 = numbers.socket | X32_SYSCALL_BIT;
            assert_eq!(call(x32, [AF_UNIX, stream, 0]), RET_KILL_PROCESS);
        }
    }
}

#[test]
fn forward_filter_runs_on_bpf_cases() {
    let x86_64 = BridgeNumbers {
        arch: SeccompArch::X86_64,
        target: TargetArch::x86_64,
        audit: ARCH_X86_64,
        socket: 41,
        socketpair: 53,
        denied: [49, 50, 59, 322, 101, 438],
        allowed: [42, 288, 0, 1, 39],
        clone: 56,
        clone3: 435,
    };
    let aarch64 = BridgeNumbers {
        arch: SeccompArch::Aarch64,
        target: TargetArch::aarch64,
        audit: ARCH_AARCH64,
        socket: 198,
        socketpair: 199,
        denied: [200, 201, 221, 281, 117, 438],
        allowed: [203, 242, 63, 64, 172],
        clone: 220,
        clone3: 435,
    };
    let high = 1_u64 << 32;
    let (stream, dgram, raw) = (1, 2, 3);
    let thread_flags = 0x003d_0f00;
    for numbers in [x86_64, aarch64] {
        let programs =
            compile_for(&SeccompProfile::forward(), numbers.target, numbers.arch).unwrap();
        let call =
            |nr: u32, args: [u64; 3]| verdict(&programs, &seccomp_data(nr, numbers.audit, args));
        let what = format!("{:?}", numbers.arch);
        // Unix, IPv4 and IPv6 sockets: the forward socket and the job's own servers.
        // The kernel reads the family as an int, so the high word changes nothing.
        for family in [AF_UNIX, AF_INET, AF_INET6, AF_INET | high] {
            assert_eq!(call(numbers.socket, [family, stream, 0]), RET_ALLOW, "{what} {family}");
        }
        assert_eq!(call(numbers.socket, [AF_INET6, dgram, 0]), RET_ALLOW, "{what}");
        assert_eq!(call(numbers.socketpair, [AF_UNIX, dgram, 0]), RET_ALLOW, "{what}");
        for family in [AF_PACKET, AF_NETLINK, 40, 44, AF_PACKET | high] {
            let refused = errno(EAFNOSUPPORT);
            assert_eq!(call(numbers.socket, [family, raw, 0]), refused, "{what} {family}");
            assert_eq!(call(numbers.socketpair, [family, stream, 0]), refused, "{what}");
        }
        // bind, listen, execve, execveat, ptrace and pidfd_getfd are refused; connect,
        // accept4, read, write and getpid run.
        for nr in numbers.denied {
            assert_eq!(call(nr, [3, 0, 0]), errno(EPERM), "{what} {nr}");
        }
        for nr in numbers.allowed {
            assert_eq!(call(nr, [3, 0, 0]), RET_ALLOW, "{what} {nr}");
        }
        // Threads start, for the poll and the copies; a new namespace does not.
        assert_eq!(call(numbers.clone3, [0, 0, 0]), errno(ENOSYS), "{what}");
        assert_eq!(call(numbers.clone, [thread_flags, 0, 0]), RET_ALLOW, "{what}");
        assert_eq!(call(numbers.clone, [0x1000_0000, 0, 0]), errno(EPERM), "{what}");
        if numbers.arch == SeccompArch::X86_64 {
            let x32 = numbers.socket | X32_SYSCALL_BIT;
            assert_eq!(call(x32, [AF_INET, stream, 0]), RET_KILL_PROCESS);
        }
    }
}

//! Installs an `efr_sandbox::SeccompProfile` in the inner stage, after Landlock (the
//! spec's section 3.6).
//!
//! seccompiler gives one match action per filter, so the rules are grouped by errno
//! into one filter each; the kernel runs every filter and the strictest answer wins.
//! The profile names system calls, and seccompiler's json frontend maps the names with
//! its own tables for x86_64 and aarch64. A name the table of this architecture lacks
//! fails the call, except the x86-only port calls on aarch64. Each filter kills the
//! process on another architecture; on x86_64 one more filter kills a call of the x32
//! ABI, whose numbers the others would not match.

use std::collections::BTreeMap;

use efr_sandbox::{SeccompAction, SeccompArch, SeccompProfile, SyscallRule};
use seccompiler::{BpfProgram, TargetArch, sock_filter};
use serde_json::{Value, json};

use crate::error::SbxError;

/// System calls that only x86 has.
const X86_ONLY: &[&str] = &["iopl", "ioperm"];

/// `AUDIT_ARCH_X86_64`.
const AUDIT_ARCH_X86_64: u32 = 62 | 0x8000_0000 | 0x4000_0000;
/// The bit that marks a system call number of the x32 ABI.
const X32_SYSCALL_BIT: u32 = 0x4000_0000;
/// `SECCOMP_RET_KILL_PROCESS` and `SECCOMP_RET_ALLOW`.
const RET_KILL_PROCESS: u32 = 0x8000_0000;
const RET_ALLOW: u32 = 0x7fff_0000;

fn profile_error(detail: impl Into<String>) -> SbxError {
    SbxError::SeccompProfile { detail: detail.into() }
}

/// The architecture this build runs on, when the profile covers it.
fn target(profile: &SeccompProfile) -> Result<(TargetArch, SeccompArch), SbxError> {
    let arch = match std::env::consts::ARCH {
        "x86_64" => (TargetArch::x86_64, SeccompArch::X86_64),
        "aarch64" => (TargetArch::aarch64, SeccompArch::Aarch64),
        other => return Err(profile_error(format!("the architecture {other}"))),
    };
    if !profile.arches.contains(&arch.1) {
        return Err(profile_error(format!("the profile does not cover {:?}", arch.1)));
    }
    Ok(arch)
}

/// The profile as seccompiler's json, one filter per errno, for `arch`.
pub(crate) fn filters_json(profile: &SeccompProfile, arch: SeccompArch) -> Result<Value, SbxError> {
    if profile.default_action != SeccompAction::Allow
        || profile.other_arch_action != SeccompAction::KillProcess
    {
        return Err(profile_error("a default action other than allow, kill elsewhere"));
    }
    let mut groups: BTreeMap<u32, Vec<Value>> = BTreeMap::new();
    let mut add = |errno: u32, rule: Value| groups.entry(errno).or_default().push(rule);
    let skip = |name: &str| arch == SeccompArch::Aarch64 && X86_ONLY.contains(&name);
    for rule in &profile.rules {
        match rule {
            SyscallRule::Deny { syscall, errno } => {
                if !skip(syscall) {
                    add(*errno, json!({ "syscall": syscall }));
                }
            }
            SyscallRule::DenyIoctl { requests, errno } => {
                for request in requests {
                    let arg = json!({ "index": 1, "type": "dword", "op": "eq", "val": request });
                    add(*errno, json!({ "syscall": "ioctl", "args": [arg] }));
                }
            }
            SyscallRule::AllowSocketFamilies { allowed, errno } => {
                let args: Vec<Value> = allowed
                    .iter()
                    .map(|family| json!({ "index": 0, "type": "dword", "op": "ne", "val": family }))
                    .collect();
                for syscall in ["socket", "socketpair"] {
                    add(*errno, json!({ "syscall": syscall, "args": args }));
                }
            }
            SyscallRule::DenyCloneFlags { mask, errno } => {
                for bit in (0..64).map(|shift| 1_u64 << shift).filter(|bit| mask & bit != 0) {
                    let op = json!({ "masked_eq": bit });
                    let arg = json!({ "index": 0, "type": "qword", "op": op, "val": bit });
                    add(*errno, json!({ "syscall": "clone", "args": [arg] }));
                }
            }
            other => return Err(profile_error(format!("{other:?}"))),
        }
    }
    let filters: serde_json::Map<String, Value> = groups
        .into_iter()
        .map(|(errno, rules)| {
            let filter = json!({
                "mismatch_action": "allow",
                "match_action": { "errno": errno },
                "filter": rules,
            });
            (format!("errno_{errno}"), filter)
        })
        .collect();
    Ok(Value::Object(filters))
}

/// Compiles the profile into the programs to install, in order.
pub(crate) fn compile(profile: &SeccompProfile) -> Result<Vec<BpfProgram>, SbxError> {
    let (target_arch, arch) = target(profile)?;
    let json = filters_json(profile, arch)?;
    let bytes = serde_json::to_vec(&json).map_err(|error| profile_error(error.to_string()))?;
    let map = seccompiler::compile_from_json(bytes.as_slice(), target_arch)
        .map_err(|source| SbxError::Seccomp { source })?;
    let mut programs: Vec<(String, BpfProgram)> = map.into_iter().collect();
    programs.sort_by(|a, b| a.0.cmp(&b.0));
    let mut out: Vec<BpfProgram> = programs.into_iter().map(|(_, program)| program).collect();
    if arch == SeccompArch::X86_64 {
        out.push(x32_filter());
    }
    Ok(out)
}

/// Kills a process that makes a system call through the x32 ABI.
fn x32_filter() -> BpfProgram {
    const LD_W_ABS: u16 = 0x20;
    const JEQ_K: u16 = 0x15;
    const JGE_K: u16 = 0x35;
    const RET_K: u16 = 0x06;
    // struct seccomp_data: nr at offset 0, arch at offset 4.
    let op = |code, jt, jf, k| sock_filter { code, jt, jf, k };
    vec![
        op(LD_W_ABS, 0, 0, 4),
        op(JEQ_K, 0, 3, AUDIT_ARCH_X86_64),
        op(LD_W_ABS, 0, 0, 0),
        op(JGE_K, 0, 1, X32_SYSCALL_BIT),
        op(RET_K, 0, 0, RET_KILL_PROCESS),
        op(RET_K, 0, 0, RET_ALLOW),
    ]
}

/// Installs the profile on this thread, which is the inner stage's only one.
pub(crate) fn install(profile: &SeccompProfile) -> Result<(), SbxError> {
    for program in compile(profile)? {
        seccompiler::apply_filter(&program).map_err(|source| SbxError::Seccomp { source })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;

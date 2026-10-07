//! Applies an `efr_sandbox::LandlockPolicy` to the inner stage (the spec's sections 3.4
//! and 3.5).
//!
//! The rule set handles every right up to ABI 9 at `CompatLevel::HardRequirement`: a
//! kernel without one of them fails the call, a right is never dropped. The erratum
//! for disconnected directories must be fixed, because the design binds many
//! directories. A path rule whose path does not exist inside the sandbox is left out,
//! which only takes rights away.

use std::io;
use std::os::fd::BorrowedFd;
use std::path::Path;

use efr_sandbox::{FsAccess, LandlockPolicy, LandlockScope};
use landlock::{
    ABI, Access, AccessFs, BitFlags, CompatLevel, Compatible, Erratum, PathBeneath, PathFd,
    Ruleset, RulesetAttr, RulesetCreatedAttr, RulesetStatus, Scope,
};

use crate::error::SbxError;

/// The highest ABI this build knows; the policy may not ask for more.
const BUILD_ABI: u32 = 9;

fn landlock_error(source: impl Into<landlock::RulesetError>) -> SbxError {
    SbxError::Landlock { source: source.into() }
}

/// The crate's flags for a list of rights; `None` for a right this build does not
/// know, which fails the call.
fn access(rights: &[FsAccess]) -> Option<BitFlags<AccessFs>> {
    let mut flags = BitFlags::<AccessFs>::empty();
    for right in rights {
        flags |= match right {
            FsAccess::Execute => AccessFs::Execute,
            FsAccess::WriteFile => AccessFs::WriteFile,
            FsAccess::ReadFile => AccessFs::ReadFile,
            FsAccess::ReadDir => AccessFs::ReadDir,
            FsAccess::RemoveDir => AccessFs::RemoveDir,
            FsAccess::RemoveFile => AccessFs::RemoveFile,
            FsAccess::MakeChar => AccessFs::MakeChar,
            FsAccess::MakeDir => AccessFs::MakeDir,
            FsAccess::MakeReg => AccessFs::MakeReg,
            FsAccess::MakeSock => AccessFs::MakeSock,
            FsAccess::MakeFifo => AccessFs::MakeFifo,
            FsAccess::MakeBlock => AccessFs::MakeBlock,
            FsAccess::MakeSym => AccessFs::MakeSym,
            FsAccess::Refer => AccessFs::Refer,
            FsAccess::Truncate => AccessFs::Truncate,
            FsAccess::IoctlDev => AccessFs::IoctlDev,
            FsAccess::ResolveUnix => AccessFs::ResolveUnix,
            _ => return None,
        };
    }
    Some(flags)
}

fn scopes(policy: &LandlockPolicy) -> Option<BitFlags<Scope>> {
    let mut flags = BitFlags::<Scope>::empty();
    for scope in &policy.scopes {
        flags |= match scope {
            LandlockScope::AbstractUnixSocket => Scope::AbstractUnixSocket,
            LandlockScope::Signal => Scope::Signal,
            _ => return None,
        };
    }
    Some(flags)
}

/// The errata bits the running kernel reports fixed.
pub(crate) fn kernel_errata() -> u32 {
    Erratum::current().bits()
}

/// Restricts this process with `policy`. `terminals` are descriptors that get the
/// device rights when they are a terminal, so the child can reopen its terminal
/// (`echo > /dev/stderr`) and change its modes.
pub(crate) fn restrict(
    policy: &LandlockPolicy,
    terminals: &[BorrowedFd<'_>],
) -> Result<(), SbxError> {
    let too_old =
        || SbxError::LandlockTooOld { abi: policy.min_abi, errata: policy.required_errata };
    if policy.min_abi > BUILD_ABI {
        return Err(too_old());
    }
    if kernel_errata() & policy.required_errata != policy.required_errata {
        return Err(too_old());
    }
    // Every right up to ABI 9 is handled, whatever the rules grant, so a right that no
    // rule names is denied everywhere.
    let handled = access(&FsAccess::HANDLED).ok_or_else(too_old)?;
    if handled != AccessFs::from_all(ABI::V9) {
        return Err(too_old());
    }
    let scope = scopes(policy).ok_or_else(too_old)?;
    let mut ruleset = Ruleset::default()
        .set_compatibility(CompatLevel::HardRequirement)
        .handle_access(handled)
        .map_err(landlock_error)?
        .scope(scope)
        .map_err(landlock_error)?
        .create()
        .map_err(landlock_error)?;
    for rule in policy.rules() {
        let Some(rights) = access(&rule.access) else { return Err(too_old()) };
        let Some(fd) = open_rule_path(&rule.path)? else { continue };
        ruleset = ruleset.add_rule(PathBeneath::new(fd, rights)).map_err(landlock_error)?;
    }
    let device = access(&FsAccess::DEVICE).ok_or_else(too_old)?;
    for terminal in terminals {
        if rustix::termios::isatty(terminal) {
            ruleset =
                ruleset.add_rule(PathBeneath::new(*terminal, device)).map_err(landlock_error)?;
        }
    }
    let status = ruleset.restrict_self().map_err(landlock_error)?;
    if status.ruleset != RulesetStatus::FullyEnforced {
        return Err(SbxError::LandlockPartial);
    }
    Ok(())
}

/// An `O_PATH` descriptor of a rule's path; `None` when the path does not exist inside.
fn open_rule_path(path: &Path) -> Result<Option<PathFd>, SbxError> {
    match PathFd::new(path) {
        Ok(fd) => Ok(Some(fd)),
        Err(error) => match std::fs::symlink_metadata(path) {
            Err(missing) if missing.kind() == io::ErrorKind::NotFound => Ok(None),
            _ => Err(SbxError::io("open the Landlock rule path", path, io::Error::other(error))),
        },
    }
}

#[cfg(test)]
mod tests;

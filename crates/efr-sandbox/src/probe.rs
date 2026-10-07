//! The probe's result types and the pure parts of its checks (the spec's section 12).
//!
//! `efr-sbx probe --json` runs every check and one real launch with a self-test, and
//! prints a [`ProbeReport`]; efrd turns it into the `SandboxStatus` that `admin.status`
//! and the fallback read. The reason and fix texts of each failure live here, so the
//! probe, efrd and the CLI say the same thing.

use std::path::PathBuf;

use efr_protocol::{CacheMode, CheckOutcome, NetworkMode, SandboxCheck, SandboxStatus};
use serde::{Deserialize, Serialize};

use crate::SandboxError;
use crate::landlock::{ERRATUM_DISCONNECTED_DIRS, MIN_LANDLOCK_ABI};

/// The flags that bwrap's `--help` must list: the flags of `MountPlan::bwrap_args` that
/// came after bwrap 0.3.0, with the first release that has each one. The newest is
/// `--bind-fd`, so upstream bwrap must be 0.10.0 or newer; Ubuntu 24.04's 0.9.0
/// (`0.9.0-1ubuntu0.3`) has it as a backport. efr mounts no overlay with bwrap (the
/// layer helper does), so `--overlay` and `--tmp-overlay` (0.11.0) are not on the list.
pub const BWRAP_REQUIRED_FLAGS: &[&str] = &[
    // 0.8.0
    "--disable-userns",
    // 0.10.0, for CVE-2024-42472
    "--bind-fd",
    "--ro-bind-fd",
    // 0.3.0 or older
    "--ro-bind-data",
    // 0.4.0
    "--json-status-fd",
    // 0.5.0
    "--perms",
    // 0.3.0 or older
    "--args",
];

/// The address the self-test connects to: TEST-NET-1, which must fail at once.
pub const SELF_TEST_TCP_ADDR: &str = "192.0.2.1:9";

/// The fix of [`ModeFallback::HOME_PROJECT_REASON`](efr_protocol::ModeFallback::HOME_PROJECT_REASON):
/// a turn in a project at the home directory runs as `cautious`.
pub const HOME_PROJECT_FIX: &str =
    "register a narrower project, such as efr project add ~/dotfiles";

/// The flags of [`BWRAP_REQUIRED_FLAGS`] that `help`, bwrap's `--help` output, lacks.
pub fn bwrap_missing_flags(help: &str) -> Vec<&'static str> {
    BWRAP_REQUIRED_FLAGS
        .iter()
        .copied()
        .filter(|flag| !help.split_whitespace().any(|word| word == *flag))
        .collect()
}

/// The version in bwrap's `--version` output, such as `0.13.0` from `bubblewrap 0.13.0`.
pub fn parse_bwrap_version(text: &str) -> Option<String> {
    let word =
        text.split_whitespace().find(|word| word.starts_with(|c: char| c.is_ascii_digit()))?;
    word.chars().all(|c| c.is_ascii_digit() || c == '.').then(|| word.to_owned())
}

/// Why `auto` cannot run here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ProbeFailure {
    /// `sandbox.enabled = false`.
    Disabled,
    /// Not Linux on x86_64 or aarch64.
    Platform {
        /// The platform, such as `linux riscv64`.
        platform: String,
    },
    /// No bwrap.
    NoBwrap,
    /// bwrap lacks a flag.
    OldBwrap {
        /// Its version, when known.
        version: Option<String>,
        /// The first flag it lacks.
        flag: String,
    },
    /// bwrap is setuid.
    SetuidBwrap,
    /// bwrap is not owned by root, or the user can write it.
    BwrapNotTrusted {
        /// The program.
        path: PathBuf,
    },
    /// Unprivileged user namespaces are off.
    UsernsOff,
    /// AppArmor blocks user namespaces for bwrap.
    AppArmor,
    /// Landlock is off.
    LandlockOff,
    /// The Landlock ABI is below the minimum.
    AbiLow {
        /// The ABI found.
        found: u32,
    },
    /// The kernel lacks the Landlock fix for disconnected directories.
    Erratum,
    /// A program the sandbox relies on lies in a write root.
    InWriteRoot {
        /// What: `efr-sbx`, `bwrap` or `zsh`.
        what: String,
        /// Where.
        path: PathBuf,
    },
    /// The launcher copy does not match its source.
    LauncherMismatch,
    /// No `efr-sbx` lies next to efrd or in `../lib/efr/`.
    NoLauncher,
    /// The launcher's probe did not run or printed no report.
    ProbeFailed {
        /// What went wrong, in a few words.
        detail: String,
    },
    /// The hidden shell is not zsh, or its integration does not load.
    NoZsh,
    /// The hidden shell's `PATH` has a relative entry.
    RelativePath {
        /// The entry.
        entry: String,
    },
    /// A directory that the probe's fake call or the sandbox needs lies below `/tmp` or
    /// `/var/tmp`, which the sandbox replaces with its private ones.
    BelowTmp {
        /// What it is, such as `efr's state dir`.
        what: String,
        /// Where.
        path: PathBuf,
    },
    /// The self-test let something through.
    SelfTest {
        /// What got through.
        detail: String,
    },
    /// The self-test did not run every check, so it proves less than it must.
    SelfTestIncomplete {
        /// The checks that did not run.
        checks: Vec<String>,
        /// Each check that did not run, with the reason when the probe knows it.
        detail: String,
    },
}

impl ProbeFailure {
    /// The check that fails, as `SandboxCheck::name` names it.
    pub fn check_name(&self) -> &'static str {
        match self {
            ProbeFailure::Disabled => "enabled",
            ProbeFailure::Platform { .. } => "platform",
            ProbeFailure::NoBwrap
            | ProbeFailure::OldBwrap { .. }
            | ProbeFailure::SetuidBwrap
            | ProbeFailure::BwrapNotTrusted { .. } => "bwrap",
            ProbeFailure::UsernsOff | ProbeFailure::AppArmor => "user_namespaces",
            ProbeFailure::LandlockOff | ProbeFailure::AbiLow { .. } | ProbeFailure::Erratum => {
                "landlock"
            }
            ProbeFailure::InWriteRoot { .. }
            | ProbeFailure::LauncherMismatch
            | ProbeFailure::NoLauncher
            | ProbeFailure::ProbeFailed { .. } => "launcher",
            ProbeFailure::NoZsh => "zsh",
            ProbeFailure::RelativePath { .. } => "path",
            ProbeFailure::BelowTmp { .. } => "dirs",
            ProbeFailure::SelfTest { .. } | ProbeFailure::SelfTestIncomplete { .. } => "self_test",
        }
    }

    /// The reason, one sentence for the user.
    pub fn reason(&self) -> String {
        match self {
            ProbeFailure::Disabled => "sandbox.enabled = false".to_owned(),
            ProbeFailure::Platform { platform } => {
                format!("the sandbox needs Linux on x86_64 or aarch64, not {platform}")
            }
            ProbeFailure::NoBwrap => "bubblewrap is not installed".to_owned(),
            ProbeFailure::OldBwrap { version, flag } => match version {
                Some(version) => format!("bubblewrap {version} found; it lacks {flag}"),
                None => format!("the bubblewrap found lacks {flag}"),
            },
            ProbeFailure::SetuidBwrap => {
                "bubblewrap is setuid; efr needs the unprivileged build".to_owned()
            }
            ProbeFailure::BwrapNotTrusted { path } => {
                format!("{} is not owned by root or you can write it", path.display())
            }
            ProbeFailure::UsernsOff => {
                "unprivileged user namespaces are off (kernel.unprivileged_userns_clone = 0)"
                    .to_owned()
            }
            ProbeFailure::AppArmor => "AppArmor blocks user namespaces for bwrap \
                                       (kernel.apparmor_restrict_unprivileged_userns = 1)"
                .to_owned(),
            ProbeFailure::LandlockOff => "Landlock is not enabled".to_owned(),
            ProbeFailure::AbiLow { found } => {
                format!("Landlock ABI {found} found; auto needs {MIN_LANDLOCK_ABI} (Linux 7.1)")
            }
            ProbeFailure::Erratum => {
                "the kernel lacks the Landlock fix for disconnected directories".to_owned()
            }
            ProbeFailure::InWriteRoot { what, path } => {
                format!("{what} lies in a writable project ({})", path.display())
            }
            ProbeFailure::LauncherMismatch => {
                "the launcher copy does not match the installed efr-sbx".to_owned()
            }
            ProbeFailure::NoLauncher => "efr-sbx is not installed next to efrd".to_owned(),
            ProbeFailure::ProbeFailed { detail } => {
                format!("the sandbox launcher's probe failed: {detail}")
            }
            ProbeFailure::NoZsh => {
                "the hidden shell is not zsh with the efr integration".to_owned()
            }
            ProbeFailure::RelativePath { entry } => {
                format!("the hidden shell's PATH has the relative entry {entry:?}")
            }
            ProbeFailure::BelowTmp { what, path } => format!(
                "{what} {} lies below /tmp or /var/tmp, which the sandbox replaces with its own",
                path.display()
            ),
            ProbeFailure::SelfTest { detail } => {
                format!("the sandbox let a test through: {detail}")
            }
            ProbeFailure::SelfTestIncomplete { detail, .. } => {
                format!("the self-test did not run every check: {detail}")
            }
        }
    }

    /// What fixes it.
    pub fn fix(&self) -> String {
        match self {
            ProbeFailure::Disabled => "set sandbox.enabled = true".to_owned(),
            ProbeFailure::Platform { .. } => "use the cautious mode on this machine".to_owned(),
            ProbeFailure::NoBwrap => "Arch: pacman -S bubblewrap; Debian and Ubuntu: apt install \
                                      bubblewrap; Fedora: dnf install bubblewrap"
                .to_owned(),
            ProbeFailure::OldBwrap { .. } => "update the bubblewrap package".to_owned(),
            ProbeFailure::SetuidBwrap => "install the non-setuid build of bubblewrap".to_owned(),
            ProbeFailure::BwrapNotTrusted { .. } => {
                "use the distribution's bubblewrap, or set sandbox.bwrap to it".to_owned()
            }
            ProbeFailure::UsernsOff => "sysctl kernel.unprivileged_userns_clone=1".to_owned(),
            ProbeFailure::AppArmor => "add an AppArmor profile for bwrap with userns, or set \
                                       kernel.apparmor_restrict_unprivileged_userns=0"
                .to_owned(),
            ProbeFailure::LandlockOff => "add landlock to the lsm= boot parameter".to_owned(),
            ProbeFailure::AbiLow { .. } | ProbeFailure::Erratum => "use a newer kernel".to_owned(),
            ProbeFailure::InWriteRoot { .. } => {
                "install efr, or run efrd from outside the project".to_owned()
            }
            ProbeFailure::LauncherMismatch => {
                "restart efrd so it copies the launcher again".to_owned()
            }
            ProbeFailure::NoLauncher => {
                "install efr-sbx next to efrd or in ../lib/efr/ (just install does it)".to_owned()
            }
            ProbeFailure::ProbeFailed { .. } => {
                "run efr sandbox check; reinstall efr when it fails again".to_owned()
            }
            ProbeFailure::NoZsh => "set shell.program to zsh".to_owned(),
            ProbeFailure::RelativePath { .. } => {
                "remove the relative entry from PATH in your shell startup files".to_owned()
            }
            ProbeFailure::BelowTmp { .. } => "keep efr's state dir (EFR_STATE_DIR or EFR_HOME) \
                                              and XDG_RUNTIME_DIR outside /tmp and /var/tmp"
                .to_owned(),
            ProbeFailure::SelfTest { .. } => {
                "file an issue; efr sandbox check has the details".to_owned()
            }
            ProbeFailure::SelfTestIncomplete { .. } => {
                "remove the cause that the reason names, or file an issue".to_owned()
            }
        }
    }

    /// The failed check as `efr sandbox check` lists it.
    pub fn check(&self) -> SandboxCheck {
        SandboxCheck {
            name: self.check_name().to_owned(),
            outcome: CheckOutcome::Fail,
            detail: Some(self.reason()),
            fix: Some(self.fix()),
        }
    }
}

/// The Landlock check: ABI and errata.
pub fn check_landlock(abi: Option<u32>, errata: u32) -> Result<(), ProbeFailure> {
    match abi {
        None | Some(0) => Err(ProbeFailure::LandlockOff),
        Some(found) if found < MIN_LANDLOCK_ABI => Err(ProbeFailure::AbiLow { found }),
        Some(_) if errata & ERRATUM_DISCONNECTED_DIRS == 0 => Err(ProbeFailure::Erratum),
        Some(_) => Ok(()),
    }
}

/// What the probe found about one bwrap program.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BwrapFacts {
    /// The program, `None` when none was found.
    pub path: Option<PathBuf>,
    /// The owner's uid.
    pub owner_uid: u32,
    /// The file mode, with the setuid bit.
    pub mode: u32,
    /// True when the user may write the file.
    pub writable_by_user: bool,
    /// Its `--help` output.
    pub help: String,
    /// Its `--version` output.
    pub version: String,
}

/// The bwrap check: present, root's, not writable, not setuid, every flag.
pub fn check_bwrap(facts: &BwrapFacts) -> Result<(), ProbeFailure> {
    const SETUID: u32 = 0o4000;
    let Some(path) = &facts.path else { return Err(ProbeFailure::NoBwrap) };
    if facts.mode & SETUID != 0 {
        return Err(ProbeFailure::SetuidBwrap);
    }
    if facts.owner_uid != 0 || facts.writable_by_user {
        return Err(ProbeFailure::BwrapNotTrusted { path: path.clone() });
    }
    if let Some(flag) = bwrap_missing_flags(&facts.help).first() {
        let version = parse_bwrap_version(&facts.version);
        return Err(ProbeFailure::OldBwrap { version, flag: (*flag).to_owned() });
    }
    Ok(())
}

/// What `efr-sbx probe --json` prints.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ProbeReport {
    /// Every check in order; a failed one carries its reason and fix.
    pub checks: Vec<SandboxCheck>,
    /// The first failure, when one check failed.
    pub failure: Option<ProbeFailure>,
    /// The kernel's Landlock ABI.
    pub landlock_abi: Option<u32>,
    /// The errata the kernel reports fixed.
    pub errata: Option<u32>,
    /// The bwrap program.
    pub bwrap: Option<PathBuf>,
    /// Its version.
    pub bwrap_version: Option<String>,
    /// The cache mode that works here: the configured one, or `tmp` when an overlay
    /// on the state file system failed.
    pub cache_mode: CacheMode,
    /// The cost of one launch, in microseconds.
    pub launch_us: Option<u64>,
    /// The cost of one launch with the user's shell snapshot, in microseconds.
    pub snapshot_launch_us: Option<u64>,
    /// Warnings, one sentence each.
    pub warnings: Vec<String>,
}

impl ProbeReport {
    /// The status that efrd keeps and reports.
    pub fn status(&self) -> SandboxStatus {
        let failure = self.failure.as_ref();
        SandboxStatus {
            available: failure.is_none(),
            reason: failure.map(ProbeFailure::reason),
            fix: failure.map(ProbeFailure::fix),
            landlock_abi: self.landlock_abi,
            errata: self.errata,
            bwrap: self.bwrap.clone(),
            bwrap_version: self.bwrap_version.clone(),
            cache_mode: self.cache_mode,
            network_mode: NetworkMode::None,
            warnings: self.warnings.clone(),
        }
    }

    /// The report as JSON, as `efr-sbx probe --json` prints it.
    pub fn to_json(&self) -> Result<Vec<u8>, SandboxError> {
        serde_json::to_vec(self).map_err(|source| SandboxError::Json { what: "probe", source })
    }

    /// Reads the probe's output.
    pub fn from_json(bytes: &[u8]) -> Result<ProbeReport, SandboxError> {
        serde_json::from_slice(bytes).map_err(|source| SandboxError::Json { what: "probe", source })
    }
}

#[cfg(test)]
mod tests;

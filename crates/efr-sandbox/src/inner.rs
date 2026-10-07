//! [`InnerPolicy`]: what `efr-sbx run` sends to `efr-sbx inner` on the policy fd.
//!
//! The inner stage runs inside bwrap's namespaces as trusted code in an untrusted
//! place. It reads this policy, moves the terminal copy to fd 2 and the records pipe
//! to fd 3, closes every other descriptor, applies Landlock, then seccomp, and executes
//! the child shell. It reports its own failure as a `setup-error` record on fd 3.

use std::ffi::OsString;

use serde::{Deserialize, Serialize};

use crate::SandboxError;
use crate::landlock::LandlockPolicy;
use crate::plan::MountPlan;
use crate::seccomp::SeccompProfile;

/// The most bytes of a policy.
pub const MAX_POLICY_BYTES: usize = 1024 * 1024;

/// The fd that the child sees the records pipe on.
pub const RECORDS_FD: i32 = 3;

/// Everything the inner stage needs, as data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct InnerPolicy {
    /// The Landlock rule set.
    pub landlock: LandlockPolicy,
    /// The seccomp filter.
    pub seccomp: SeccompProfile,
    /// The descriptor of the records pipe's write end, moved to [`RECORDS_FD`].
    pub records_fd: i32,
    /// The descriptor of the terminal copy, moved to fd 2 so the child's stderr is the
    /// terminal again while bwrap's own stderr went to the launcher.
    pub terminal_fd: Option<i32>,
    /// The child: `zsh -f <inside>/child.zsh`.
    pub argv: Vec<OsString>,
}

impl InnerPolicy {
    /// The policy of `plan`, with the descriptors the launcher passes.
    pub fn new(plan: &MountPlan, records_fd: i32, terminal_fd: Option<i32>) -> InnerPolicy {
        InnerPolicy {
            landlock: plan.landlock(),
            seccomp: SeccompProfile::phase1(),
            records_fd,
            terminal_fd,
            argv: plan.child_argv().to_vec(),
        }
    }

    /// The policy as the bytes of the policy pipe.
    pub fn to_json(&self) -> Result<Vec<u8>, SandboxError> {
        serde_json::to_vec(self).map_err(|source| SandboxError::Json { what: "policy", source })
    }

    /// Reads the policy pipe.
    pub fn from_json(bytes: &[u8]) -> Result<InnerPolicy, SandboxError> {
        if bytes.len() > MAX_POLICY_BYTES {
            return Err(SandboxError::TooLarge {
                what: "policy",
                len: bytes.len(),
                max: MAX_POLICY_BYTES,
            });
        }
        serde_json::from_slice(bytes)
            .map_err(|source| SandboxError::Json { what: "policy", source })
    }
}

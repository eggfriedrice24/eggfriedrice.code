//! [`SbxError`], the one error type of efr-sbx.

use std::fmt;
use std::io;
use std::path::PathBuf;

use efr_sandbox::{SandboxError, SeccompAction, SeccompArch, SyscallRule};

/// Every way a launch, the inner stage or the probe can fail.
///
/// The launcher turns an error after the call dir check into a setup failure in
/// `result.json` (exit status 125); the line never runs anywhere else.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub(crate) enum SbxError {
    /// The call dir is not one that efrd made for this user.
    #[error("the call dir {path:?} is not valid: {problem}")]
    CallDir {
        /// The call dir.
        path: PathBuf,
        /// What is wrong with it.
        problem: CallDirProblem,
    },

    /// A file system call failed.
    #[error("efr-sbx could not {what} {path:?}")]
    Io {
        /// What it tried, such as `read` or `write`.
        what: &'static str,
        /// The path.
        path: PathBuf,
        /// The error.
        #[source]
        source: io::Error,
    },

    /// A call that names no path failed: a pipe, a descriptor, a process step.
    #[error("efr-sbx could not {what}")]
    Os {
        /// What it tried, such as `make the records pipe`.
        what: &'static str,
        /// The error.
        #[source]
        source: io::Error,
    },

    /// The spec, the plan, the records or a state file was refused.
    #[error(transparent)]
    Sandbox(#[from] SandboxError),

    /// Landlock refused the rule set.
    #[error("Landlock refused the rule set")]
    Landlock {
        /// The crate's error.
        #[source]
        source: landlock::RulesetError,
    },

    /// Landlock did not enforce every rule.
    #[error("Landlock did not enforce the whole rule set")]
    LandlockPartial,

    /// The policy asks for more than this build or this kernel knows.
    #[error("the kernel or this build lacks Landlock ABI {abi} or erratum mask {errata:#x}")]
    LandlockTooOld {
        /// The ABI the policy needs.
        abi: u32,
        /// The errata the policy needs fixed.
        errata: u32,
    },

    /// seccomp could not compile or install the filter.
    #[error("seccomp could not install the filter")]
    Seccomp {
        /// The crate's error.
        #[source]
        source: seccompiler::Error,
    },

    /// The launcher runs on an architecture that has no seccomp filter.
    #[error("the seccomp profile has no filter for the architecture {arch}")]
    SeccompUnknownArch {
        /// The architecture, as Rust names it.
        arch: &'static str,
    },

    /// The seccomp profile does not cover the architecture the launcher runs on.
    #[error("the seccomp profile does not cover {arch:?}")]
    SeccompArchNotCovered {
        /// The architecture.
        arch: SeccompArch,
    },

    /// The seccomp profile asks for actions that this build cannot install.
    #[error(
        "the seccomp profile asks for {default:?} by default and {other_arch:?} elsewhere; this build installs allow and kill"
    )]
    SeccompActions {
        /// The default action.
        default: SeccompAction,
        /// The action for another architecture.
        other_arch: SeccompAction,
    },

    /// The seccomp profile holds a rule this build does not know.
    #[error("the seccomp profile holds a rule this build does not know: {rule:?}")]
    SeccompUnknownRule {
        /// The rule.
        rule: SyscallRule,
    },

    /// The seccomp filters could not be encoded for the compiler.
    #[error("the seccomp filters could not be encoded")]
    SeccompEncode {
        /// The encoder's error.
        #[source]
        source: serde_json::Error,
    },

    /// A program could not start.
    #[error("efr-sbx could not start {program:?}")]
    Spawn {
        /// The program.
        program: PathBuf,
        /// The error.
        #[source]
        source: io::Error,
    },

    /// SIGINT or SIGQUIT came before the launcher started the call, so it did not.
    #[error("signal {signal} stopped the call before it started")]
    Interrupted {
        /// The signal.
        signal: i32,
    },

    /// Something that a later phase builds.
    #[error("{what} comes in a later phase of the auto sandbox")]
    LaterPhase {
        /// What.
        what: &'static str,
    },
}

/// What is wrong with a call dir.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub(crate) enum CallDirProblem {
    /// The path is relative or not in normal form.
    NotAbsolute,
    /// It is a symbolic link, or one is on the way.
    Link,
    /// It is not a directory, or a file in it is not a regular file.
    Kind,
    /// Another user owns it or a file in it.
    Owner,
    /// Group or others may read, write or enter it.
    Mode,
    /// The spec names another call dir or conversation.
    Ids,
    /// A file is missing or too large.
    File,
}

impl fmt::Display for CallDirProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            CallDirProblem::NotAbsolute => "it is not an absolute path in normal form",
            CallDirProblem::Link => "a symbolic link is on the way",
            CallDirProblem::Kind => "it or a file in it is of the wrong kind",
            CallDirProblem::Owner => "another user owns it or a file in it",
            CallDirProblem::Mode => "other users may use it or a file in it",
            CallDirProblem::Ids => "its spec names another call or conversation",
            CallDirProblem::File => "a file in it is missing or too large",
        })
    }
}

impl SbxError {
    /// An [`SbxError::Io`] for `path`.
    pub(crate) fn io(what: &'static str, path: impl Into<PathBuf>, source: io::Error) -> Self {
        SbxError::Io { what, path: path.into(), source }
    }

    /// An [`SbxError::Os`].
    pub(crate) fn os(what: &'static str, source: io::Error) -> Self {
        SbxError::Os { what, source }
    }

    /// The error and its sources in one line, for `setup_error` and the inner stage's
    /// report: what failed, then why.
    pub(crate) fn chain(&self) -> String {
        let mut text = self.to_string();
        let mut source = std::error::Error::source(self);
        while let Some(cause) = source {
            text.push_str(": ");
            text.push_str(&cause.to_string());
            source = cause.source();
        }
        text
    }
}

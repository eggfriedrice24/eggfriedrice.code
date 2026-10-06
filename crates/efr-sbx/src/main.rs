//! `efr-sbx`: the launcher of the `auto` sandbox.
//!
//! The hidden zsh's wrapper runs `efr-sbx run --call-dir $CALL` for each model call in
//! `auto`. The launcher reads the spec that efrd wrote, starts bwrap with the plan of
//! `efr-sandbox`, and inside bwrap `efr-sbx inner` applies Landlock and seccomp before
//! it runs the child shell; or, for an approved exit, it runs the exit child as a
//! subreaper. efrd runs `efr-sbx probe --json` to learn whether `auto` can run here.
//! `bridge` is the seam of phase 2.
//!
//! Allowed dependencies: `efr-sandbox` and `efr-protocol`. No async runtime: the
//! launcher is a small process in the foreground job of the hidden shell, and every
//! rule it applies is a pure function of `efr-sandbox`. The one module that allows
//! `unsafe_code` is `fds.rs` (ADR 0007).

mod bridge;
mod call;
mod call_dir;
mod error;
mod exit_child;
// NOTE: descriptor numbers that the process does not own yet: `close_range`, `dup3` onto a
// fixed number, `fcntl` on a number, and the probe's raw system calls have no safe
// wrapper. It is the one module of efr-sbx that allows `unsafe_code` (ADR 0007; the
// allowlist is in `xtask/src/tidy.rs`).
#[allow(unsafe_code)]
mod fds;
mod finish;
mod guard;
mod inner;
mod landlock;
mod launch;
mod os;
mod probe;
mod real_fs;
mod run;
mod seccomp;
mod self_test;
mod signals;
#[cfg(test)]
mod testing;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

/// The launcher of the auto sandbox. efr runs it; a person runs `probe`.
#[derive(Debug, Parser)]
#[command(name = "efr-sbx", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    /// Runs one call from its call dir.
    Run {
        /// The call dir, `$R/sbx/<conversation>/<call>`.
        #[arg(long)]
        call_dir: PathBuf,
    },
    /// The stage inside bwrap: Landlock, seccomp, then the child shell.
    #[command(hide = true)]
    Inner {
        /// The descriptor that carries the policy.
        #[arg(long)]
        policy_fd: i32,
    },
    /// Phase 2: copies TCP from inside to the call's proxy socket.
    #[command(hide = true)]
    Bridge {
        /// The listening descriptor inside.
        #[arg(long)]
        listen_fd: i32,
        /// The call's proxy socket.
        #[arg(long)]
        socket: PathBuf,
    },
    /// Checks whether the auto sandbox can run here.
    Probe(probe::ProbeArgs),
    /// The probe's checks from inside a sandbox.
    #[command(hide = true)]
    SelfTest(self_test::SelfTestArgs),
    /// Prints the kernel's Landlock ABI after a rule set that restricts nothing.
    #[command(hide = true)]
    LandlockAbi,
}

fn main() -> ExitCode {
    run::run(Cli::parse().command)
}

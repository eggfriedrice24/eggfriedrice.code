//! `efr-sbx`: the launcher of the `auto` sandbox.
//!
//! The hidden zsh's wrapper runs `efr-sbx run --call-dir $CALL` for each model call in
//! `auto`. The launcher reads the spec that efrd wrote, starts bwrap with the plan of
//! `efr-sandbox`, and inside bwrap `efr-sbx inner` applies Landlock and seccomp before
//! it runs the child shell; or, for an approved exit, it runs the exit child as a
//! subreaper. `layers` mounts the cache overlays of a call after bwrap's setup. efrd
//! runs `efr-sbx probe --json` to learn whether `auto` can run here. efrd starts a
//! background job itself with `efr-sbx run --job`.
//! The inner stage of a proxy call starts `bridge`, which copies the call's TCP to its
//! proxy socket. efrd runs `statvfs` to measure the host's mounts outside the sandbox,
//! and the `df` and `findmnt` of a call print them with `mounts`. efrd runs
//! `xdg-dbus-proxy` through `dbus-proxy`, which confines it first.
//!
//! Allowed dependencies: `efr-sandbox` and `efr-protocol`. No async runtime: the
//! launcher is a small process in the foreground job of the hidden shell, and every
//! rule it applies is a pure function of `efr-sandbox`. The one module that allows
//! `unsafe_code` is `fds.rs` (ADR 0007).

mod bridge;
mod call;
mod call_dir;
mod dbus_proxy;
mod deletes;
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
mod guard_check;
mod helper;
mod inner;
mod job;
mod landlock;
mod launch;
mod layer_lock;
mod layers;
mod mounts;
mod os;
mod probe;
mod real_fs;
mod run;
mod seccomp;
mod self_test;
mod signals;
mod statvfs;
#[cfg(test)]
mod testing;
mod timings;

use std::ffi::OsString;
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
        /// The call dir, `$R/sbx/<conversation>/<call>`, or for a job
        /// `$R/sbx/<conversation>/jobs/<call>`.
        #[arg(long)]
        call_dir: PathBuf,
        /// Runs a job that efrd starts: no terminal, a session of its own, a lifeline
        /// on stdin and the output pipe on stdout.
        #[arg(long)]
        job: bool,
    },
    /// The stage inside bwrap: Landlock, seccomp, then the child shell.
    #[command(hide = true)]
    Inner {
        /// The descriptor that carries the policy.
        #[arg(long)]
        policy_fd: i32,
    },
    /// Mounts the cache overlays of a call once bwrap's setup is done.
    #[command(hide = true)]
    Layers {
        /// The inner stage's process id.
        #[arg(long)]
        pid: u32,
    },
    /// The bridge of a proxy call: copies TCP from `127.0.0.1:3128` inside to the
    /// call's proxy socket.
    #[command(hide = true)]
    Bridge(bridge::BridgeArgs),
    /// Confines `xdg-dbus-proxy` (Landlock, seccomp, a descriptor limit) and runs it:
    /// efrd's filtered bus of a call. The sync pipe on stdout moves to fd 3.
    #[command(hide = true)]
    DbusProxy(dbus_proxy::DbusProxyArgs),
    /// Compares a running contained job's git surface with the one of its start, and
    /// moves what breaks a rule to its quarantine. Exit status 3 when it found a change.
    GuardCheck {
        /// The job's call dir, `$R/sbx/<conversation>/jobs/<call>`.
        #[arg(long)]
        call_dir: PathBuf,
        /// The job's state dir, `$S/sandbox/<conversation>/jobs/<call>`.
        #[arg(long)]
        state_dir: PathBuf,
    },
    /// Checks whether the auto sandbox can run here.
    Probe(probe::ProbeArgs),
    /// The probe's checks from inside a sandbox.
    #[command(hide = true)]
    SelfTest(Box<self_test::SelfTestArgs>),
    /// Prints the kernel's Landlock ABI after a rule set that restricts nothing.
    #[command(hide = true)]
    LandlockAbi,
    /// Inside a call: prints the host's mounts from the snapshot next to the launcher,
    /// as `df` or `findmnt --df` would. Exit status 2 with no output when it cannot.
    #[command(hide = true)]
    Mounts {
        /// Which table.
        #[command(subcommand)]
        table: MountsTable,
    },
    /// Measures each path of stdin (NUL-separated) with `statvfs`, one thread per
    /// path, and prints one JSON line per path as each answers. efrd runs it and kills
    /// it at its deadline.
    #[command(hide = true)]
    Statvfs,
}

/// The tables of `mounts`. They take their arguments raw, so `-h` is `df`'s `-h`.
#[derive(Debug, Subcommand)]
pub(crate) enum MountsTable {
    /// The `df` table, for `df`'s options.
    #[command(disable_help_flag = true, disable_version_flag = true)]
    Df {
        /// `df`'s arguments.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<OsString>,
    },
    /// The `findmnt --df` table; `-I` or `--dfi` for the inodes.
    #[command(name = "findmnt-df", disable_help_flag = true, disable_version_flag = true)]
    FindmntDf {
        /// `findmnt`'s arguments: `-D`, `--df`, `-I` and `--dfi` only.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<OsString>,
    },
}

fn main() -> ExitCode {
    run::run(Cli::parse().command)
}

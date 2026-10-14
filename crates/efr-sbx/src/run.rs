//! Runs one subcommand.

use std::process::ExitCode;

use crate::Command;
use crate::{bridge, call, guard_check, inner, layers, mounts, probe, self_test, statvfs};

/// Runs `command` and returns the process's exit status.
pub(crate) fn run(command: Command) -> ExitCode {
    match command {
        Command::Run { call_dir, job } => call::main(&call_dir, job),
        Command::GuardCheck { call_dir, state_dir } => guard_check::main(&call_dir, &state_dir),
        Command::Inner { policy_fd } => inner::main(policy_fd),
        Command::Layers { pid } => layers::main(pid),
        Command::Bridge(args) => bridge::main(&args),
        Command::Probe(args) => probe::main(&args),
        Command::SelfTest(args) => self_test::main(&args),
        Command::LandlockAbi => probe::abi_main(),
        Command::Mounts { table } => mounts::main(table),
        Command::Statvfs => statvfs::main(),
    }
}

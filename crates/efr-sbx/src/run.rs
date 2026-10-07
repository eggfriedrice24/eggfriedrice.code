//! Runs one subcommand.

use std::process::ExitCode;

use crate::Command;
use crate::{bridge, call, inner, layers, probe, self_test};

/// Runs `command` and returns the process's exit status.
pub(crate) fn run(command: Command) -> ExitCode {
    match command {
        Command::Run { call_dir } => call::main(&call_dir),
        Command::Inner { policy_fd } => inner::main(policy_fd),
        Command::Layers { pid } => layers::main(pid),
        Command::Bridge { listen_fd, socket } => bridge::main(listen_fd, &socket),
        Command::Probe(args) => probe::main(&args),
        Command::SelfTest(args) => self_test::main(&args),
        Command::LandlockAbi => probe::abi_main(),
    }
}

//! The only module in the xtask that writes to the terminal, so the print lints stay
//! denied everywhere else (the same escape hatch `efr-cli/src/output.rs` will use).
#![expect(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "the xtask is a command-line tool and this module is its single output path"
)]

/// A normal result line, for humans or for CI logs.
pub(crate) fn line(text: &str) {
    println!("{text}");
}

/// A failure that stops the command, as opposed to a rule violation it reports.
pub(crate) fn error(text: &str) {
    eprintln!("error: {text}");
}

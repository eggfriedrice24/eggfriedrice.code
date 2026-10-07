//! `efrd`: parses the flags, loads the config, sets up tracing and runs the daemon.
//!
//! `efrd --print-config` prints the effective configuration with the source of every
//! value and exits. Everything else is in the library's `run`.

use std::io::Write as _;
use std::process::ExitCode;

use clap::Parser;
use efr_daemon::{DaemonError, Deps, Flags};
use efr_stdx::env::Env;

/// The efr daemon. It runs as the systemd user unit `efrd.service`.
#[derive(Debug, Parser)]
#[command(name = "efrd", version, about)]
struct Args {
    /// The tracing filter, in EnvFilter syntax (overrides EFR_LOG and the config).
    #[arg(long, value_name = "FILTER")]
    log: Option<String>,
    /// The screen backend: auto, vt100 or ghostty (overrides EFR_SCREEN and the config).
    #[arg(long, value_name = "BACKEND")]
    screen: Option<String>,
    /// Print the effective configuration with the source of each value, then exit.
    #[arg(long)]
    print_config: bool,
    /// Print whether this build has the sandbox's test seams (on or off), then exit;
    /// `just install` refuses a build that has them.
    #[arg(long, hide = true)]
    test_seams: bool,
}

fn main() -> ExitCode {
    let args = Args::parse();
    match daemon(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let mut stderr = std::io::stderr();
            let _ = match error.downcast_ref::<DaemonError>() {
                Some(DaemonError::AlreadyRunning { path }) => writeln!(
                    stderr,
                    "efrd: another efrd is running (it holds {}); exiting",
                    path.display()
                ),
                _ => writeln!(stderr, "efrd: {error:#}"),
            };
            ExitCode::FAILURE
        }
    }
}

fn daemon(args: Args) -> anyhow::Result<()> {
    if args.test_seams {
        let seams = if efr_daemon::TEST_SEAMS { "on\n" } else { "off\n" };
        std::io::stdout().write_all(seams.as_bytes())?;
        return Ok(());
    }
    let deps = Deps::from_process()?;
    let flags = Flags::new(args.log, args.screen);
    let config = efr_daemon::load_settings(deps.dirs.config(), &Env::process(), &flags)?;
    if args.print_config {
        std::io::stdout().write_all(config.effective().as_bytes())?;
        return Ok(());
    }
    let (_, log) = efr_daemon::init_telemetry(&config.log);
    let deps = deps.with_log_filter(log);
    let runtime = efr_daemon::build_runtime()?;
    runtime.block_on(async {
        let shutdown = efr_daemon::shutdown_on_signals()?;
        efr_daemon::run(config, deps, shutdown).await
    })?;
    Ok(())
}

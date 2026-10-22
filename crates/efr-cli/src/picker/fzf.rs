//! fzf as a child process: the `PATH` walk, `fzf --version`, and one run of the
//! picker.
//!
//! efr sets no terminal mode and starts no key thread while fzf runs: fzf sets and
//! restores its own mode on `/dev/tty`. Ctrl+C reaches fzf and efr together (one
//! foreground process group); efr waits for fzf to end, at most 1 s, then ends it, so
//! the prompt never comes back under a live picker. SIGTERM and SIGHUP: efr sends
//! SIGTERM to fzf and waits the same time.

use std::ffi::OsString;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use efr_stdx::time::Clock;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

use super::{
    FZF_FLOOR, Finding, Fzf, NoPicker, Outcome, PickRequest, Picker, Picking, Stops, find_on_path,
    fzf_args, fzf_query_opts, outcome, parse_version,
};
use crate::error::CliError;

/// How long `fzf --version` may take.
const VERSION_WAIT: Duration = Duration::from_secs(1);

/// How long efr waits for fzf to end after Ctrl+C or a SIGTERM before it kills fzf.
const GRACE: Duration = Duration::from_secs(1);

/// fzf's own variables that could change the picker, such as into a popup or a full
/// screen: efr removes them, and sets `FZF_DEFAULT_OPTS` itself for a first query.
const FZF_DEFAULTS: [&str; 3] =
    ["FZF_DEFAULT_OPTS", "FZF_DEFAULT_OPTS_FILE", "FZF_DEFAULT_COMMAND"];

/// The fzf of the `PATH` of this process.
#[derive(Debug)]
pub(crate) struct ProcessFzf {
    path: Option<OsString>,
    clock: Arc<dyn Clock>,
}

impl ProcessFzf {
    /// fzf on `path`, the value of `PATH`, with `clock` for the waits.
    pub(crate) fn new(path: Option<OsString>, clock: Arc<dyn Clock>) -> ProcessFzf {
        ProcessFzf { path, clock }
    }

    /// The version of the fzf at `path`.
    async fn version(&self, path: &Path) -> Result<super::Version, NoPicker> {
        let mut command = efr_stdx::process::command(path, "/");
        for name in FZF_DEFAULTS {
            command.env_remove(name);
        }
        command
            .arg("--version")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let output = match self.clock.timeout(VERSION_WAIT, command.output()).await {
            Ok(Ok(output)) if output.status.success() => output,
            _ => return Err(NoPicker::VersionFailed),
        };
        let text = String::from_utf8_lossy(&output.stdout);
        parse_version(&text).ok_or(NoPicker::VersionFailed)
    }

    /// Waits at most [`GRACE`] for `child` to end, then kills it.
    async fn end(&self, child: &mut tokio::process::Child) {
        if self.clock.timeout(GRACE, child.wait()).await.is_err() {
            let _ = child.start_kill();
            let _ = child.wait().await;
        }
    }
}

/// True for a file that someone may run.
fn executable(path: &Path) -> bool {
    std::fs::metadata(path)
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

/// How a run of fzf ended.
enum Ended {
    /// fzf exited with this status (`None` for a signal) and printed this.
    Exited(Option<i32>, Vec<u8>),
    Interrupted,
    Terminated(i32),
}

impl Picker for ProcessFzf {
    fn find(&self) -> Finding<'_> {
        Box::pin(async move {
            let path = self.path.as_deref().ok_or(NoPicker::NotFound)?;
            let path = find_on_path(path, executable).ok_or(NoPicker::NotFound)?;
            let version = self.version(&path).await?;
            if version < FZF_FLOOR {
                return Err(NoPicker::TooOld(version));
            }
            Ok(Fzf { path, version })
        })
    }

    fn efr(&self) -> Option<PathBuf> {
        std::env::current_exe().ok()
    }

    fn run(&self, fzf: Fzf, request: PickRequest, stops: Stops) -> Picking<'_> {
        Box::pin(async move {
            let Stops { mut interrupt, terminate } = stops;
            // NOTE: the first poll installs the handler of SIGINT, so Ctrl+C in fzf
            // does not end efr while fzf still holds the terminal.
            let _ = futures::poll!(&mut interrupt);
            let mut command = efr_stdx::process::command(&fzf.path, &request.cwd);
            for name in FZF_DEFAULTS {
                command.env_remove(name);
            }
            if let Some(query) = &request.query {
                command.env("FZF_DEFAULT_OPTS", fzf_query_opts(query));
            }
            // NOTE: fzf runs each command with `$SHELL -c`; zsh would read ~/.zshenv on
            // each key.
            command
                .env("SHELL", "/bin/sh")
                .args(fzf_args(&request.look))
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit())
                .kill_on_drop(true);
            let Ok(mut child) = command.spawn() else {
                return Ok(Outcome::Failed(None));
            };
            let stdin = child.stdin.take();
            let stdout = child.stdout.take();
            let ended = {
                let rows = &request.rows;
                let work = async {
                    // fzf may end before it read every row; the rest does not matter.
                    let write = async {
                        if let Some(mut stdin) = stdin {
                            let _ = stdin.write_all(rows).await;
                        }
                    };
                    let read = async {
                        let mut printed = Vec::new();
                        if let Some(mut stdout) = stdout {
                            let _ = stdout.read_to_end(&mut printed).await;
                        }
                        printed
                    };
                    let ((), printed) = tokio::join!(write, read);
                    let status = child.wait().await.ok().and_then(|status| status.code());
                    (status, printed)
                };
                tokio::select! {
                    (status, printed) = work => Ended::Exited(status, printed),
                    () = &mut interrupt => Ended::Interrupted,
                    signal = terminate => Ended::Terminated(signal),
                }
            };
            match ended {
                Ended::Exited(status, printed) => Ok(outcome(status, &printed)),
                Ended::Interrupted => {
                    self.end(&mut child).await;
                    Ok(Outcome::Interrupted)
                }
                Ended::Terminated(signal) => {
                    let pid = child
                        .id()
                        .and_then(|pid| i32::try_from(pid).ok())
                        .and_then(rustix::process::Pid::from_raw);
                    if let Some(pid) = pid {
                        let _ = rustix::process::kill_process(pid, rustix::process::Signal::TERM);
                    }
                    self.end(&mut child).await;
                    Err(CliError::Ended { signal })
                }
            }
        })
    }
}

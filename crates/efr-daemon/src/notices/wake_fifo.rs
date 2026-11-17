//! The wake FIFO of a terminal: `notices/<tty>.wake` next to its notice file.
//!
//! The zsh plugin of the terminal holds the FIFO open while its prompt waits
//! (`zle -F`). After each notice line for the terminal, efrd writes one line to the
//! FIFO: the body of the desktop notice that the row asks for
//! (`efr_stdx::text::notice_body`), or an empty line for a row that asks for none. The
//! plugin then prints the rows above the waiting prompt at once, and it sends the
//! desktop notice for each body. zsh has no builtin that makes a FIFO, so efrd makes
//! it, when a prompt names its terminal ([`ensure`]).
//!
//! A write never blocks the daemon: no FIFO, no reader and a full pipe skip the line,
//! and the rows then show at the next prompt, as before. A path that is not a FIFO
//! gets nothing. Each process of the user can write the FIFO, so the plugin does not
//! trust its text. efrd never removes a FIFO: the runtime root goes at logout.

use std::fs::OpenOptions;
use std::io::Write as _;
use std::os::unix::fs::{FileTypeExt as _, OpenOptionsExt as _};
use std::path::{Path, PathBuf};

use rustix::fs::{Mode, OFlags};
use rustix::io::Errno;

use crate::DaemonError;

/// What follows the terminal's name in the name of its FIFO.
const SUFFIX: &str = ".wake";

/// The FIFO of the terminal whose notice file is `name` in `dir`.
pub(crate) fn path(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!("{name}{SUFFIX}"))
}

/// Makes the FIFO of `tty` under `dir`, mode 0600, unless a file is at its path
/// already. `None` for a terminal without a safe name (`notices::file_name`). Blocks;
/// async code calls it on the blocking pool.
pub(crate) fn ensure(dir: &Path, tty: &str) -> Result<Option<PathBuf>, DaemonError> {
    let Some(name) = super::file_name(tty) else {
        return Ok(None);
    };
    super::make_dir(dir)?;
    let path = path(dir, &name);
    match rustix::fs::mkfifoat(rustix::fs::CWD, &path, Mode::from_raw_mode(super::FILE_MODE)) {
        Ok(()) | Err(Errno::EXIST) => Ok(Some(path)),
        Err(errno) => Err(DaemonError::Io { path, source: errno.into() }),
    }
}

/// Writes `body` and a newline to the FIFO at `path`, or a newline alone without a
/// body. `body` is one line of at most `NOTICE_BODY_MAX` characters, so one write
/// carries the line whole (`PIPE_BUF`). True when the line went out; false when there
/// is no FIFO, no reader or no room, or the path is not a FIFO. Never blocks.
pub(crate) fn send(path: &Path, body: Option<&str>) -> Result<bool, DaemonError> {
    let io = |source| DaemonError::Io { path: path.to_path_buf(), source };
    // NOTE: std opens with O_CLOEXEC; a terminal is never opened as efrd's own.
    let flags = OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::NOCTTY;
    let opened = OpenOptions::new().write(true).custom_flags(flags.bits().cast_signed()).open(path);
    let mut file = match opened {
        Ok(file) => file,
        // NOTE: NXIO is a FIFO without a reader, and LOOP a symbolic link.
        Err(error) if skipped(&error, &[Errno::NOENT, Errno::NXIO, Errno::LOOP]) => {
            return Ok(false);
        }
        Err(error) => return Err(io(error)),
    };
    if !file.metadata().map_err(io)?.file_type().is_fifo() {
        return Ok(false);
    }
    let mut line = String::with_capacity(body.map_or(0, str::len) + 1);
    line.push_str(body.unwrap_or_default());
    line.push('\n');
    match file.write(line.as_bytes()) {
        Ok(_) => Ok(true),
        Err(error) if skipped(&error, &[Errno::AGAIN]) => Ok(false),
        Err(error) => Err(io(error)),
    }
}

/// True when `error` is one of `errnos`.
fn skipped(error: &std::io::Error, errnos: &[Errno]) -> bool {
    Errno::from_io_error(error).is_some_and(|errno| errnos.contains(&errno))
}

#[cfg(test)]
mod tests;

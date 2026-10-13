//! `efr-sbx statvfs`: the sizes of the host's mounts for efrd's mount view.
//!
//! efrd runs it outside the sandbox with the mount targets on stdin, NUL-separated.
//! Each path gets a thread of its own, so one mount that hangs (a FUSE server that does
//! not answer, a hard NFS mount) does not hold back the others. A line goes out as each
//! answer comes. A thread in such a wait cannot be stopped; efrd ends the whole
//! process with SIGKILL at its deadline, which also ends a FUSE or NFS wait.

use std::ffi::OsString;
use std::io::{Read as _, Write as _};
use std::os::unix::ffi::OsStringExt as _;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::mpsc;

use efr_sandbox::{MAX_STATVFS_PATH_BYTES, MAX_STATVFS_PATHS, StatvfsLine, StatvfsValues};

use crate::error::SbxError;

/// `EAGAIN`: a thread for the path could not start.
const EAGAIN: i32 = 11;

/// Measures the paths of stdin and prints a line for each.
pub(crate) fn main() -> ExitCode {
    let paths = match read_paths(std::io::stdin().lock()) {
        Ok(paths) => paths,
        Err(error) => {
            let _ = writeln!(std::io::stderr(), "efr-sbx statvfs: {}", error.chain());
            return ExitCode::FAILURE;
        }
    };
    let (answers, answered) = mpsc::channel::<StatvfsLine>();
    for path in paths {
        let name = path.to_string_lossy().into_owned();
        let failed =
            StatvfsLine { path: name.clone(), ok: None, errno: Some(EAGAIN), device: None };
        let sender = answers.clone();
        let started = std::thread::Builder::new()
            .name("statvfs".to_owned())
            .stack_size(64 * 1024)
            .spawn(move || {
                let _ = sender.send(measure(name, &path));
            });
        if started.is_err() {
            let _ = answers.send(failed);
        }
    }
    drop(answers);
    let mut out = std::io::stdout().lock();
    for line in answered {
        let Ok(mut bytes) = serde_json::to_vec(&line) else { continue };
        bytes.push(b'\n');
        if out.write_all(&bytes).and_then(|()| out.flush()).is_err() {
            return ExitCode::FAILURE;
        }
    }
    ExitCode::SUCCESS
}

/// The paths of `input`: NUL-separated, at most [`MAX_STATVFS_PATHS`] of at most
/// [`MAX_STATVFS_PATH_BYTES`] bytes each.
pub(crate) fn read_paths(input: impl std::io::Read) -> Result<Vec<PathBuf>, SbxError> {
    let limit = MAX_STATVFS_PATHS * (MAX_STATVFS_PATH_BYTES + 1);
    let mut bytes = Vec::new();
    input
        .take(u64::try_from(limit + 1).unwrap_or(u64::MAX))
        .read_to_end(&mut bytes)
        .map_err(|source| SbxError::os("read the paths", source))?;
    if bytes.len() > limit {
        return Err(SbxError::StatvfsInput { problem: "stdin holds too many bytes" });
    }
    let mut paths = Vec::new();
    for path in bytes.split(|byte| *byte == 0).filter(|path| !path.is_empty()) {
        if path.len() > MAX_STATVFS_PATH_BYTES {
            return Err(SbxError::StatvfsInput { problem: "a path is too long" });
        }
        if paths.len() == MAX_STATVFS_PATHS {
            return Err(SbxError::StatvfsInput { problem: "stdin holds too many paths" });
        }
        paths.push(PathBuf::from(OsString::from_vec(path.to_vec())));
    }
    Ok(paths)
}

/// The line for `path`, which `name` prints: its sizes, and the device that `stat`
/// gives for it.
fn measure(name: String, path: &std::path::Path) -> StatvfsLine {
    let sizes = rustix::fs::statvfs(path);
    let device = rustix::fs::stat(path)
        .ok()
        .map(|stat| (rustix::fs::major(stat.st_dev), rustix::fs::minor(stat.st_dev)));
    match sizes {
        Ok(stat) => StatvfsLine {
            path: name,
            ok: Some(StatvfsValues {
                blocks: stat.f_blocks,
                bfree: stat.f_bfree,
                bavail: stat.f_bavail,
                frsize: stat.f_frsize,
                files: stat.f_files,
                ffree: stat.f_ffree,
            }),
            errno: None,
            device,
        },
        Err(errno) => {
            StatvfsLine { path: name, ok: None, errno: Some(errno.raw_os_error()), device: None }
        }
    }
}

#[cfg(test)]
mod tests;

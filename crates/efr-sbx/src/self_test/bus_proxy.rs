//! `efr-sbx self-test --bus-proxy`: the checks of the bus proxy's domain, from a
//! program that `efr-sbx dbus-proxy` runs in the place of `xdg-dbus-proxy`. The tests
//! run the launcher itself as that program, because the domain lets a program read only
//! itself and the library dirs.
//!
//! The program writes the ready byte `x` on `--ready-fd` (3, the sync pipe that the
//! wrapper moved there), then one JSON line per check on the same descriptor, because
//! its stdout is the wrapper's stderr. The exit status is 0 when every check passed.

use std::fs;
use std::io::{self, Write};
use std::net::TcpStream;
use std::os::fd::AsFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::process::ExitCode;
use std::time::Duration;

use rustix::io::Errno;

use super::{CheckLine, SelfTestArgs, outcome, socket_path};
use crate::fds;

/// The checks of the bus proxy's domain, in the order they run.
pub(crate) const BUS_PROXY_CHECKS: &[&str] = &[
    "stdout_is_stderr",
    "bind_in_dir",
    "upstream",
    "other_socket",
    "write_outside",
    "read_outside",
    "tcp",
    "fds",
];

/// The byte that the program writes on the ready descriptor before its lines.
pub(crate) const READY: u8 = b'x';

/// Runs the checks and writes them after the ready byte.
pub(crate) fn main(args: &SelfTestArgs) -> ExitCode {
    let Some(fd) = args.ready_fd else { return ExitCode::FAILURE };
    let Ok(ready) = fds::adopt(fd) else { return ExitCode::FAILURE };
    let mut out = fs::File::from(ready);
    if out.write_all(&[READY]).is_err() {
        return ExitCode::FAILURE;
    }
    let mut all = true;
    for name in BUS_PROXY_CHECKS {
        if !args.checks.is_empty() && !args.checks.iter().any(|check| check == name) {
            continue;
        }
        let Some((ok, detail)) = check(name, args) else { continue };
        all &= ok;
        let line = CheckLine { name: (*name).to_owned(), ok, detail };
        let text = serde_json::to_string(&line).unwrap_or_default();
        let _ = writeln!(out, "{text}");
    }
    if all { ExitCode::SUCCESS } else { ExitCode::FAILURE }
}

/// The errno of `error`.
fn errno(error: &io::Error) -> i32 {
    error.raw_os_error().unwrap_or_default()
}

/// A connect to `path` that the domain must refuse with `EACCES`.
fn refused_connect(path: &Path) -> (bool, String) {
    match socket_path(path) {
        Err(error) => (false, format!("no path to the socket: {error:?}")),
        Ok((dir, short)) => {
            let done = UnixStream::connect(&short);
            drop(dir);
            let refused =
                done.as_ref().is_err_and(|error| errno(error) == Errno::ACCESS.raw_os_error());
            (refused, format!("{}: {:?}", path.display(), done.map(drop)))
        }
    }
}

/// `Some((ok, detail))` for one check, or `None` when its argument is missing.
fn check(name: &str, args: &SelfTestArgs) -> Option<(bool, String)> {
    Some(match name {
        "stdout_is_stderr" => {
            let same = |a: &rustix::fs::Stat, b: &rustix::fs::Stat| {
                a.st_dev == b.st_dev && a.st_ino == b.st_ino
            };
            match (rustix::fs::fstat(io::stdout().as_fd()), rustix::fs::fstat(io::stderr().as_fd()))
            {
                (Ok(out), Ok(err)) => outcome(same(&out, &err), "fd 1 and fd 2".to_owned()),
                (out, err) => outcome(false, format!("{out:?} {err:?}")),
            }
        }
        "bind_in_dir" => {
            // A socket by a relative name in the working dir, as the proxy binds its own;
            // a connect to it and its removal work there.
            let name = args.bind.as_ref()?;
            let done = UnixListener::bind(name).and_then(|listener| {
                let client = UnixStream::connect(name)?;
                drop((client, listener));
                fs::remove_file(name)
            });
            outcome(done.is_ok(), format!("{done:?}"))
        }
        "upstream" => {
            if args.upstream.is_empty() {
                return None;
            }
            let results: Vec<String> = args
                .upstream
                .iter()
                .map(|path| match socket_path(path) {
                    Ok((dir, short)) => {
                        let done = UnixStream::connect(&short).map(drop);
                        drop(dir);
                        format!("{}: {done:?}", path.display())
                    }
                    Err(error) => format!("{}: {error:?}", path.display()),
                })
                .collect();
            outcome(results.iter().all(|line| line.ends_with("Ok(())")), results.join(", "))
        }
        "other_socket" => {
            if args.refuse.is_empty() {
                return None;
            }
            let results: Vec<(bool, String)> =
                args.refuse.iter().map(|path| refused_connect(path)).collect();
            let ok = results.iter().all(|(ok, _)| *ok);
            outcome(
                ok,
                results.into_iter().map(|(_, detail)| detail).collect::<Vec<_>>().join(", "),
            )
        }
        "write_outside" => {
            let path = args.write_dir.as_ref()?.join(".efr-bus-proxy");
            let done = fs::write(&path, b"x");
            let refused =
                done.as_ref().is_err_and(|error| errno(error) == Errno::ACCESS.raw_os_error());
            outcome(refused, format!("{done:?}"))
        }
        "read_outside" => {
            let done = fs::read(args.read.as_ref()?);
            let refused =
                done.as_ref().is_err_and(|error| errno(error) == Errno::ACCESS.raw_os_error());
            outcome(refused, format!("{:?}", done.map(|bytes| bytes.len())))
        }
        "tcp" => {
            // seccomp refuses the family before Landlock's TCP rule could.
            let done = TcpStream::connect_timeout(args.tcp.as_ref()?, Duration::from_secs(2));
            let refused =
                done.as_ref().is_err_and(|error| errno(error) == Errno::AFNOSUPPORT.raw_os_error());
            outcome(refused, format!("{:?}", done.map(drop)))
        }
        "fds" => {
            // The wrapper leaves fds 0 to 3 open and sets the limit to 64: the first new
            // descriptor is 4, and the last that opens is 63.
            let mut opened = Vec::new();
            let error = loop {
                match fs::File::open("/etc/ld.so.cache") {
                    Ok(file) => opened.push(file),
                    Err(error) => break error,
                }
                if opened.len() > 1024 {
                    break io::Error::other("no limit");
                }
            };
            let numbers: Vec<i32> = opened.iter().map(std::os::fd::AsRawFd::as_raw_fd).collect();
            let first = numbers.first().copied();
            let last = numbers.last().copied();
            let ok = first == Some(4)
                && last == Some(63)
                && errno(&error) == Errno::MFILE.raw_os_error();
            outcome(ok, format!("first {first:?} last {last:?} then {error:?}"))
        }
        _ => return None,
    })
}

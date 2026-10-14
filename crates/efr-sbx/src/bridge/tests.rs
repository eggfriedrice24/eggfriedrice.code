//! The parts of the bridge that need no sandbox: its arguments, the ready byte and its
//! limits. The real starts, the copies and the confinement are in `tests/it/proxy.rs`.

use std::io::Write;
use std::time::Duration;

use clap::Parser;

use super::{BRIDGE_CNAME, BRIDGE_NAME, BridgeArgs, MAX_CONNECTIONS, READY_WAIT, Step, read_ready};
use crate::{Cli, Command};

fn parse(args: &[&str]) -> Result<BridgeArgs, clap::Error> {
    let mut argv = vec!["efr-sbx", "bridge"];
    argv.extend_from_slice(args);
    match Cli::try_parse_from(argv)?.command {
        Command::Bridge(args) => Ok(args),
        other => panic!("not the bridge: {other:?}"),
    }
}

#[test]
fn bridge_arguments_of_the_starter_and_the_server() {
    let starter =
        parse(&["--listen-fd", "0", "--socket", "/run/user/1000/efr-proxy.sock"]).unwrap();
    assert_eq!(starter.listen_fd, Some(0));
    assert!(!starter.serve && !starter.check_dumpable);
    assert_eq!(starter.fail_setup, None);
    let server = parse(&[
        "--serve",
        "--listen-fd",
        "0",
        "--ready-fd",
        "1",
        "--socket",
        "/x/efr-proxy.sock",
        "--fail-setup",
        "landlock",
    ])
    .unwrap();
    assert!(server.serve);
    assert_eq!((server.listen_fd, server.ready_fd), (Some(0), Some(1)));
    assert_eq!(server.fail_setup, Some(Step::Landlock));
    // Every step has the word that the starter passes on.
    for step in
        [Step::Close, Step::Session, Step::Name, Step::Dumpable, Step::Landlock, Step::Seccomp]
    {
        let args = parse(&["--socket", "/s", "--fail-setup", step.word()]).unwrap();
        assert_eq!(args.fail_setup, Some(step));
    }
    assert!(parse(&["--socket", "/s", "--fail-setup", "network"]).is_err());
    // The checks of the bridge's own sandbox come only with --check-dumpable.
    assert!(parse(&["--socket", "/s", "--write", "/p"]).is_err());
    let check = parse(&["--check-dumpable", "--socket", "/s", "--write", "/p", "--reach", "/b"]);
    assert!(check.is_ok_and(|check| check.write.is_some() && check.reach.is_some()));
}

#[test]
fn ready_byte_or_its_absence() {
    let (read, write) = rustix::pipe::pipe().unwrap();
    std::fs::File::from(write).write_all(b"R").unwrap();
    assert_eq!(read_ready(read, Duration::from_secs(10)), Some(b'R'));
    // The server ended before its byte: the end of the pipe.
    let (read, write) = rustix::pipe::pipe().unwrap();
    drop(write);
    assert_eq!(read_ready(read, Duration::from_secs(10)), None);
    // The server holds the pipe open and sends nothing: the wait ends it.
    let (read, write) = rustix::pipe::pipe().unwrap();
    assert_eq!(read_ready(read, Duration::from_millis(20)), None);
    drop(write);
}

#[test]
fn bridge_limits() {
    assert_eq!(BRIDGE_CNAME.to_bytes(), BRIDGE_NAME.as_bytes());
    // The kernel keeps 15 bytes of a process name.
    assert!(BRIDGE_CNAME.to_bytes().len() <= 15);
    assert_eq!(MAX_CONNECTIONS, 64);
    assert!(READY_WAIT < crate::inner::BRIDGE_WAIT);
}

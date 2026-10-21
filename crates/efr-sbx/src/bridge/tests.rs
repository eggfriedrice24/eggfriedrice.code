//! The parts of the bridge that need no sandbox: its arguments and its limits. The real
//! starts, the copies and the confinement are in `tests/it/proxy.rs`.

use clap::Parser;

use super::{BRIDGE_NAME, BridgeArgs, MAX_CONNECTIONS};
use crate::helper::{Helper, READY_WAIT, Step};
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
    for step in Step::ALL {
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
fn bridge_limits() {
    assert_eq!(Helper::Bridge.name().to_bytes(), BRIDGE_NAME.as_bytes());
    assert_eq!(MAX_CONNECTIONS, 64);
    assert!(READY_WAIT < crate::inner::BRIDGE_WAIT);
}

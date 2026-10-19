//! The buses on the real launcher: a socket grant never reaches a bus, and (phase E) the
//! filtered bus sockets of a call. A `UnixListener` of the test stands for each bus; no
//! test reaches the machine's real system or session bus, and no test needs a D-Bus
//! tool.

use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};

use efr_protocol::Grant;

use crate::support::{Fixture, q, sandbox_or_skip};

/// A stand-in bus at `path`: a listener that the test checks for connections.
fn stand_in(path: &Path) -> UnixListener {
    let listener = UnixListener::bind(path).unwrap();
    listener.set_nonblocking(true).unwrap();
    listener
}

/// True when a connection reached `listener`.
fn reached(listener: &UnixListener) -> bool {
    listener.accept().is_ok()
}

/// A line that connects to `path` and prints `reached` when it can.
fn connect_line(path: &Path) -> String {
    format!("zmodload zsh/net/socket; zsocket {} && print reached", q(path))
}

#[test]
fn escape_socket_grant_link_to_bus_refused() {
    let ready = sandbox_or_skip!();
    let mut fixture = Fixture::new(&ready);
    // The session bus of the call's user, at `$XRT/bus`, and a link to it in the
    // project, which a contained call could have made.
    let bus: PathBuf = fixture.xrt.join("bus");
    let listener = stand_in(&bus);
    fixture.spec.runtime.session_bus = Some(bus.clone());
    let link = fixture.project.join("app.sock");
    std::os::unix::fs::symlink(&bus, &link).unwrap();
    fixture.grant(Grant::Socket { path: link.clone() });
    let run = fixture.run(&connect_line(&link));
    // The plan refuses the grant: a setup failure, and the line never runs.
    run.expect_status(efr_sandbox::SETUP_FAILURE_STATUS);
    let error = run.result().setup_error.clone().unwrap_or_default();
    assert!(error.contains("reaches a bus"), "{run:#?}");
    assert!(!run.stdout.contains("reached"), "{run:#?}");
    assert!(!reached(&listener), "the stand-in bus got a connection");

    // A link to the service manager's private socket is refused the same way.
    let private_dir = fixture.xrt.join("systemd");
    std::fs::create_dir(&private_dir).unwrap();
    let private = private_dir.join("private");
    let manager = stand_in(&private);
    let link = fixture.project.join("m.sock");
    std::os::unix::fs::symlink(&private, &link).unwrap();
    fixture.spec.grants = vec![Grant::Socket { path: link.clone() }];
    let run = fixture.run(&connect_line(&link));
    run.expect_status(efr_sandbox::SETUP_FAILURE_STATUS);
    assert!(!run.stdout.contains("reached"), "{run:#?}");
    assert!(!reached(&manager), "the stand-in manager got a connection");
}

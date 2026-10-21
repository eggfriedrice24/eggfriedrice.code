use std::io::Write;

use efr_sandbox::FsAccess;
use landlock::{ABI, AccessFs};

use super::*;

#[test]
fn handled_rights_are_every_right_of_abi_9() {
    assert_eq!(access(&FsAccess::HANDLED), Some(AccessFs::from_all(ABI::V9)));
}

#[test]
fn device_rights_are_file_rights_only() {
    let device = access(&FsAccess::DEVICE).unwrap();
    assert_eq!(device, AccessFs::ReadFile | AccessFs::WriteFile | AccessFs::IoctlDev);
    assert!(AccessFs::from_file(ABI::V9).contains(device));
}

#[test]
fn write_dir_rights_leave_out_devices_and_sockets() {
    let write = access(&FsAccess::WRITE_DIR).unwrap();
    for right in
        [AccessFs::MakeChar, AccessFs::MakeBlock, AccessFs::IoctlDev, AccessFs::ResolveUnix]
    {
        assert!(!write.contains(right), "{right:?}");
    }
}

/// A TCP connect to `address` from a new thread that first restricts itself with a
/// policy that only reads `/`, with `deny_tcp`. Landlock acts on the calling thread
/// alone, so the test's own threads keep their rights. No seccomp filter is in place:
/// the result is Landlock's alone.
fn connect_restricted(
    address: std::net::SocketAddr,
    deny_tcp: bool,
) -> Result<io::Result<std::net::TcpStream>, SbxError> {
    let policy: LandlockPolicy = serde_json::from_value(serde_json::json!({
        "min_abi": 9,
        "required_errata": 0,
        "read_exec": ["/"],
        "write": [],
        "write_files": [],
        "dev_rw": [],
        "tty_fds": [],
        "resolve_unix": [],
        "devices": [],
        "scopes": ["abstract_unix_socket", "signal"],
        "deny_tcp": deny_tcp,
    }))
    .unwrap();
    std::thread::spawn(move || {
        restrict(&policy, &[])?;
        Ok(std::net::TcpStream::connect(address))
    })
    .join()
    .unwrap()
}

#[test]
fn netview_tcp_landlock_layer() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let refused = match connect_restricted(address, true) {
        Ok(connect) => connect,
        Err(error) => {
            let required = crate::os::var("EFR_TEST_SBX_REQUIRE").is_some_and(|value| value == "1");
            assert!(!required, "Landlock ABI 9 is required here, but: {error:?}");
            let _ = writeln!(io::stdout(), "skipped: no Landlock ABI 9 here: {error:?}");
            return;
        }
    };
    let error = refused.map(drop).unwrap_err();
    assert_eq!(error.raw_os_error(), Some(rustix::io::Errno::ACCESS.raw_os_error()), "{error:?}");
    assert!(listener.accept().is_err(), "the refused connect reached the listener");
    // The control: the same rule set without deny_tcp connects, so the refusal above
    // was Landlock's TCP right and nothing else.
    let allowed = connect_restricted(address, false).unwrap();
    assert!(allowed.is_ok(), "{allowed:?}");
}

#[test]
fn forward_domain_refuses_bind_and_keeps_connect() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    // The forwarder's own domain, on a new thread: Landlock acts on the calling thread
    // alone. The socket path does not exist, so its rule is left out.
    let policy = LandlockPolicy::forward(Path::new("/nonexistent/efr-forward.sock"));
    assert!(policy.deny_tcp_bind && !policy.deny_tcp);
    let done = std::thread::spawn(move || {
        restrict(&policy, &[])?;
        let bind = std::net::TcpListener::bind("127.0.0.1:0").map(drop);
        let connect = std::net::TcpStream::connect(address).map(drop);
        let open = std::fs::File::open("/proc/self/status").map(drop);
        Ok::<_, SbxError>((bind, connect, open))
    })
    .join()
    .unwrap();
    let (bind, connect, open) = match done {
        Ok(done) => done,
        Err(error) => {
            let required = crate::os::var("EFR_TEST_SBX_REQUIRE").is_some_and(|value| value == "1");
            assert!(!required, "Landlock ABI 9 is required here, but: {error:?}");
            let _ = writeln!(io::stdout(), "skipped: no Landlock ABI 9 here: {error:?}");
            return;
        }
    };
    let access = Some(rustix::io::Errno::ACCESS.raw_os_error());
    assert_eq!(bind.as_ref().map_err(io::Error::raw_os_error), Err(access), "{bind:?}");
    assert!(connect.is_ok(), "the forwarder must reach the job's servers: {connect:?}");
    assert!(listener.accept().is_ok());
    // No file right: an open after the domain fails.
    assert_eq!(open.as_ref().map_err(io::Error::raw_os_error), Err(access), "{open:?}");
}

#[test]
fn a_missing_rule_path_is_left_out() {
    assert!(open_rule_path(Path::new("/nonexistent/efr-sbx-rule")).unwrap().is_none());
    assert!(open_rule_path(Path::new("/")).unwrap().is_some());
}

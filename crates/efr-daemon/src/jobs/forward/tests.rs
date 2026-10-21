//! efrd's side of job port forwards: which job gets forwards, the forward socket, the
//! table of held host ports, the lines, and the system's host ports.

use std::io::{Read as _, Write as _};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::os::fd::AsFd as _;
use std::os::unix::fs::{FileTypeExt as _, PermissionsExt as _};
use std::path::PathBuf;
use std::sync::Arc;

use efr_config::JobsSettings;
use efr_permissions::Egress;
use efr_protocol::{BusKind, ConversationId, ForwardOutcome, Grant, JobId, Launch, PortForward};
use efr_sandbox::{ForwardPlan, MAX_DAEMON_FORWARDS, peer_uid};
use pretty_assertions::assert_eq;

use super::{Forwards, SystemHostPorts, Taken, bind, line, plan_of, ports_text};
use crate::jobs::JobKey;
use crate::sandbox::seams::{HostPorts, TestHostPorts};

fn key(conversation: u128, number: u32) -> JobKey {
    JobKey { conversation: ConversationId::from_uuid(uuid::Uuid::from_u128(conversation)), number }
}

#[test]
fn a_host_port_belongs_to_one_job_and_efrd_holds_at_most_32() {
    let forwards = Forwards::new(Arc::new(TestHostPorts::new()));
    let (j1, j2) = (key(1, 1), key(1, 2));
    assert_eq!(forwards.reserve(j1, 8765), Ok(()));
    assert_eq!(forwards.reserve(j1, 8765), Ok(()), "the job holds it already");
    assert_eq!(forwards.reserve(j2, 8765), Err(Taken::By(j1)));
    forwards.release(j2, 8765);
    assert_eq!(forwards.reserve(j2, 8765), Err(Taken::By(j1)), "only its holder frees it");
    forwards.release(j1, 8765);
    assert_eq!(forwards.reserve(j2, 8765), Ok(()));
    forwards.release_job(j2);
    let ports: Vec<u16> = (0..MAX_DAEMON_FORWARDS).map(|n| 20_000 + n as u16).collect();
    for (n, port) in ports.iter().enumerate() {
        let holder = key(2, u32::try_from(n / 8).unwrap() + 1);
        assert_eq!(forwards.reserve(holder, *port), Ok(()));
    }
    assert_eq!(forwards.reserve(key(3, 1), 30_000), Err(Taken::Full), "the 33rd port");
    forwards.release_job(key(2, 1));
    assert_eq!(forwards.reserve(key(3, 1), 30_000), Ok(()), "a job's end frees its ports");
}

#[test]
fn a_turn_off_counts_once_for_each_reload() {
    let forwards = Forwards::new(Arc::new(TestHostPorts::new()));
    let before = forwards.generation();
    forwards.turn_off();
    assert_eq!(forwards.generation(), before + 1);
}

#[test]
fn each_outcome_has_its_line() {
    let (j1, j2) = (JobId::nth(1), JobId::nth(2));
    let cases = [
        (
            line(&j1, "127.0.0.1", 8765, ForwardOutcome::Forwarded, None, None),
            "job j1 listens on 127.0.0.1:8765 -> http://localhost:8765",
        ),
        (
            line(&j1, "::1", 5173, ForwardOutcome::Forwarded, None, None),
            "job j1 listens on [::1]:5173 -> http://localhost:5173",
        ),
        (
            line(&j1, "127.0.0.1", 8765, ForwardOutcome::InUse, None, None),
            "job j1 listens on 127.0.0.1:8765; not forwarded: 127.0.0.1:8765 is in use on this \
             machine",
        ),
        (
            line(&j1, "127.0.0.1", 8765, ForwardOutcome::InUse, Some(&j2), None),
            "job j1 listens on 127.0.0.1:8765; not forwarded: 127.0.0.1:8765 is in use by job j2",
        ),
        (
            line(&j1, "127.0.0.1", 6379, ForwardOutcome::Refused, None, Some("a service port")),
            "job j1 listens on 127.0.0.1:6379; not forwarded: efr does not forward port 6379 (a \
             service port)",
        ),
        (
            line(
                &j1,
                "127.0.0.1",
                9000,
                ForwardOutcome::Limit,
                None,
                Some("the job forwards 8 ports already"),
            ),
            "job j1 listens on 127.0.0.1:9000; not forwarded: the job forwards 8 ports already",
        ),
        (
            line(
                &j1,
                "127.0.0.1",
                8765,
                ForwardOutcome::Stopped,
                None,
                Some("jobs.forward_ports is off"),
            ),
            "job j1: port 8765 is no longer forwarded (jobs.forward_ports is off)",
        ),
        (
            line(&j1, "127.0.0.1", 8765, ForwardOutcome::Unknown, None, Some("what")),
            "job j1 listens on 127.0.0.1:8765",
        ),
    ];
    for (got, want) in cases {
        assert_eq!(got, want);
    }
    let forwards = [
        PortForward { listen: "127.0.0.1".to_owned(), port: 8765 },
        PortForward { listen: "::1".to_owned(), port: 5173 },
    ];
    assert_eq!(ports_text(&forwards).as_deref(), Some("forwarded ports 8765, 5173"));
    assert_eq!(ports_text(&[]), None);
}

/// Check first (plan 5.1): a listener that std binds and tokio takes over has no
/// `SO_REUSEPORT`, so no other process can share the port.
#[tokio::test]
async fn system_bind_has_no_reuseport() {
    // NOTE: port 0 binds a free port; the real bind of a fixed port is the one test of
    // tests/it/jobs.rs.
    let listener = SystemHostPorts.bind(0).unwrap();
    let reuse =
        nix::sys::socket::getsockopt(&listener.as_fd(), nix::sys::socket::sockopt::ReusePort)
            .unwrap();
    assert!(!reuse);
    let port = listener.local_addr().unwrap().port();
    let tokio = tokio::net::TcpListener::from_std(listener).unwrap();
    let reuse = nix::sys::socket::getsockopt(&tokio, nix::sys::socket::sockopt::ReusePort).unwrap();
    assert!(!reuse, "tokio keeps the options");
    let second = TcpListener::bind((Ipv4Addr::LOCALHOST, port));
    assert!(second.is_err(), "no second listener on the port");
}

/// Check first (plan 5.1): the row of the peer of a connection that efrd accepted is in
/// the host's tcp table at once, with the peer's uid. (`efr-sandbox` pins that the
/// lookup never takes efrd's own row of the connection.)
#[test]
fn peer_row_is_in_the_table_at_once() {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let mut client = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
    let (mut accepted, peer) = listener.accept().unwrap();
    let (mut rows, v6) = SystemHostPorts.rows().unwrap();
    rows.extend(v6);
    assert_eq!(peer_uid(&rows, port, peer.port()), Some(SystemHostPorts.own_uid()));
    client.write_all(b"x").unwrap();
    let mut byte = [0u8; 1];
    accepted.read_exact(&mut byte).unwrap();
    assert_eq!(&byte, b"x");
}

#[test]
fn test_host_ports_bind_a_free_port_for_each_asked_port() {
    let ports = TestHostPorts::new();
    let listener = ports.bind(8765).unwrap();
    let real = listener.local_addr().unwrap().port();
    assert_eq!(ports.real_port(8765), Some(real));
    assert_ne!(real, 8765);
    ports.set_in_use(8766);
    assert_eq!(ports.bind(8766).unwrap_err().kind(), std::io::ErrorKind::AddrInUse);
    ports.set_v6_listen(8767);
    let (_, v6) = ports.rows().unwrap();
    assert!(efr_sandbox::v6_holds(&v6, 8767));
    ports.set_peer_uid(4242);
    let (v4, _) = ports.rows().unwrap();
    assert!(v4.iter().all(|row| row.uid == 4242));
}

fn socket() -> PathBuf {
    PathBuf::from("/run/user/1000/efr/sbx/c/jobs/k/forward.sock")
}

#[test]
fn contained_job_with_its_own_network_gets_forwards() {
    let on = JobsSettings::default();
    assert!(on.forward_ports, "the default forwards");
    let routine = Launch::contained();
    assert_eq!(
        plan_of(&on, &routine, Egress::None, socket()),
        Some(ForwardPlan::new(socket(), false)),
        "network.mode = none"
    );
    assert_eq!(
        plan_of(&on, &routine, Egress::Proxy, socket()),
        Some(ForwardPlan::new(socket(), true)),
        "the proxy: port 3128 is the bridge's"
    );
    let granted = Launch::Contained {
        grants: vec![
            Grant::Host { host: "gitlab.com".to_owned(), port: 443 },
            Grant::BusFilter { bus: BusKind::Session },
        ],
    };
    assert_eq!(
        plan_of(&on, &granted, Egress::Proxy, socket()),
        Some(ForwardPlan::new(socket(), true)),
        "grants that keep the job's own namespace"
    );
}

#[test]
fn no_forwards_off_in_the_exit_child_or_on_the_host_network() {
    let on = JobsSettings::default();
    let mut off = JobsSettings::default();
    off.forward_ports = false;
    assert_eq!(plan_of(&off, &Launch::contained(), Egress::Proxy, socket()), None, "off");
    for launch in [Launch::Unsandboxed, Launch::Direct] {
        assert_eq!(plan_of(&on, &launch, Egress::Proxy, socket()), None, "{launch:?}");
    }
    for grant in [Grant::OpenNetwork, Grant::NetView] {
        let launch = Launch::Contained { grants: vec![grant.clone()] };
        assert_eq!(plan_of(&on, &launch, Egress::Proxy, socket()), None, "{grant:?}");
        assert_eq!(plan_of(&on, &launch, Egress::None, socket()), None, "{grant:?}");
    }
}

#[tokio::test]
async fn forward_socket_binds_0600_in_a_deep_call_dir() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("r".repeat(60)).join("sbx").join("c".repeat(36)).join("jobs");
    let dir = dir.join("k".repeat(36));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("forward.sock");
    assert!(path.as_os_str().len() > 107, "{}", path.display());
    let socket = bind(&dir, ForwardPlan::new(path.clone(), false), 0).await.unwrap();
    let meta = std::fs::symlink_metadata(&path).unwrap();
    assert!(meta.file_type().is_socket());
    assert_eq!(meta.permissions().mode() & 0o777, 0o600);
    assert!(bind(&dir, ForwardPlan::new(path, false), 0).await.is_err(), "one socket per job");
    drop(socket);
}

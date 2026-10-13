//! The network view's escape tests (efr's auto spec, section 8.8): a call with
//! `Grant::NetView` keeps the host's network namespace, so `ss`, `ip` and `/proc/net`
//! show the host, and every way to send data out must fail with the errno of its layer.
//!
//! A refusal passes only on its exact errno (`EAFNOSUPPORT` from seccomp's families,
//! `EPROTONOSUPPORT` from its netlink protocols, `EPERM` from `listen`, `pidfd_getfd`
//! and the Landlock scope): in a VM a connect can fail with no seccomp at all. Every
//! target is the test's own: a listener on `127.0.0.1:0` or an abstract name with the
//! test's pid. The write checks use only forms that change nothing even if they work.
//! No test calls `socket()` for a refused family outside the network view's profile.

use std::fs;
use std::io::ErrorKind;
use std::net::{TcpListener, UdpSocket};
use std::os::linux::net::SocketAddrExt;
use std::os::unix::fs::MetadataExt;
use std::os::unix::net::{SocketAddr, UnixDatagram, UnixListener};

use crate::support::{Fixture, Ready, Run, command, need, q, require, sandbox_or_skip};

/// A fixture whose calls have the network view.
fn viewed(ready: &Ready) -> Fixture {
    let mut fixture = Fixture::new(ready);
    fixture.net_view();
    fixture
}

/// The line that runs the network view's self-test `check` inside, with `args`.
fn self_test(fixture: &Fixture, check: &str, args: &str) -> String {
    let inside = fixture.spec.runtime.inside_launcher();
    format!("{} self-test --net-view --check {check} {args}", q(&inside))
}

/// The `ok` and the `detail` of the self-test's line of `check` in `run`.
fn check_line(run: &Run, check: &str) -> (bool, String) {
    let line = run
        .stdout
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .find(|line| line["name"] == check)
        .unwrap_or_else(|| panic!("no {check} line: {run:#?}"));
    let ok = line["ok"].as_bool().unwrap_or_default();
    (ok, line["detail"].as_str().unwrap_or_default().to_owned())
}

/// Runs `check` in the network view and asserts that it passed with `detail`.
fn passes(fixture: &Fixture, check: &str, args: &str, detail: &str) -> Run {
    let run = fixture.run(&self_test(fixture, check, args));
    let (ok, got) = check_line(&run, check);
    assert!(ok, "{check}: {got}: {run:#?}");
    assert!(got.contains(detail), "{check}: {got:?} lacks {detail:?}");
    // The gate ran first and passed.
    assert_eq!(check_line(&run, "tcp4"), (true, "errno 97".to_owned()), "{run:#?}");
    run
}

/// A listener of the test on a free loopback port, which a test checks for a
/// connection that must not come.
fn loopback() -> (TcpListener, u16) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let port = listener.local_addr().unwrap().port();
    (listener, port)
}

/// True when `text` holds the errno message of `EAFNOSUPPORT`, in any case.
fn family_refused(text: &str) -> bool {
    text.to_ascii_lowercase().contains("address family not supported")
}

#[test]
fn netview_tcp_socket_refused() {
    let ready = sandbox_or_skip!();
    let fixture = viewed(&ready);
    passes(&fixture, "tcp6", "", "errno 97");
    let (listener, port) = loopback();
    let run = fixture.run(&format!(
        "zmodload zsh/net/tcp; ztcp 127.0.0.1 {port} && print connected; ztcp 192.0.2.1 9 && \
         print connected"
    ));
    assert!(!run.stdout.contains("connected"), "{run:#?}");
    // NOTE: ztcp prints a wrong errno for a failed socket(); the self-test above and
    // the loopback test hold the exact one. The place of the failure is what counts.
    assert_eq!(run.stderr.matches("socket creation failed").count(), 2, "{run:#?}");
    assert!(listener.accept().is_err(), "a TCP connect left the call");
    // The control: phase 1 gets a TCP socket, so the refusal is the view's profile.
    let contained = Fixture::new(&ready);
    let run = contained.run(&self_test(&contained, "tcp4", ""));
    assert_eq!(check_line(&run, "tcp4"), (false, "opened".to_owned()), "{run:#?}");
}

#[test]
fn netview_udp_socket_refused() {
    let ready = sandbox_or_skip!();
    need!("bash", "getent");
    let fixture = viewed(&ready);
    passes(&fixture, "udp4", "", "errno 97");
    passes(&fixture, "udp6", "", "errno 97");
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    socket.set_nonblocking(true).unwrap();
    let port = socket.local_addr().unwrap().port();
    let run = fixture.run(&format!(
        "bash -c 'echo leak > /dev/udp/127.0.0.1/{port}; echo leak > /dev/udp/192.0.2.1/9'"
    ));
    assert!(family_refused(&run.stderr), "{run:#?}");
    let mut buffer = [0_u8; 64];
    assert!(socket.recv(&mut buffer).is_err(), "a datagram left the call");
    // DNS: no AF_INET socket, so no query.
    let run = fixture.run("getent hosts example.com");
    assert_ne!(run.result().exit_code, Some(0), "{run:#?}");
    assert!(run.stdout.is_empty(), "{run:#?}");
}

#[test]
fn netview_packet_socket_refused() {
    let ready = sandbox_or_skip!();
    let fixture = viewed(&ready);
    // EAFNOSUPPORT from seccomp, not the kernel's EPERM for a missing capability.
    passes(&fixture, "packet", "", "errno 97");
}

#[test]
fn netview_abstract_socket_refused() {
    let ready = sandbox_or_skip!();
    let fixture = viewed(&ready);
    let name = format!("efr-sbx-netview-{}", std::process::id());
    let listener =
        UnixListener::bind_addr(&SocketAddr::from_abstract_name(&name).unwrap()).unwrap();
    listener.set_nonblocking(true).unwrap();
    // EPERM from the Landlock scope: the name exists in the shared namespace, so
    // ECONNREFUSED would mean the call did not share it.
    passes(&fixture, "abstract_stream", &format!("--abstract-name {name}"), "code: 1,");
    assert!(listener.accept().is_err(), "a connect reached the host's listener");
}

#[test]
fn netview_abstract_dgram_send_refused() {
    let ready = sandbox_or_skip!();
    let fixture = viewed(&ready);
    let name = format!("efr-sbx-netview-d-{}", std::process::id());
    let socket = UnixDatagram::bind_addr(&SocketAddr::from_abstract_name(&name).unwrap()).unwrap();
    socket.set_nonblocking(true).unwrap();
    passes(&fixture, "abstract_dgram", &format!("--abstract-dgram-name {name}"), "code: 1,");
    let mut buffer = [0_u8; 64];
    let received = socket.recv(&mut buffer);
    assert_eq!(received.map_err(|error| error.kind()), Err(ErrorKind::WouldBlock));
}

#[test]
fn netview_host_loopback_refused() {
    let ready = sandbox_or_skip!();
    let fixture = viewed(&ready);
    let (listener, port) = loopback();
    let mut targets = vec![format!("('127.0.0.1', {port})")];
    // The IPv6 half only where the host has ::1.
    let listener6 = TcpListener::bind("[::1]:0").ok();
    if let Some(listener6) = &listener6 {
        listener6.set_nonblocking(true).unwrap();
        targets.push(format!("('::1', {})", listener6.local_addr().unwrap().port()));
    }
    let run =
        fixture.run(&format!("zmodload zsh/net/tcp; ztcp 127.0.0.1 {port} && print connected"));
    assert!(!run.stdout.contains("connected"), "{run:#?}");
    assert!(run.stderr.contains("socket creation failed"), "{run:#?}");
    // The exact errno of each connect, where the machine has python3.
    if crate::support::have("python3") {
        let script = fixture.project.join("connect.py");
        let text = format!(
            "import socket\nfor target in [{}]:\n    try:\n        \
             socket.create_connection(target, 2)\n        print('connected')\n    \
             except OSError as error:\n        print('errno', error.errno)\n",
            targets.join(", ")
        );
        fs::write(&script, text).unwrap();
        let run = fixture.run(&format!("python3 {}", q(&script)));
        run.expect_status(0);
        assert_eq!(run.stdout, "errno 97\n".repeat(targets.len()), "{run:#?}");
    }
    assert!(listener.accept().is_err());
    assert!(listener6.is_none_or(|listener6| listener6.accept().is_err()));
}

#[test]
fn netview_ss_sees_host() {
    let ready = sandbox_or_skip!();
    require!("ss");
    let (_listener, port) = loopback();
    let address = format!("127.0.0.1:{port} ");
    let run = viewed(&ready).run("ss -Htln");
    run.expect_status(0);
    assert!(run.stdout.contains(&address), "{run:#?}");
    // The control: the private namespace of phase 1 has no such listener.
    let run = Fixture::new(&ready).run("ss -Htln");
    run.expect_status(0);
    assert!(!run.stdout.contains(&address), "{run:#?}");
}

#[test]
fn netview_proc_net_dev_sees_host() {
    let ready = sandbox_or_skip!();
    let (_listener, port) = loopback();
    let host = fs::metadata("/proc/self/ns/net").unwrap().ino();
    let line = "stat -L -c %i /proc/self/ns/net; cat /proc/net/tcp; cat /proc/net/dev";
    // /proc/net/tcp names a local address as hex, its port in four upper digits.
    let entry = format!("0100007F:{port:04X} ");
    let run = viewed(&ready).run(line);
    run.expect_status(0);
    assert_eq!(run.stdout.lines().next(), Some(host.to_string().as_str()), "{run:#?}");
    assert!(run.stdout.contains(&entry), "{run:#?}");
    assert!(run.stdout.contains("Receive"), "no /proc/net/dev: {run:#?}");
    let run = Fixture::new(&ready).run(line);
    run.expect_status(0);
    assert_ne!(run.stdout.lines().next(), Some(host.to_string().as_str()), "{run:#?}");
    assert!(!run.stdout.contains(&entry), "{run:#?}");
}

/// The host's own view of `lo`: its links, its addresses and its MTU.
fn host_lo() -> String {
    let mut view = String::new();
    for args in [["-o", "link", "show", "dev", "lo"], ["-o", "addr", "show", "dev", "lo"]] {
        let output = command("ip").args(args).output().unwrap();
        assert!(output.status.success(), "ip {args:?}: {output:?}");
        view.push_str(&String::from_utf8_lossy(&output.stdout));
    }
    view.push_str(&fs::read_to_string("/sys/class/net/lo/mtu").unwrap());
    view
}

#[test]
fn netview_ip_link_set_refused() {
    let ready = sandbox_or_skip!();
    require!("ip");
    let before = host_lo();
    // Only writes that change nothing even if they work: the MTU that lo has, and an
    // address that lo has (a success would give EEXIST). rtnetlink checks the
    // capability for every change before it acts.
    let run = viewed(&ready).run(
        "ip link set lo mtu \"$(cat /sys/class/net/lo/mtu)\"; print link=$?; \
         ip addr add 127.0.0.1/8 dev lo; print addr=$?",
    );
    assert_eq!(run.stderr.matches("Operation not permitted").count(), 2, "{run:#?}");
    assert!(!run.stdout.contains("link=0") && !run.stdout.contains("addr=0"), "{run:#?}");
    assert_eq!(host_lo(), before);
}

#[test]
fn netview_other_families_refused() {
    let ready = sandbox_or_skip!();
    let fixture = viewed(&ready);
    let run = passes(&fixture, "families", "", "errno 97");
    let (_, detail) = check_line(&run, "families");
    for family in [31, 38, 40, 44, 45] {
        assert!(detail.contains(&format!("{family}: errno 97")), "{detail}");
    }
}

#[test]
fn netview_netlink_protocols_limited() {
    let ready = sandbox_or_skip!();
    require!("ss");
    require!("ip");
    let fixture = viewed(&ready);
    passes(&fixture, "netlink_route", "", "opens");
    passes(&fixture, "netlink_sock_diag", "", "opens");
    let run = passes(&fixture, "netlink_other", "", "errno 93");
    let (_, detail) = check_line(&run, "netlink_other");
    for protocol in [16, 15, 9] {
        assert!(detail.contains(&format!("{protocol}: errno 93")), "{detail}");
    }
    // The read forms of ss and ip need no other protocol.
    let forms = [
        "ss -tunap",
        "ss -ti",
        "ss -x",
        "ip -s link",
        "ip addr",
        "ip route",
        "ip neigh",
        "ip -br a",
    ];
    for form in forms {
        let run = fixture.run(form);
        assert_eq!(run.result().exit_code, Some(0), "{form}: {run:#?}");
        assert!(!run.stderr.contains("Cannot open netlink"), "{form}: {run:#?}");
    }
}

#[test]
fn netview_listen_refused() {
    let ready = sandbox_or_skip!();
    let fixture = viewed(&ready);
    passes(&fixture, "listen", "", "Err(1)");
    let line = |fixture: &Fixture| {
        format!(
            "zmodload zsh/net/socket && zsocket -l {} && print listening",
            q(&fixture.project.join("s"))
        )
    };
    let run = fixture.run(&line(&fixture));
    assert!(!run.stdout.contains("listening"), "{run:#?}");
    // The control: phase 1 listens on a Unix socket in the project.
    let contained = Fixture::new(&ready);
    let run = contained.run(&line(&contained));
    assert!(run.stdout.contains("listening"), "{run:#?}");
}

#[test]
fn netview_pidfd_getfd_refused() {
    let ready = sandbox_or_skip!();
    passes(&viewed(&ready), "pidfd_getfd", "", "errno 1");
}

#[test]
fn netview_io_uring_still_eperm() {
    let ready = sandbox_or_skip!();
    let fixture = viewed(&ready);
    let inside = fixture.spec.runtime.inside_launcher();
    let run = fixture.run(&format!("{} self-test --check io_uring", q(&inside)));
    run.expect_status(0);
    assert!(run.stdout.contains("errno 1"), "{run:#?}");
}

#[test]
fn netview_ss_shows_no_outside_process_names() {
    let ready = sandbox_or_skip!();
    require!("ss");
    let (_listener, port) = loopback();
    let address = format!("127.0.0.1:{port} ");
    let ours = |text: &str| text.lines().find(|line| line.contains(&address)).map(str::to_owned);
    // The control: the host names the test's own process.
    let host = command("ss").arg("-Htlnp").output().unwrap();
    let host = ours(&String::from_utf8_lossy(&host.stdout)).unwrap();
    assert!(host.contains("users:"), "{host}");
    let run = viewed(&ready).run("ss -Htlnp");
    run.expect_status(0);
    let line = ours(&run.stdout).unwrap_or_else(|| panic!("no listener: {run:#?}"));
    assert!(!line.contains("users:"), "{line}");
}

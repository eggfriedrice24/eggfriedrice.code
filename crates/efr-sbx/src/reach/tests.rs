//! The parts of the reach helper that need no sandbox: its arguments, its refusals and
//! the route of one connection, on an ephemeral port of the host's loopback with a Unix
//! listener of the test that stands for efrd. The real starts, the listens and the
//! confinement are in `tests/it/reach.rs`.

use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, Shutdown, TcpListener, TcpStream};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};

use clap::Parser;
use efr_sandbox::{FRAME_LEN, Frame, FrameKind, InnerReach, Listen};

use super::{ReachArgs, argv, check_ports, refused, refused_port, route};
use crate::helper::Step;
use crate::testing::temp_dir;
use crate::{Cli, Command};

fn parse(args: &[&str]) -> Result<ReachArgs, clap::Error> {
    let mut argv = vec!["efr-sbx", "reach"];
    argv.extend_from_slice(args);
    match Cli::try_parse_from(argv)?.command {
        Command::Reach(args) => Ok(args),
        other => panic!("not the reach helper: {other:?}"),
    }
}

#[test]
fn reach_arguments_carry_the_plan() {
    let inner = InnerReach {
        socket: PathBuf::from("/run/user/1000/efr-reach.sock"),
        ports: vec![8765, 5173],
        proxy: true,
    };
    let words: Vec<String> =
        argv(&inner).iter().map(|word| word.to_string_lossy().into_owned()).collect();
    assert_eq!(
        words,
        ["reach", "--socket", "/run/user/1000/efr-reach.sock", "--port", "8765", "--port", "5173"]
    );
    let rest: Vec<&str> = words.iter().skip(1).map(String::as_str).collect();
    let args = parse(&rest).unwrap();
    assert_eq!(
        (args.socket.as_path(), args.ports.as_slice()),
        (inner.socket.as_path(), &[8765, 5173][..])
    );
    assert!(!args.serve && !args.check);
    // A plan with no port yet: the ports come during the call.
    let none = InnerReach { ports: Vec::new(), ..inner };
    assert_eq!(argv(&none).len(), 3);
    // The server and the check.
    let server =
        parse(&["--socket", "/s", "--serve", "--ready-fd", "1", "--fail-setup", "landlock"])
            .unwrap();
    assert!(server.serve);
    assert_eq!((server.ready_fd, server.fail_setup), (Some(1), Some(Step::Landlock)));
    assert!(parse(&["--socket", "/s", "--write", "/p"]).is_err(), "--write needs --check");
    assert!(parse(&["--socket", "/s", "--check", "--write", "/p"]).is_ok());
    assert!(parse(&["--socket", "/s", "--port", "70000"]).is_err());
    assert!(parse(&["--port", "8765"]).is_err(), "the socket is required");
}

#[test]
fn start_ports_refuse_what_efrd_never_forwards() {
    assert!(check_ports(&[]).is_ok());
    assert!(check_ports(&[1024, 8765, 65535]).is_ok());
    for bad in [&[80][..], &[1023], &[3128], &[8765, 8765]] {
        assert!(check_ports(bad).is_err(), "{bad:?}");
    }
    let many: Vec<u16> = (2000..2033).collect();
    assert_eq!(many.len(), 33);
    assert!(check_ports(&many).is_err());
    assert!(check_ports(&many[..32]).is_ok());
}

#[test]
fn listens_refuse_other_addresses_and_ports() {
    let v4 = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let v6 = IpAddr::V6(Ipv6Addr::LOCALHOST);
    assert!(!refused(Listen { addr: v4, port: 8765 }));
    assert!(!refused(Listen { addr: v6, port: 1024 }));
    for (addr, port) in [
        (v4, 1023),
        (v6, 80),
        (v4, 3128),
        (v6, 3128),
        (IpAddr::V4(Ipv4Addr::new(127, 0, 0, 2)), 8765),
        (IpAddr::V4(Ipv4Addr::UNSPECIFIED), 8765),
        (IpAddr::V6(Ipv6Addr::UNSPECIFIED), 8765),
    ] {
        assert!(refused(Listen { addr, port }), "{addr} {port}");
    }
    assert!(refused_port(3128) && refused_port(0) && !refused_port(65535));
}

/// What the test's stand-in for efrd does with the connection of a `Connect`.
#[derive(Clone, Copy)]
enum Efrd {
    /// Sends every byte back, then ends its side.
    Echo,
    /// Closes the connection with no byte: no job's server answers.
    Close,
}

/// A Unix listener in a temp dir that stands for efrd at the reach socket: it reads
/// the `Connect` frame of one connection, sends it on `frames`, and answers `efrd`.
fn efrd(dir: &Path, efrd: Efrd) -> (PathBuf, std::sync::mpsc::Receiver<Frame>) {
    let socket = dir.join("reach.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let (send, frames) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut bytes = [0_u8; FRAME_LEN];
        stream.read_exact(&mut bytes).unwrap();
        send.send(Frame::decode(&bytes).unwrap()).unwrap();
        match efrd {
            Efrd::Echo => {
                let mut back = stream.try_clone().unwrap();
                std::io::copy(&mut stream, &mut back).unwrap();
                back.shutdown(Shutdown::Write).unwrap();
            }
            Efrd::Close => drop(stream),
        }
    });
    (socket, frames)
}

/// A client and the helper's end of its connection, on an ephemeral port of the host's
/// loopback.
fn pair() -> (TcpStream, TcpStream, Listen) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let client = TcpStream::connect(address).unwrap();
    let (served, _) = listener.accept().unwrap();
    (client, served, Listen { addr: address.ip(), port: address.port() })
}

#[test]
fn route_sends_connect_and_copies_both_ways() {
    let dir = temp_dir();
    let (socket, frames) = efrd(dir.path(), Efrd::Echo);
    let (mut client, served, listen) = pair();
    let routing = std::thread::spawn(move || route(served, listen, &socket));
    let sent: Vec<u8> = (0..64 * 1024).map(|n| u8::try_from(n % 251).unwrap()).collect();
    let mut writer = client.try_clone().unwrap();
    let bytes = sent.clone();
    let writing = std::thread::spawn(move || {
        writer.write_all(&bytes).unwrap();
        writer.shutdown(Shutdown::Write).unwrap();
    });
    let mut back = Vec::new();
    client.read_to_end(&mut back).unwrap();
    writing.join().unwrap();
    routing.join().unwrap();
    assert!(back == sent, "the echo differs: {} bytes of {}", back.len(), sent.len());
    let connect = frames.recv().unwrap();
    assert_eq!((connect.kind, connect.target()), (FrameKind::Connect, listen));
}

#[test]
fn route_resets_the_client_when_efrd_closes_with_no_byte() {
    let dir = temp_dir();
    let (socket, frames) = efrd(dir.path(), Efrd::Close);
    let (mut client, served, listen) = pair();
    let routing = std::thread::spawn(move || route(served, listen, &socket));
    routing.join().unwrap();
    assert_eq!(frames.recv().unwrap().kind, FrameKind::Connect);
    let mut back = Vec::new();
    let read = client.read_to_end(&mut back);
    let error = read.unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::ConnectionReset, "{error:?}");
}

#[test]
fn route_resets_the_client_when_the_socket_does_not_answer() {
    let dir = temp_dir();
    let socket = dir.path().join("reach.sock");
    drop(UnixListener::bind(&socket).unwrap());
    let (mut client, served, listen) = pair();
    route(served, listen, &socket);
    let mut byte = [0_u8; 1];
    let error = client.read(&mut byte).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::ConnectionReset, "{error:?}");
    let _ = UnixStream::connect(&socket).unwrap_err();
}

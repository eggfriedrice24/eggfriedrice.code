//! The parts of the forwarder that need no sandbox: its arguments, the ephemeral range,
//! the status of an `Open`, and the fresh reads of the tables through descriptors that
//! it opened before its own domain. The real starts, the frames and the confinement are
//! in `tests/it/forward.rs`.

use std::io::{self, Write};
use std::net::{Ipv4Addr, TcpListener};
use std::path::{Path, PathBuf};

use clap::Parser;
use efr_sandbox::{ForwardPlan, InnerForward, LandlockPolicy};

use super::{
    DEFAULT_EPHEMERAL, ForwardArgs, STATUS_BUSY, STATUS_FAILED, STATUS_REFUSED, TCP, argv,
    open_optional, parse_range, read_table, rows, status_before_connect, status_of,
};
use crate::helper::Step;
use crate::{Cli, Command};

fn parse(args: &[&str]) -> Result<ForwardArgs, clap::Error> {
    let mut argv = vec!["efr-sbx", "forward"];
    argv.extend_from_slice(args);
    match Cli::try_parse_from(argv)?.command {
        Command::Forward(args) => Ok(args),
        other => panic!("not the forwarder: {other:?}"),
    }
}

#[test]
fn forward_arguments_carry_the_plan() {
    // The defaults are the plan's defaults.
    let args = parse(&["--socket", "/x/efr-forward.sock"]).unwrap();
    assert_eq!(args.plan(), ForwardPlan::new(PathBuf::from("/x/efr-forward.sock"), false));
    assert!(!args.serve && !args.check);
    // The inner stage's arguments give back the plan of the inner policy.
    let plan = ForwardPlan {
        socket: PathBuf::from("/run/user/1000/efr-forward.sock"),
        poll_ms: 20,
        stable_ms: 40,
        ephemeral_stable_ms: 200,
        max_ports: 3,
        proxy: true,
    };
    let inner = InnerForward::new(&plan, plan.socket.clone());
    let words: Vec<String> =
        argv(&inner).iter().map(|word| word.to_string_lossy().into_owned()).collect();
    assert_eq!(words.first().map(String::as_str), Some("forward"));
    let rest: Vec<&str> = words.iter().skip(1).map(String::as_str).collect();
    let args = parse(&rest).unwrap();
    assert_eq!(args.plan(), plan);
    // The server and the check.
    let server =
        parse(&["--socket", "/s", "--serve", "--ready-fd", "1", "--fail-setup", "seccomp"])
            .unwrap();
    assert!(server.serve);
    assert_eq!((server.ready_fd, server.fail_setup), (Some(1), Some(Step::Seccomp)));
    assert!(parse(&["--socket", "/s", "--write", "/p"]).is_err(), "--write needs --check");
    assert!(parse(&["--socket", "/s", "--check", "--write", "/p"]).is_ok());
    assert!(parse(&["--socket", "/s", "--max-ports", "300"]).is_err());
}

#[test]
fn ephemeral_range_of_the_kernel_file() {
    assert_eq!(parse_range("32768\t60999\n"), Some(32768..=60999));
    assert_eq!(parse_range("1024 1024"), Some(1024..=1024));
    for bad in ["", "32768", "60999 32768", "a b", "1 2 3", "-1 5", "70000 70001"] {
        assert_eq!(parse_range(bad), None, "{bad:?}");
    }
    assert_eq!(DEFAULT_EPHEMERAL, 32768..=60999);
}

#[test]
fn open_status_refuses_low_and_bridge_ports_and_a_full_table() {
    // (port, proxy, a free place) -> the status before any connect.
    let cases = [
        (8765, false, true, None),
        (8765, true, true, None),
        (1023, false, true, Some(STATUS_REFUSED)),
        (80, true, true, Some(STATUS_REFUSED)),
        (3128, true, true, Some(STATUS_REFUSED)),
        // Without a proxy plan 3128 is a service port: efrd never asks, and the
        // forwarder leaves that rule to efrd.
        (3128, false, true, None),
        (6379, false, true, None),
        (8765, false, false, Some(STATUS_BUSY)),
        (3128, true, false, Some(STATUS_REFUSED)),
    ];
    for (port, proxy, place, want) in cases {
        assert_eq!(status_before_connect(port, proxy, place), want, "{port} {proxy} {place}");
    }
    assert_eq!(status_of(&io::Error::from_raw_os_error(111)), 111);
    assert_eq!(status_of(&io::Error::from(io::ErrorKind::TimedOut)), STATUS_FAILED);
    assert_eq!(status_of(&io::Error::from_raw_os_error(300)), STATUS_FAILED);
}

#[test]
fn missing_tcp6_is_no_table() {
    // A kernel with IPv6 off has no /proc/net/tcp6: the forwarder reads no v6 rows.
    assert!(open_optional(Path::new("/proc/self/net/efr-no-such-table")).unwrap().is_none());
    assert!(open_optional(Path::new(TCP)).unwrap().is_some());
}

#[test]
fn read_table_reads_a_long_file_from_the_start_each_time() {
    let dir = crate::testing::temp_dir();
    let path = dir.path().join("table");
    let text: String = (0..20_000).map(|n| format!("{n:08}\n")).collect();
    std::fs::write(&path, &text).unwrap();
    let file = std::fs::File::open(&path).unwrap();
    let mut read = Vec::new();
    read_table(&file, &mut read).unwrap();
    assert_eq!(read, text.as_bytes());
    read_table(&file, &mut read).unwrap();
    assert_eq!(read, text.as_bytes(), "a second read starts at offset 0 again");
}

/// The ports of the LISTEN rows of `/proc/self/net/tcp`, read through `file`.
fn listening(file: &std::fs::File) -> Vec<u16> {
    let mut text = Vec::new();
    rows(file, &mut text, false)
        .unwrap_or_default()
        .iter()
        .filter(|row| row.state == efr_sandbox::TCP_LISTEN)
        .map(|row| row.local.port())
        .collect()
}

/// The check first item of the plan: a descriptor of `/proc/self/net/tcp` that the
/// forwarder opened before it became not dumpable and before its Landlock domain
/// reads fresh rows at each read from offset 0, and the domain refuses a new open.
#[test]
fn proc_tables_read_fresh_rows_after_the_domain() {
    let tcp = std::fs::File::open(TCP).unwrap();
    rustix::process::set_dumpable_behavior(rustix::process::DumpableBehavior::NotDumpable).unwrap();
    let (to_main, from_thread) = std::sync::mpsc::channel::<Vec<u16>>();
    let (to_thread, from_main) = std::sync::mpsc::channel::<u16>();
    // The domain acts on the thread that applies it; the test's main thread keeps its
    // rights and binds the listener, which the forwarder's domain would refuse.
    let restricted = std::thread::spawn(move || {
        let policy = LandlockPolicy::forward(Path::new("/nonexistent/efr-forward.sock"));
        crate::landlock::restrict(&policy, &[])?;
        let reopen = std::fs::File::open(TCP).map(drop);
        let _ = to_main.send(listening(&tcp));
        let port = from_main.recv().unwrap_or_default();
        let after = listening(&tcp);
        Ok::<_, crate::error::SbxError>((reopen, port, after))
    });
    let before = from_thread.recv();
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let _ = to_thread.send(port);
    let (reopen, port, after) = match restricted.join().unwrap() {
        Ok(done) => done,
        Err(error) => {
            let required = crate::os::var("EFR_TEST_SBX_REQUIRE").is_some_and(|value| value == "1");
            assert!(!required, "Landlock ABI 9 is required here, but: {error:?}");
            let _ = writeln!(io::stdout(), "skipped: no Landlock ABI 9 here: {error:?}");
            return;
        }
    };
    let before = before.unwrap();
    assert!(!before.contains(&port), "the port listened before its bind: {before:?}");
    assert!(after.contains(&port), "the read after the bind missed port {port}: {after:?}");
    let access = Some(rustix::io::Errno::ACCESS.raw_os_error());
    assert_eq!(reopen.as_ref().map_err(io::Error::raw_os_error), Err(access), "{reopen:?}");
}

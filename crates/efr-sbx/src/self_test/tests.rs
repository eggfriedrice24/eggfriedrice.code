use super::*;

#[test]
fn a_check_without_its_argument_is_skipped() {
    let args = SelfTestArgs::default();
    for name in ["write_inside", "write_outside", "mask_empty", "unix_socket", "tcp", "cache_write"]
    {
        assert_eq!(run(name, &args), None, "{name}");
    }
}

#[test]
fn an_unknown_check_is_skipped() {
    assert_eq!(run("no_such_check", &SelfTestArgs::default()), None);
}

#[test]
fn write_inside_passes_in_a_writable_dir() {
    let dir = tempfile::tempdir().unwrap();
    let args = SelfTestArgs { project: Some(dir.path().to_path_buf()), ..SelfTestArgs::default() };
    let (ok, _) = run("write_inside", &args).unwrap();
    assert!(ok);
}

#[test]
fn write_outside_fails_where_the_write_works() {
    // Outside a sandbox the write works, so the check must report a leak.
    let dir = tempfile::tempdir().unwrap();
    let args = SelfTestArgs { outside: Some(dir.path().join("file")), ..SelfTestArgs::default() };
    let (ok, _) = run("write_outside", &args).unwrap();
    assert!(!ok);
}

#[test]
fn self_test_stops_after_tcp4_mismatch() {
    // A fake runner: no socket() call happens here, outside any sandbox.
    let mut ran: Vec<String> = Vec::new();
    let lines = net_view(&[], &mut |name| {
        ran.push(name.to_owned());
        Some((name != "tcp4", format!("{name} ran")))
    });
    assert_eq!(ran, ["tcp4"]);
    assert_eq!(lines.len(), 1);
    assert_eq!((lines[0].name.as_str(), lines[0].ok), ("tcp4", false));
    // tcp4 runs first even when --check names only another check.
    let mut ran: Vec<String> = Vec::new();
    let selected = ["packet".to_owned()];
    let lines = net_view(&selected, &mut |name| {
        ran.push(name.to_owned());
        Some((name != "tcp4", String::new()))
    });
    assert_eq!(ran, ["tcp4"]);
    assert!(!lines[0].ok);
    // When tcp4 passes, every check runs in the order of the list.
    let mut ran: Vec<String> = Vec::new();
    let lines = net_view(&[], &mut |name| {
        ran.push(name.to_owned());
        Some((true, String::new()))
    });
    assert_eq!(ran, NET_VIEW_CHECKS);
    assert!(lines.iter().all(|line| line.ok));
    let mut ran: Vec<String> = Vec::new();
    net_view(&selected, &mut |name| {
        ran.push(name.to_owned());
        Some((true, String::new()))
    });
    assert_eq!(ran, ["tcp4", "packet"]);
}

#[test]
fn net_view_checks_stay_out_of_checks() {
    // The phase 1 self-test keeps its list: no check of the network view runs there.
    // Only the Unix socket check is the same in both.
    for name in NET_VIEW_CHECKS.iter().filter(|name| **name != "unix_socket") {
        assert!(!CHECKS.contains(name), "{name}");
    }
    assert!(CHECKS.contains(&"tcp") && CHECKS.contains(&"abstract_socket"));
    assert_eq!(NET_VIEW_CHECKS.first(), Some(&"tcp4"));
    // A check of the network view without its argument is skipped, as in phase 1.
    let args = SelfTestArgs { net_view: true, ..SelfTestArgs::default() };
    for name in ["abstract_stream", "abstract_dgram", "unix_socket"] {
        assert_eq!(net_view_check(name, &args), None, "{name}");
    }
    assert_eq!(net_view_check("no_such_check", &args), None);
}

#[test]
fn check_lines_round_trip() {
    let line = CheckLine { name: "tcp".to_owned(), ok: true, detail: "Err".to_owned() };
    let text = serde_json::to_string(&line).unwrap();
    assert_eq!(serde_json::from_str::<CheckLine>(&text).unwrap(), line);
}

#[test]
fn proxy_checks_stay_apart_and_the_probe_runs_four() {
    for name in PROXY_CHECKS {
        assert!(!CHECKS.contains(name) && !NET_VIEW_CHECKS.contains(name), "{name}");
    }
    assert_eq!(
        PROBE_PROXY_CHECKS,
        ["proxy_tcp", "proxy_socket_direct", "proxy_tcp_other", "proxy_bridge_signal"]
    );
    assert!(PROBE_PROXY_CHECKS.iter().all(|name| PROXY_CHECKS.contains(name)));
    // A check without its argument is skipped, as in the other lists.
    let args = SelfTestArgs { proxy: true, ..SelfTestArgs::default() };
    for name in ["proxy_socket_direct", "proxy_udp"] {
        assert_eq!(proxy_check(name, &args), None, "{name}");
    }
    assert_eq!(proxy_check("no_such_check", &args), None);
}

#[test]
fn the_pid_of_a_process_in_its_own_namespace_and_no_bridge_here() {
    // Outside a sandbox the host's pid is the namespace's pid.
    let own: i32 = fs::read_link("/proc/self").unwrap().to_str().unwrap().parse().unwrap();
    assert_eq!(inner_pid(own), Some(rustix::process::getpid().as_raw_nonzero().get()));
    assert_eq!(inner_pid(i32::MAX), None);
    // No process named efr-bridge is a child of an ancestor of this test.
    assert_eq!(bridge_pid(), None);
}

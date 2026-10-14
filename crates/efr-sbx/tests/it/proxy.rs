//! The proxy plan on the real launcher (the spec's sections 8.2 and 16.6): the bridge,
//! its own sandbox, the call's socket, and what the child can and cannot reach. A test
//! listener stands for efrd's proxy (`support::TestProxy`); no test reaches a real host,
//! and the only fixed port is 3128 inside the call's own network namespace.

use std::fs;
use std::net::{IpAddr, SocketAddr, TcpListener};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;
use std::path::PathBuf;

use efr_protocol::Grant;
use efr_sandbox::{NO_PROXY, PROXY_SOCKET_FILE, PROXY_URL, PROXY_VARIABLES, STARTED_FILE};

use crate::support::{
    Fixture, ProxyAnswer, REFUSE_LINE, Run, TUNNEL_LINE, alive, q, run_with_timeout,
    sandbox_or_skip, target_tmp,
};

/// A line that connects to the bridge, asks for a tunnel to `probe.test:443`, prints
/// the answer's status line, sends `ping` and prints what comes back.
pub(crate) const TUNNEL: &str = "zmodload zsh/net/tcp; ztcp 127.0.0.1 3128 || exit 9; fd=$REPLY; \
    print -n -u $fd $'CONNECT probe.test:443 HTTP/1.1\\r\\nHost: probe.test:443\\r\\n\\r\\n'; \
    read -r -u $fd status_line; read -r -u $fd blank; print -r -- \"status ${status_line%$'\\r'}\"; \
    print -n -u $fd ping; read -k 4 -u $fd back; print -r -- \"back $back\"; ztcp -c $fd";

/// The head that [`TUNNEL`] sends, and its `ping`.
pub(crate) const TUNNEL_BYTES: &[u8] =
    b"CONNECT probe.test:443 HTTP/1.1\r\nHost: probe.test:443\r\n\r\nping";

/// The launcher inside the sandbox, quoted for a line.
fn launcher(fixture: &Fixture) -> String {
    q(&fixture.spec.runtime.inside_launcher())
}

/// The bridge's socket inside the call, quoted for a line.
fn inside_socket(fixture: &Fixture) -> String {
    q(&fixture.spec.runtime.inside_proxy_socket())
}

/// A self-test line with the checks of a proxy call.
fn proxy_self_test(fixture: &Fixture, check: &str, args: &str) -> String {
    format!(
        "{} self-test --proxy --check {check} --proxy-socket {} {args}",
        launcher(fixture),
        inside_socket(fixture)
    )
}

/// The one line of `check` that a self-test printed: whether it passed, and the whole
/// line for the message.
fn check_line(run: &Run, check: &str) -> (bool, String) {
    let line = run
        .stdout
        .lines()
        .find(|line| line.contains(&format!("\"name\":\"{check}\"")))
        .unwrap_or_else(|| panic!("no {check} line: {run:#?}"))
        .to_owned();
    (line.contains("\"ok\":true"), line)
}

/// Fails unless `check` printed its line and passed.
fn passed(run: &Run, check: &str) {
    let (ok, line) = check_line(run, check);
    assert!(ok, "{line}\n{run:#?}");
}

/// True while a bridge of `fixture` runs: its command line names the fixture's own
/// inside socket, which no call of another test has.
pub(crate) fn bridge_alive(fixture: &Fixture) -> bool {
    alive(&fixture.spec.runtime.inside_proxy_socket().to_string_lossy())
}

/// A line that runs the bridge's starter with its server stopped before `step`, on
/// `/dev/null` (the server stops before it would accept), prints the starter's exit
/// status, and prints `found` when a server of that start still runs.
fn failed_start(fixture: &Fixture, step: &str) -> String {
    format!(
        "{} bridge --listen-fd 0 --socket {} --fail-setup {step} </dev/null; \
         print -r -- \"starter $?\"; \
         for f in /proc/[0-9]*/cmdline(N); do c=\"$(<$f)\"; \
         [[ $c == *fail-setup* && $c == *{}* ]] && print found; done",
        launcher(fixture),
        inside_socket(fixture),
        fixture.spec.runtime.inside_proxy_socket().display()
    )
}

#[test]
fn bridge_copies_tcp_to_proxy_socket() {
    let ready = sandbox_or_skip!();
    let mut fixture = Fixture::new(&ready);
    let proxy = fixture.proxy(ProxyAnswer::Tunnel);
    let run = fixture.run(TUNNEL);
    run.expect_status(0);
    assert!(run.stdout.contains(&format!("status {TUNNEL_LINE}")), "{run:#?}");
    assert!(run.stdout.contains("back ping"), "{run:#?}");
    // Both ways, in order: the head and the ping reached the socket, and the echo came
    // back through the bridge.
    let connections = proxy.connections();
    assert_eq!(connections.len(), 1, "{connections:?}");
    assert_eq!(connections[0], TUNNEL_BYTES, "{:?}", String::from_utf8_lossy(&connections[0]));
    // A refusal of efrd's proxy reaches the tool the same way.
    let mut fixture = Fixture::new(&ready);
    let refusing = fixture.proxy(ProxyAnswer::Refuse);
    let run = fixture.run(TUNNEL);
    assert!(run.stdout.contains(&format!("status {REFUSE_LINE}")), "{run:#?}");
    assert_eq!(refusing.connections().len(), 1);
}

#[test]
fn bridge_not_dumpable() {
    let ready = sandbox_or_skip!();
    let mut fixture = Fixture::new(&ready);
    fixture.proxy(ProxyAnswer::Tunnel);
    // The real bridge: the kernel gives the files of a process that is not dumpable to
    // root, which the call's user namespace shows as the overflow uid, and the read of
    // its environment fails with EACCES.
    let run = fixture.run(&proxy_self_test(&fixture, "proxy_bridge_environ", ""));
    passed(&run, "proxy_bridge_environ");
    // The bridge's own check after its setup: prctl(PR_GET_DUMPABLE) gives 0.
    let run = fixture.run(&format!(
        "{} bridge --check-dumpable --socket {}",
        launcher(&fixture),
        inside_socket(&fixture)
    ));
    run.expect_status(0);
    passed(&run, "dumpable");
}

#[test]
fn child_cannot_connect_proxy_socket_directly() {
    let ready = sandbox_or_skip!();
    let mut fixture = Fixture::new(&ready);
    let proxy = fixture.proxy(ProxyAnswer::Tunnel);
    // The direct connect, then a control through the bridge in the same call.
    let line = format!("{}; {TUNNEL}", proxy_self_test(&fixture, "proxy_socket_direct", ""));
    let run = fixture.run(&line);
    passed(&run, "proxy_socket_direct");
    assert!(run.stdout.contains("back ping"), "{run:#?}");
    // The socket's own accept log has only the bridge's connection of the control.
    assert_eq!(proxy.connections(), [TUNNEL_BYTES.to_vec()]);
}

#[test]
fn escape_direct_tcp_still_unreachable_with_proxy() {
    let ready = sandbox_or_skip!();
    let mut fixture = Fixture::new(&ready);
    let proxy = fixture.proxy(ProxyAnswer::Tunnel);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let port = listener.local_addr().unwrap().port();
    let run = fixture.run(&format!(
        "zmodload zsh/net/tcp; ztcp 127.0.0.1 {port} && exit 0; ztcp 192.0.2.1 9 && exit 0; exit 1"
    ));
    run.expect_status(1);
    assert!(listener.accept().is_err(), "the host's listener got a connection");
    let run = fixture.run(&proxy_self_test(&fixture, "proxy_tcp_other", "--tcp 192.0.2.1:9"));
    passed(&run, "proxy_tcp_other");
    // Only the bridge answers.
    let run = fixture.run(&proxy_self_test(&fixture, "proxy_tcp", ""));
    passed(&run, "proxy_tcp");
    assert_eq!(proxy.connections().len(), 1);
    assert!(listener.accept().is_err());
}

#[test]
fn escape_dns_unreachable_with_proxy() {
    let ready = sandbox_or_skip!();
    let mut fixture = Fixture::new(&ready);
    fixture.proxy(ProxyAnswer::Tunnel);
    // A TEST-NET name server, and the host's first one that is not on its loopback (a
    // loopback name server would be the call's own loopback inside). getent proves
    // nothing: a name below .test never resolves.
    let mut targets = vec!["192.0.2.53:53".to_owned()];
    let resolv = fs::read_to_string("/etc/resolv.conf").unwrap_or_default();
    let first = resolv
        .lines()
        .filter_map(|line| line.strip_prefix("nameserver"))
        .filter_map(|word| word.trim().parse::<IpAddr>().ok())
        .find(|address| !address.is_loopback());
    if let Some(address) = first {
        targets.push(SocketAddr::new(address, 53).to_string());
    }
    let args: Vec<String> = targets.iter().map(|target| format!("--udp {target}")).collect();
    let run = fixture.run(&proxy_self_test(&fixture, "proxy_udp", &args.join(" ")));
    passed(&run, "proxy_udp");
    for target in &targets {
        assert!(check_line(&run, "proxy_udp").1.contains(target.as_str()), "{run:#?}");
    }
}

#[test]
fn child_cannot_signal_bridge() {
    let ready = sandbox_or_skip!();
    let mut fixture = Fixture::new(&ready);
    fixture.proxy(ProxyAnswer::Tunnel);
    let run = fixture.run(&proxy_self_test(&fixture, "proxy_bridge_signal", ""));
    passed(&run, "proxy_bridge_signal");
}

#[test]
fn bridge_ends_with_call() {
    let ready = sandbox_or_skip!();
    let mut fixture = Fixture::new(&ready);
    fixture.proxy(ProxyAnswer::Tunnel);
    // The bridge runs while the call runs: the self-test finds it.
    let run = fixture.run(&proxy_self_test(&fixture, "proxy_bridge_signal", ""));
    passed(&run, "proxy_bridge_signal");
    // The launcher writes result.json after every process of the call is gone.
    assert!(run.result().setup_error.is_none(), "{run:#?}");
    assert!(!bridge_alive(&fixture), "a bridge outlived its call");
}

#[test]
fn bridge_not_in_zsh_jobs() {
    let ready = sandbox_or_skip!();
    let mut fixture = Fixture::new(&ready);
    fixture.proxy(ProxyAnswer::Tunnel);
    // bwrap's init holds the bridge, so the child's shell has no job to wait for.
    let run = fixture.run("wait; jobs -l; print -r -- \"jobs done\"");
    run.expect_status(0);
    assert_eq!(run.stdout, "jobs done\n", "{run:#?}");
}

#[test]
fn bridge_port_3128_taken_inside() {
    let ready = sandbox_or_skip!();
    let mut fixture = Fixture::new(&ready);
    fixture.proxy(ProxyAnswer::Tunnel);
    let run = fixture.run(&proxy_self_test(&fixture, "proxy_port_taken", ""));
    passed(&run, "proxy_port_taken");
}

#[test]
fn proxy_env_points_at_bridge() {
    let ready = sandbox_or_skip!();
    let mut fixture = Fixture::new(&ready);
    fixture.proxy(ProxyAnswer::Tunnel);
    // The trusted shell's own proxy never reaches the call.
    fixture.env.insert("https_proxy".into(), "http://corp.test:8080".into());
    fixture.env.insert("NO_PROXY".into(), "corp.test".into());
    let run = fixture.run("env");
    run.expect_status(0);
    let env: Vec<(&str, &str)> =
        run.stdout.lines().filter_map(|line| line.split_once('=')).collect();
    let value = |name: &str| env.iter().find(|(key, _)| *key == name).map(|(_, value)| *value);
    for name in PROXY_VARIABLES {
        assert_eq!(value(name), Some(PROXY_URL), "{name}: {run:#?}");
    }
    for name in ["NO_PROXY", "no_proxy"] {
        assert_eq!(value(name), Some(NO_PROXY), "{name}");
    }
    assert_eq!(value("NODE_USE_ENV_PROXY"), Some("1"));
    // A call with a network gets no offline hint.
    for name in ["CARGO_NET_OFFLINE", "npm_config_offline", "UV_OFFLINE", "GOPROXY"] {
        assert_eq!(value(name), None, "{name}");
    }
    let removed = &run.result().env_removed;
    assert!(removed.contains(&"https_proxy".to_owned()), "{removed:?}");
    assert!(removed.contains(&"NO_PROXY".to_owned()), "{removed:?}");
}

#[test]
fn child_cannot_chmod_proxy_socket() {
    let ready = sandbox_or_skip!();
    let mut fixture = Fixture::new(&ready);
    fixture.proxy(ProxyAnswer::Tunnel);
    let run = fixture.run(&format!("chmod 0666 {}", inside_socket(&fixture)));
    assert_ne!(run.result().exit_code, Some(0), "{run:#?}");
    assert!(run.stderr.to_lowercase().contains("read-only file system"), "{run:#?}");
    let host = run.call_dir.join(PROXY_SOCKET_FILE);
    assert_eq!(fs::metadata(&host).unwrap().permissions().mode() & 0o777, 0o600);
}

#[test]
fn proxy_plan_with_open_network_is_refused() {
    let ready = sandbox_or_skip!();
    let mut fixture = Fixture::new(&ready);
    let proxy = fixture.proxy(ProxyAnswer::Tunnel);
    fixture.grant(Grant::OpenNetwork);
    let run = fixture.run(TUNNEL);
    // The spec check refuses it before anything starts.
    assert_eq!(run.status, 125, "{run:#?}");
    assert!(run.stderr.contains("proxy plan beside a grant of the whole network"), "{run:#?}");
    assert!(!run.call_dir.join(STARTED_FILE).exists());
    assert!(!run.stdout.contains("status"), "{run:#?}");
    assert!(proxy.connections().is_empty());
}

#[test]
fn bridge_not_ready_without_confinement() {
    let ready = sandbox_or_skip!();
    let mut fixture = Fixture::new(&ready);
    fixture.proxy(ProxyAnswer::Tunnel);
    for step in ["landlock", "seccomp"] {
        let run = fixture.run(&failed_start(&fixture, step));
        assert!(run.stdout.contains("starter 1"), "{step}: {run:#?}");
        assert!(!run.stdout.contains("found"), "{step}: {run:#?}");
    }
}

#[test]
fn escape_proxy_socket_host_path_hidden() {
    let ready = sandbox_or_skip!();
    let mut fixture = Fixture::new(&ready);
    fixture.proxy(ProxyAnswer::Tunnel);
    let call_dir = fixture.prepare("");
    let host = call_dir.join(PROXY_SOCKET_FILE);
    assert!(host.exists());
    let line =
        format!("[[ -e {path} ]]; print -r -- \"exists $?\"; ls -l {path} 2>&1", path = q(&host));
    fs::write(call_dir.join(efr_sandbox::LINE_FILE), line).unwrap();
    let ended = run_with_timeout(fixture.command(&call_dir, &fixture.project));
    let run = fixture.collect(&call_dir, ended);
    assert!(run.stdout.contains("exists 1"), "{run:#?}");
    assert!(run.stdout.contains("No such file or directory"), "{run:#?}");
}

#[test]
fn bridge_cannot_write_or_reach_bus() {
    let ready = sandbox_or_skip!();
    let mut fixture = Fixture::new(&ready);
    fixture.proxy(ProxyAnswer::Tunnel);
    // A socket that the call may reach, as a bus grant gives one.
    let dir = tempfile::Builder::new().prefix("k").tempdir_in(target_tmp()).unwrap();
    let socket: PathBuf = dir.path().canonicalize().unwrap().join("bus");
    let _listener = UnixListener::bind(&socket).unwrap();
    fixture.grant(Grant::Socket { path: socket.clone() });
    let line = format!(
        "zmodload zsh/net/socket; zsocket {path} && print reached; \
         {} bridge --check-dumpable --socket {} --write \"$PWD\" --reach {path}",
        launcher(&fixture),
        inside_socket(&fixture),
        path = q(&socket)
    );
    let run = fixture.run(&line);
    // The call itself reaches the socket and writes the project; the bridge's domain
    // does neither, and it has no network socket, no bind and no exec.
    assert!(run.stdout.contains("reached"), "{run:#?}");
    run.expect_status(0);
    for check in ["dumpable", "write", "reach", "inet", "bind", "exec"] {
        passed(&run, check);
    }
    assert!(!fixture.project.join(".efr-bridge-write").exists());
}

#[test]
fn bridge_start_failure_is_a_setup_error() {
    let ready = sandbox_or_skip!();
    let mut fixture = Fixture::new(&ready);
    fixture.proxy(ProxyAnswer::Tunnel);
    // A stop in each step of the server, not only in the starter's own work: the
    // starter reports it with exit 1, which the inner stage turns into a setup error,
    // and no server of that start keeps running.
    for step in ["close", "session", "name", "dumpable", "landlock", "seccomp"] {
        let run = fixture.run(&failed_start(&fixture, step));
        assert!(run.stdout.contains("starter 1"), "{step}: {run:#?}");
        assert!(!run.stdout.contains("found"), "{step}: {run:#?}");
    }
    // A proxy plan without efrd's socket fails as a setup failure, and the line does
    // not run.
    let call_dir = fixture.prepare("print ran");
    fs::remove_file(call_dir.join(PROXY_SOCKET_FILE)).unwrap();
    let ended = run_with_timeout(fixture.command(&call_dir, &fixture.project));
    let run = fixture.collect(&call_dir, ended);
    assert!(run.result().setup_error.is_some(), "{run:#?}");
    assert!(!run.stdout.contains("ran"), "{run:#?}");
}

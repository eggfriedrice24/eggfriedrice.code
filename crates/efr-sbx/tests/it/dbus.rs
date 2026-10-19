//! The buses on the real launcher: a socket grant never reaches a bus, and (phase E) the
//! filtered bus sockets of a call. A `UnixListener` of the test stands for each bus and
//! for efrd's bus proxy; no test reaches the machine's real system or session bus, and
//! no test needs a D-Bus tool.

use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use efr_protocol::{BusKind, Grant};

use crate::support::{
    Fixture, Run, bus_word, command, q, run_with_timeout, sandbox_or_skip, target_tmp,
};

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

/// A line that connects to the socket at `path` inside (a shell word), reads one line
/// and prints `<label> <line>`, or `<label> failed` with zsh's error on stderr.
fn ask_line(label: &str, path: &str) -> String {
    format!(
        "zmodload zsh/net/socket; if zsocket {path}; then read -r -u $REPLY got; \
         print -r -- \"{label} $got\"; else print -r -- \"{label} failed\"; fi"
    )
}

/// Fails unless `run` printed the line `line`.
fn printed(run: &Run, line: &str) {
    assert!(run.stdout.lines().any(|got| got == line), "no line {line:?}: {run:#?}");
}

/// A dir below the target dir's temp dir for a test's own sockets, short enough for a
/// socket address, and visible inside the sandbox read-only.
fn socket_dir() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::Builder::new().prefix("b").tempdir_in(target_tmp()).unwrap();
    let path = dir.path().canonicalize().unwrap();
    (dir, path)
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

/// The wrapper's command: `efr-sbx dbus-proxy --dir <dir> --upstream <each> -- <the
/// launcher> self-test --bus-proxy --ready-fd 3 <checks>`. The launcher stands for
/// `xdg-dbus-proxy`: the domain lets a program read only itself and the library dirs.
pub(crate) fn wrapper(bin: &Path, dir: &Path, upstream: &[PathBuf], checks: &[String]) -> Command {
    let mut command = command(bin);
    command.arg("dbus-proxy").arg("--dir").arg(dir);
    for path in upstream {
        command.arg("--upstream").arg(path);
    }
    command
        .arg("--")
        .arg(bin)
        .args(["self-test", "--bus-proxy", "--ready-fd", "3"])
        .args(checks)
        .current_dir("/")
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

/// The checks that the program behind the wrapper wrote after its ready byte.
fn check_lines(stdout: &str) -> Vec<(String, bool, String)> {
    stdout
        .strip_prefix('x')
        .unwrap_or_default()
        .lines()
        .map(|line| {
            let value: serde_json::Value = serde_json::from_str(line).unwrap();
            (
                value["name"].as_str().unwrap_or_default().to_owned(),
                value["ok"].as_bool().unwrap_or_default(),
                value["detail"].as_str().unwrap_or_default().to_owned(),
            )
        })
        .collect()
}

#[test]
fn dbus_proxy_wrapper_moves_stdout_to_fd_3() {
    let ready = sandbox_or_skip!();
    let (_dir, base) = socket_dir();
    let bus_dir = base.join("bus");
    std::fs::create_dir(&bus_dir).unwrap();
    // The wrapper's stdout, efrd's sync pipe, is the program's fd 3: the ready byte and
    // the lines come there, and the program's own stdout is the wrapper's stderr.
    let checks = ["--check".to_owned(), "stdout_is_stderr".to_owned()];
    let (status, stdout, stderr) = run_with_timeout(wrapper(&ready.bin, &bus_dir, &[], &checks));
    assert!(stdout.starts_with('x'), "no ready byte: {stdout:?} {stderr}");
    let lines = check_lines(&stdout);
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(lines.iter().all(|(_, ok, _)| *ok), "{lines:?}");
    assert_eq!(status, 0, "{stderr}");
    // A setup failure exits 125 with one line on stderr, before the program runs.
    let mut relative = command(&ready.bin);
    relative
        .args(["dbus-proxy", "--dir"])
        .arg(&bus_dir)
        .args(["--", "efr-sbx", "self-test"])
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let (status, stdout, stderr) = run_with_timeout(relative);
    assert_eq!(status, efr_sandbox::SETUP_FAILURE_STATUS, "{stderr}");
    assert!(stdout.is_empty(), "{stdout:?}");
    assert_eq!(stderr.lines().count(), 1, "{stderr}");
    assert!(stderr.starts_with("efr-sbx dbus-proxy: "), "{stderr}");
}

#[test]
fn dbus_proxy_wrapper_confines() {
    let ready = sandbox_or_skip!();
    let (_dir, base) = socket_dir();
    let bus_dir = base.join("bus");
    let others = base.join("o");
    let home = base.join("h");
    for dir in [&bus_dir, &others, &home] {
        std::fs::create_dir(dir).unwrap();
    }
    // The bus that the program may reach, and sockets that it may not: another socket,
    // efrd's own and a call's proxy socket, as stand-ins.
    let upstream = stand_in(&base.join("up"));
    let refused: Vec<PathBuf> =
        ["other.sock", "daemon.sock", "proxy.sock"].iter().map(|name| others.join(name)).collect();
    let listeners: Vec<UnixListener> = refused.iter().map(|path| stand_in(path)).collect();
    std::fs::write(home.join("secret"), "token").unwrap();
    let tcp = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    tcp.set_nonblocking(true).unwrap();
    let mut args = vec![
        "--bind".to_owned(),
        "probe.sock".to_owned(),
        "--upstream".to_owned(),
        base.join("up").display().to_string(),
        "--write-dir".to_owned(),
        home.display().to_string(),
        "--read".to_owned(),
        home.join("secret").display().to_string(),
        "--tcp".to_owned(),
        tcp.local_addr().unwrap().to_string(),
    ];
    for path in &refused {
        args.extend(["--refuse".to_owned(), path.display().to_string()]);
    }
    let (status, stdout, stderr) =
        run_with_timeout(wrapper(&ready.bin, &bus_dir, &[base.join("up")], &args));
    let lines = check_lines(&stdout);
    let names: Vec<&str> = lines.iter().map(|(name, _, _)| name.as_str()).collect();
    assert_eq!(
        names,
        [
            "stdout_is_stderr",
            "bind_in_dir",
            "upstream",
            "other_socket",
            "write_outside",
            "read_outside",
            "tcp",
            "fds"
        ],
        "{stdout:?} {stderr}"
    );
    for (name, ok, detail) in &lines {
        assert!(ok, "{name}: {detail}");
    }
    assert_eq!(status, 0, "{stderr}");
    assert!(reached(&upstream), "the bus got no connection");
    assert!(!listeners.iter().any(reached), "a refused socket got a connection");
    assert!(tcp.accept().is_err(), "the TCP listener got a connection");
    assert!(!home.join(".efr-bus-proxy").exists());
    // The socket that the program made in its dir is gone again.
    assert!(!bus_dir.join("probe.sock").exists());
}

#[test]
fn bus_filter_sockets_reachable() {
    let ready = sandbox_or_skip!();
    let mut fixture = Fixture::new(&ready);
    let buses = fixture.bus_filter(&[BusKind::System, BusKind::Session]);
    let line = format!(
        "{}; {}",
        ask_line("system", "/run/dbus/system_bus_socket"),
        ask_line("session", "$XDG_RUNTIME_DIR/bus")
    );
    let run = fixture.run(&line);
    run.expect_status(0);
    // The connect works through the read-only bind at both places (C2-2), and the
    // tmpfs on /run/dbus gives the system socket its place (C2-3).
    printed(&run, &format!("system {}", bus_word(BusKind::System)));
    printed(&run, &format!("session {}", bus_word(BusKind::Session)));
    assert_eq!(buses.connections(), [BusKind::System, BusKind::Session]);
    // The rest of the host's /run/dbus is out of view: only the call's socket is there.
    let run = fixture.run("print -r -- /run/dbus/*(N:t)");
    printed(&run, "system_bus_socket");
}

#[test]
fn bus_env_points_at_filter_sockets() {
    let ready = sandbox_or_skip!();
    let mut fixture = Fixture::new(&ready);
    fixture.bus_filter(&[BusKind::System, BusKind::Session]);
    // The trusted shell's own addresses never reach the call.
    fixture
        .env
        .insert("DBUS_SESSION_BUS_ADDRESS".into(), "unix:path=/run/user/1/elsewhere,guid=0".into());
    fixture.env.insert("DBUS_SYSTEM_BUS_ADDRESS".into(), "unix:path=/elsewhere".into());
    let run = fixture.run(
        "print -r -- \"sys $DBUS_SYSTEM_BUS_ADDRESS\"; print -r -- \"ses $DBUS_SESSION_BUS_ADDRESS\"",
    );
    run.expect_status(0);
    printed(&run, "sys unix:path=/run/dbus/system_bus_socket");
    printed(&run, &format!("ses unix:path={}", fixture.xrt.join("bus").display()));
}

#[test]
fn escape_real_bus_denied_with_bus_filter() {
    let ready = sandbox_or_skip!();
    let mut fixture = Fixture::new(&ready);
    // The host's buses, as the spec names them: stand-ins of the test, in a dir that
    // the sandbox shows read-only. The call gets only its filtered sockets.
    let (_dir, base) = socket_dir();
    let system = stand_in(&base.join("system"));
    let session = stand_in(&base.join("session"));
    fixture.spec.runtime.system_bus = base.join("system");
    fixture.spec.runtime.session_bus = Some(base.join("session"));
    fixture.bus_filter(&[BusKind::System, BusKind::Session]);
    let line = format!(
        "{}; {}; {}",
        connect_line(&base.join("system")),
        connect_line(&base.join("session")),
        ask_line("filtered", "/run/dbus/system_bus_socket")
    );
    let run = fixture.run(&line);
    assert!(!run.stdout.contains("reached"), "{run:#?}");
    assert!(run.stderr.to_lowercase().contains("permission denied"), "{run:#?}");
    printed(&run, &format!("filtered {}", bus_word(BusKind::System)));
    assert!(!reached(&system) && !reached(&session), "a host bus got a connection");
}

#[test]
fn escape_bus_filter_host_path_hidden() {
    let ready = sandbox_or_skip!();
    let mut fixture = Fixture::new(&ready);
    fixture.bus_filter(&[BusKind::Session]);
    let host = fixture.spec.runtime.shell_dir.clone();
    // The call dir lies below the masked runtime root: the host path of the call's
    // socket is out of view, so the child reaches the filter only at $XRT/bus.
    let line = format!(
        "for f in {}/*/bus/session.sock(N); do print -r -- \"found $f\"; done; \
         [[ -e {} ]]; print -r -- \"exists $?\"",
        q(&host),
        q(&host)
    );
    let run = fixture.run(&line);
    assert!(!run.stdout.contains("found"), "{run:#?}");
    printed(&run, "exists 1");
}

#[test]
fn escape_systemd_private_denied() {
    let ready = sandbox_or_skip!();
    let mut fixture = Fixture::new(&ready);
    fixture.bus_filter(&[BusKind::Session]);
    // The user manager's private socket of the host, as a stand-in in `$XRT`: the call's
    // `$XRT` is an empty tmpfs with only the launcher's dir and the filtered bus.
    let systemd = fixture.xrt.join("systemd");
    std::fs::create_dir(&systemd).unwrap();
    let private = stand_in(&systemd.join("private"));
    let line = "zmodload zsh/net/socket; zsocket $XDG_RUNTIME_DIR/systemd/private && \
                print reached; print -r -- \"names\" $XDG_RUNTIME_DIR/*(N:t)";
    let run = fixture.run(line);
    assert!(!run.stdout.contains("reached"), "{run:#?}");
    assert!(!reached(&private), "the stand-in manager got a connection");
    let names = run.stdout.lines().find(|got| got.starts_with("names")).unwrap_or_default();
    let names: Vec<&str> = names.split(' ').skip(1).collect();
    assert!(names.contains(&"bus") && !names.contains(&"systemd"), "{run:#?}");
}

#[test]
fn resolve_unix_holds_only_the_bus_paths() {
    let ready = sandbox_or_skip!();
    let mut fixture = Fixture::new(&ready);
    fixture.bus_filter(&[BusKind::System]);
    // A socket in the project, a write root, and one in a read-only dir: Landlock gives
    // the child `RESOLVE_UNIX` on the filtered bus only.
    let own = stand_in(&fixture.project.join("own.sock"));
    let (_dir, base) = socket_dir();
    let other = stand_in(&base.join("other"));
    let line = format!(
        "{}; {}; {}",
        connect_line(&fixture.project.join("own.sock")),
        connect_line(&base.join("other")),
        ask_line("filtered", "/run/dbus/system_bus_socket")
    );
    let run = fixture.run(&line);
    assert!(!run.stdout.contains("reached"), "{run:#?}");
    printed(&run, &format!("filtered {}", bus_word(BusKind::System)));
    assert!(!reached(&own) && !reached(&other), "a socket outside the rules got a connection");
}

/// Puts a stand-in `systemctl` first on the launcher's `PATH`: it prints its words and
/// whether `XDG_RUNTIME_DIR` is set.
fn stand_in_systemctl(fixture: &mut Fixture) -> tempfile::TempDir {
    use std::os::unix::fs::PermissionsExt;
    let bin = tempfile::Builder::new().prefix("p").tempdir_in(target_tmp()).unwrap();
    let path = bin.path().canonicalize().unwrap();
    let program = path.join("systemctl");
    std::fs::write(
        &program,
        "#!/bin/sh\nprintf 'words %s xrt %s\\n' \"$*\" \"${XDG_RUNTIME_DIR-unset}\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    let old = fixture.env.get(std::ffi::OsStr::new("PATH")).cloned().unwrap_or_default();
    let mut new = path.into_os_string();
    new.push(":");
    new.push(old);
    fixture.env.insert("PATH".into(), new);
    bin
}

#[test]
fn systemctl_user_function_unsets_runtime_dir() {
    let ready = sandbox_or_skip!();
    let mut fixture = Fixture::new(&ready);
    let _bin = stand_in_systemctl(&mut fixture);
    let xrt = fixture.xrt.display().to_string();
    let line = "systemctl --user --failed; systemctl --failed; \
                systemctl status --user x.service; systemctl -- --user; \
                command systemctl --user --failed";
    // No session bus in the call: systemctl runs as it is.
    let run = fixture.run(line);
    run.expect_status(0);
    printed(&run, &format!("words --user --failed xrt {xrt}"));
    // The filtered session bus: `--user` runs with no XDG_RUNTIME_DIR, so systemd uses
    // the bus; every other form, and `command systemctl`, keeps it.
    fixture.bus_filter(&[BusKind::Session]);
    let run = fixture.run(line);
    run.expect_status(0);
    let lines: Vec<&str> = run.stdout.lines().collect();
    assert_eq!(
        lines,
        [
            "words --user --failed xrt unset".to_owned(),
            format!("words --failed xrt {xrt}"),
            "words status --user x.service xrt unset".to_owned(),
            format!("words -- --user xrt {xrt}"),
            format!("words --user --failed xrt {xrt}"),
        ],
        "{run:#?}"
    );
    // The function is efr's: the records do not report it as the line's.
    let functions: Vec<String> = run.apply_fields();
    assert!(!functions.iter().any(|field| field == "systemctl"), "{functions:?}");
}

#[test]
fn systemctl_user_function_with_a_granted_session_bus() {
    let ready = sandbox_or_skip!();
    let mut fixture = Fixture::new(&ready);
    let _bin = stand_in_systemctl(&mut fixture);
    // An approved grant of the whole session bus, here a stand-in at the user's
    // `$XRT/bus`: the same function reaches it (check C2-5).
    let bus = fixture.xrt.join("bus");
    let _listener = stand_in(&bus);
    fixture.spec.runtime.session_bus = Some(bus);
    fixture.grant(Grant::Bus { bus: BusKind::Session });
    let run = fixture.run("systemctl --user --failed; print -r -- $DBUS_SESSION_BUS_ADDRESS");
    run.expect_status(0);
    printed(&run, "words --user --failed xrt unset");
    printed(&run, &format!("unix:path={}", fixture.xrt.join("bus").display()));
}

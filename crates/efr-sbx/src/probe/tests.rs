use super::*;

#[test]
fn which_skips_relative_entries() {
    assert_eq!(which("sh", "relative:/nonexistent"), None);
    assert!(which("sh", "relative:/usr/bin:/bin").is_some());
}

#[test]
fn a_namespace_error_names_user_namespaces() {
    let failure = setup_failure("bwrap: No permissions to create a new namespace");
    assert!(matches!(failure, ProbeFailure::UsernsOff | ProbeFailure::AppArmor), "{failure:?}");
    let failure = setup_failure("bwrap: setting up uid map: Permission denied");
    assert!(matches!(failure, ProbeFailure::UsernsOff | ProbeFailure::AppArmor), "{failure:?}");
}

#[test]
fn another_setup_error_is_a_self_test_failure() {
    let failure = setup_failure("bwrap: Can't mount overlay on /x: Device or resource busy");
    assert_eq!(
        failure,
        ProbeFailure::SelfTest {
            detail: "bwrap: Can't mount overlay on /x: Device or resource busy".to_owned()
        }
    );
}

#[test]
fn missing_bwrap_has_no_facts() {
    assert_eq!(bwrap_facts(None), BwrapFacts::default());
    assert_eq!(bwrap_facts(Some(Path::new("/nonexistent/bwrap"))), BwrapFacts::default());
}

fn args(dir: &str, user_runtime: &str) -> ProbeArgs {
    ProbeArgs {
        json: true,
        dir: PathBuf::from(dir),
        bwrap: None,
        zsh: None,
        home: None,
        user_runtime: Some(PathBuf::from(user_runtime)),
        cache_mode: "tmp".to_owned(),
        write_roots: Vec::new(),
        shell_path: None,
    }
}

#[test]
fn a_state_or_runtime_dir_below_tmp_names_itself() {
    let state = below_tmp(&args("/tmp/efr-home/state/sandbox/probe", "/"));
    assert!(
        matches!(&state, Some(ProbeFailure::BelowTmp { what, .. }) if what == "efr's state dir"),
        "{state:?}"
    );
    let runtime = below_tmp(&args("/nonexistent/state/sandbox/probe", "/tmp"));
    assert!(
        matches!(&runtime, Some(ProbeFailure::BelowTmp { what, .. }) if what == "XDG_RUNTIME_DIR"),
        "{runtime:?}"
    );
    assert_eq!(below_tmp(&args("/nonexistent/state/sandbox/probe", "/")), None);
}

fn line(name: &str) -> CheckLine {
    CheckLine { name: name.to_owned(), ok: true, detail: String::new() }
}

#[test]
fn a_check_that_printed_no_line_fails_the_probe_by_name() {
    let all: Vec<CheckLine> = CHECKS.iter().map(|name| line(name)).collect();
    assert_eq!(missing_checks(&all, &[]), None);
    let some: Vec<CheckLine> = all
        .iter()
        .filter(|line| line.name != "unix_socket" && line.name != "vsock")
        .cloned()
        .collect();
    let unavailable = [("unix_socket", "no Unix socket at /x/s: path too long".to_owned())];
    let failure = missing_checks(&some, &unavailable).unwrap();
    assert_eq!(
        failure,
        ProbeFailure::SelfTestIncomplete {
            checks: vec!["unix_socket".to_owned(), "vsock".to_owned()],
            detail: "unix_socket: no Unix socket at /x/s: path too long; vsock: it printed no line"
                .to_owned(),
        }
    );
    assert_eq!(failure.check_name(), "self_test");
}

#[test]
fn a_fixture_in_a_deep_dir_still_makes_its_socket() {
    let temp = crate::testing::temp_dir();
    let root = temp.path().canonicalize().unwrap();
    // A sockaddr_un holds at most 108 bytes of path.
    let dir = root.join("d".repeat(120));
    let mut args = args(&dir.to_string_lossy(), &root.to_string_lossy());
    args.home = Some(root.clone());
    let programs = fixture::Programs {
        bwrap: "/usr/bin/bwrap".into(),
        zsh: "/usr/bin/zsh".into(),
        launcher: "/usr/bin/true".into(),
        shell_path: "/usr/bin".to_owned(),
    };
    let fixture = fixture::Fixture::make(&args, &programs).unwrap();
    let socket = fixture.unavailable().iter().find(|(check, _)| *check == "unix_socket");
    assert_eq!(socket, None, "{:?}", fixture.unavailable());
    let args = fixture.self_test_args();
    let at = args.iter().position(|arg| arg == "--socket").unwrap();
    let path = PathBuf::from(&args[at + 1]);
    assert!(path.as_os_str().len() > 108, "{}", path.display());
    // Outside a sandbox the connect works, through the same short path.
    let (dir, short) = crate::self_test::socket_path(&path).unwrap();
    assert!(dir.is_some() && short.starts_with("/proc/self/fd/"), "{}", short.display());
    std::os::unix::net::UnixStream::connect(&short).unwrap();
    drop(dir);
    fixture.remove();
}

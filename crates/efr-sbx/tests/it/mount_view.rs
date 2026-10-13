//! The mount view on the real launcher: `efr-sbx statvfs`, which efrd runs outside the
//! sandbox, and the `mounts` tables, which read the snapshot next to the launcher. These
//! need no sandbox, so they run with the launcher that cargo built when
//! `EFR_TEST_SBX_BIN` is unset. The `mount_view_` tests run `df` and `findmnt` in a real
//! contained call, with the snapshot of the test's own mount namespace in its call dir,
//! as efrd writes it, and so need the sandbox.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::io::Write as _;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};

use efr_sandbox::{
    MOUNTS_FILE, MountSizes, MountSnapshot, SIZES_FILE, StatvfsLine, is_dummy, parse_mountinfo,
    render_df, render_findmnt_df,
};

use crate::support::{
    Fixture, Run, command, env_var, require, run_with_timeout, sandbox_or_skip, target_tmp,
};

/// The launcher of `EFR_TEST_SBX_BIN`, else the one cargo built with this test binary.
fn launcher() -> PathBuf {
    env_var("EFR_TEST_SBX_BIN")
        .map_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_efr-sbx")), PathBuf::from)
}

/// A table of two mounts, and their sizes.
const TABLE: &str = "\
22 1 8:1 / / rw,relatime shared:1 - ext4 /dev/sda1 rw
23 22 0:30 / /tmp rw,nosuid shared:2 - tmpfs tmpfs rw
";
const SIZES: &str = r#"{"taken_at":1,"sizes":{
"22":{"kind":"measured","blocks":51328915,"bfree":25637330,"bavail":23012345,"frsize":4096,"files":13107200,"ffree":12351813},
"23":{"kind":"timeout"}}}"#;

/// A copy of the launcher in a dir of its own, with the snapshot next to it, as the
/// plan binds them into a call.
struct Inside {
    _temp: tempfile::TempDir,
    dir: PathBuf,
}

impl Inside {
    fn new() -> Inside {
        let root = target_tmp();
        std::fs::create_dir_all(&root).unwrap();
        let temp = tempfile::Builder::new().prefix("mv").tempdir_in(&root).unwrap();
        let dir = temp.path().to_path_buf();
        std::fs::copy(launcher(), dir.join("efr-sbx")).unwrap();
        std::fs::write(dir.join("mounts"), TABLE).unwrap();
        std::fs::write(dir.join("mounts.json"), SIZES).unwrap();
        Inside { _temp: temp, dir }
    }

    fn run(&self, args: &[&str], env: &[(&str, &str)]) -> Output {
        command(self.dir.join("efr-sbx"))
            .arg("mounts")
            .args(args)
            .env_clear()
            .envs(env.iter().copied())
            .current_dir(&self.dir)
            .output()
            .unwrap()
    }
}

fn expected_df(args: &[&str]) -> String {
    let args: Vec<OsString> = args.iter().map(OsString::from).collect();
    let snapshot = MountSnapshot::from_json(SIZES.as_bytes()).unwrap();
    render_df(&parse_mountinfo(TABLE.as_bytes()), &snapshot, &args).unwrap()
}

/// Asserts that `output` is exit status 2 with no output: run the real command.
fn assert_runs_real(output: &Output, what: &str) {
    assert_eq!(output.status.code(), Some(2), "{what}: {output:?}");
    assert!(output.stdout.is_empty(), "{what}: {output:?}");
}

#[test]
fn statvfs_answers_each_path_of_stdin() {
    let mut child = command(launcher())
        .arg("statvfs")
        .env_clear()
        .current_dir("/")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    stdin.write_all(b"/\0/no/such/dir\0").unwrap();
    drop(stdin);
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "{output:?}");
    let mut lines: Vec<StatvfsLine> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    lines.sort_by(|a, b| a.path.cmp(&b.path));
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert!(matches!(lines[0].sizes(), MountSizes::Measured { blocks, .. } if blocks > 0));
    assert_eq!(lines[1].path, "/no/such/dir");
    assert_eq!(lines[1].sizes(), MountSizes::Failed { errno: 2 });
}

#[test]
fn mounts_tables_read_the_snapshot_next_to_the_launcher() {
    let inside = Inside::new();
    for args in [&[][..], &["-h"], &["-hT", "--total"], &["-i"]] {
        let mut argv = vec!["df"];
        argv.extend_from_slice(args);
        let output = inside.run(&argv, &[]);
        assert!(output.status.success(), "{args:?}: {output:?}");
        assert_eq!(String::from_utf8_lossy(&output.stdout), expected_df(args), "{args:?}");
    }
    let snapshot = MountSnapshot::from_json(SIZES.as_bytes()).unwrap();
    let entries = parse_mountinfo(TABLE.as_bytes());
    let output = inside.run(&["findmnt-df", "-I"], &[]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        render_findmnt_df(&entries, &snapshot, true)
    );
}

#[test]
fn mounts_df_dash_h_is_df_h() {
    let inside = Inside::new();
    let output = inside.run(&["df", "-h"], &[]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(String::from_utf8_lossy(&output.stdout), expected_df(&["-h"]));
    // Neither clap nor the table answers --help or --version: the real df does.
    for args in [&["df", "--help"][..], &["df", "--version"], &["findmnt-df", "--help"]] {
        assert_runs_real(&inside.run(args, &[]), &format!("{args:?}"));
    }
}

#[test]
fn mounts_df_leaves_operands_and_units_of_the_environment_to_the_real_df() {
    let inside = Inside::new();
    assert_runs_real(&inside.run(&["df", "/tmp"], &[]), "an operand");
    assert_runs_real(&inside.run(&["df", "-h", "."], &[]), "an operand");
    assert_runs_real(&inside.run(&["df", "--direct"], &[]), "an option it does not follow");
    assert_runs_real(&inside.run(&["findmnt-df", "-D", "-t", "ext4"], &[]), "a findmnt filter");
    for name in ["DF_BLOCK_SIZE", "BLOCK_SIZE", "BLOCKSIZE", "POSIXLY_CORRECT"] {
        assert_runs_real(&inside.run(&["df"], &[(name, "1M")]), name);
    }
    std::fs::remove_file(inside.dir.join("mounts.json")).unwrap();
    assert_runs_real(&inside.run(&["df"], &[]), "no sizes");
    assert_runs_real(&inside.run(&["findmnt-df"], &[]), "no sizes");
    std::fs::write(inside.dir.join("mounts.json"), "not json").unwrap();
    assert_runs_real(&inside.run(&["df"], &[]), "sizes that do not read");
}

/// What `efr-sbx statvfs` answers for `paths`, by path.
fn statvfs(paths: &[String]) -> BTreeMap<String, StatvfsLine> {
    let mut child = command(launcher())
        .arg("statvfs")
        .env_clear()
        .current_dir("/")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    for path in paths {
        stdin.write_all(path.as_bytes()).unwrap();
        stdin.write_all(b"\0").unwrap();
    }
    drop(stdin);
    let output = child.wait_with_output().unwrap();
    String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<StatvfsLine>(line).unwrap())
        .map(|line| (line.path.clone(), line))
        .collect()
}

/// Writes the snapshot of the test's own mount namespace into `call_dir`, as efrd does:
/// the table of `/proc/self/mountinfo`, verbatim, and the sizes from a real
/// `efr-sbx statvfs` (no masks: the test has no efrd). Returns what it wrote.
fn write_snapshot(call_dir: &Path) -> (Vec<u8>, MountSnapshot) {
    let table = std::fs::read("/proc/self/mountinfo").unwrap();
    let entries = parse_mountinfo(&table);
    let targets: Vec<String> = entries
        .iter()
        .filter(|entry| !is_dummy(&entry.fstype))
        .filter_map(|entry| entry.target.to_str().map(str::to_owned))
        .collect();
    let answers = statvfs(&targets);
    let mut sizes = BTreeMap::new();
    let mut devices = BTreeMap::new();
    for entry in &entries {
        let answer = entry.target.to_str().and_then(|target| answers.get(target));
        let sized = match answer {
            _ if is_dummy(&entry.fstype) => MountSizes::Dummy,
            Some(line) => line.sizes(),
            None => MountSizes::Failed { errno: 0 },
        };
        sizes.insert(entry.id, sized);
        if let Some(device) = answer.and_then(|line| line.device) {
            devices.insert(entry.id, device);
        }
    }
    let snapshot = MountSnapshot::new(1, sizes).with_devices(devices);
    for (name, bytes) in [(MOUNTS_FILE, table.clone()), (SIZES_FILE, snapshot.to_json().unwrap())] {
        let path = call_dir.join(name);
        std::fs::write(&path, bytes).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    (table, snapshot)
}

/// Runs `line` contained, with the snapshot in its call dir.
fn call_with_snapshot(fixture: &Fixture, line: &str) -> (Run, Vec<u8>, MountSnapshot) {
    let call_dir = fixture.prepare(line);
    let (table, snapshot) = write_snapshot(&call_dir);
    let command = fixture.command(&call_dir, &fixture.project);
    let run = fixture.collect(&call_dir, run_with_timeout(command));
    (run, table, snapshot)
}

/// The output of `program` with `args` in the test's own mount namespace.
fn host(program: &str, args: &[&str]) -> String {
    let output = command(program)
        .args(args)
        .env_clear()
        .env("PATH", env_var("PATH").unwrap_or_default())
        .env("LANG", "C.UTF-8")
        .current_dir("/")
        .output()
        .unwrap();
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// The lines of `text` as a set, each without the spaces at its end.
fn line_set(text: &str) -> BTreeSet<String> {
    text.lines().map(|line| line.trim_end().to_owned()).collect()
}

#[test]
fn mount_view_df_shows_host_mounts() {
    let ready = sandbox_or_skip!();
    require!("df");
    let fixture = Fixture::new(&ready);
    let (run, table, snapshot) = call_with_snapshot(&fixture, "df --output=target; df -h");
    run.expect_status(0);
    let (targets, human) = run.stdout.split_at(run.stdout.find("Filesystem").unwrap());
    let mut inside = line_set(targets);
    let outside = line_set(&host("df", &["--output=target"]));
    let entries = parse_mountinfo(&table);
    // NOTE: the renderer follows upstream coreutils, where devtmpfs is a real file
    // system. Some builds hide it as a dummy type: the df 9.4 of Ubuntu 24.04 (the CI
    // VM) never stats `/dev`, while Arch's 9.12 lists it. Only that difference passes.
    let devtmpfs: Vec<String> = entries
        .iter()
        .filter(|entry| entry.fstype == "devtmpfs")
        .map(|entry| entry.target.to_string_lossy().into_owned())
        .collect();
    if devtmpfs.iter().all(|target| !outside.contains(target)) {
        for target in &devtmpfs {
            inside.remove(target);
        }
    }
    assert_eq!(
        inside,
        outside,
        "{}\n--- mountinfo\n{}\n--- snapshot\n{snapshot:?}\n--- df -a outside\n{}",
        run.stdout,
        String::from_utf8_lossy(&table),
        host("df", &["-a", "--output=source,fstype,size,target"]),
    );
    let base = fixture.base.to_string_lossy().into_owned();
    assert!(!inside.iter().any(|target| target.starts_with(&base)), "{inside:?}");
    let expected = render_df(&entries, &snapshot, &[OsString::from("-h")]).unwrap();
    assert_eq!(human, expected);
}

#[test]
fn mount_view_df_path_is_sandbox_view() {
    let ready = sandbox_or_skip!();
    require!("df");
    let fixture = Fixture::new(&ready);
    // A path: the real df, which shows where a write to /tmp goes in the call.
    let (run, _, _) = call_with_snapshot(&fixture, "df --output=source,fstype /tmp");
    run.expect_status(0);
    let private = fixture.private_tmp().to_string_lossy().into_owned();
    let outside = host("df", &["--output=source,fstype", &private]);
    assert_eq!(line_set(&run.stdout), line_set(&outside), "{run:#?}");
}

#[test]
fn mount_view_findmnt_uses_tab_file() {
    let ready = sandbox_or_skip!();
    require!("findmnt");
    let fixture = Fixture::new(&ready);
    let (run, _, _) = call_with_snapshot(&fixture, "findmnt -rn -o TARGET");
    run.expect_status(0);
    let inside = line_set(&run.stdout);
    assert_eq!(inside, line_set(&host("findmnt", &["-rn", "-o", "TARGET"])), "{run:#?}");
    let base = fixture.base.to_string_lossy().into_owned();
    assert!(!inside.iter().any(|target| target.starts_with(&base)), "{inside:?}");
}

#[test]
fn mount_view_findmnt_df_uses_snapshot_sizes() {
    let ready = sandbox_or_skip!();
    require!("findmnt");
    let fixture = Fixture::new(&ready);
    let (run, table, snapshot) = call_with_snapshot(&fixture, "findmnt -D; findmnt --dfi");
    run.expect_status(0);
    let entries = parse_mountinfo(&table);
    let expected = render_findmnt_df(&entries, &snapshot, false)
        + &render_findmnt_df(&entries, &snapshot, true);
    assert_eq!(run.stdout, expected);
}

#[test]
fn mount_view_missing_snapshot_runs_real_df() {
    let ready = sandbox_or_skip!();
    require!("df");
    let fixture = Fixture::new(&ready);
    // No snapshot: the call's own mounts, the fixture's masks among them.
    let run = fixture.run("df --output=target");
    run.expect_status(0);
    let base = fixture.base.to_string_lossy().into_owned();
    assert!(run.stdout.lines().any(|target| target.starts_with(&base)), "{run:#?}");
}

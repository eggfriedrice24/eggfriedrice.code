//! The mount view on the real launcher: `efr-sbx statvfs`, which efrd runs outside the
//! sandbox, and the `mounts` tables, which read the snapshot next to the launcher. They
//! need no sandbox, so they run with the launcher that cargo built when
//! `EFR_TEST_SBX_BIN` is unset.

use std::ffi::OsString;
use std::io::Write as _;
use std::path::PathBuf;
use std::process::{Output, Stdio};

use efr_sandbox::{
    MountSizes, MountSnapshot, StatvfsLine, parse_mountinfo, render_df, render_findmnt_df,
};

use crate::support::{command, env_var, target_tmp};

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

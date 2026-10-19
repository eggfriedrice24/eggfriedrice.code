use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use crate::trusted_programs::{Program, TrustedPrograms};

fn root() -> (tempfile::TempDir, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(temp.path()).unwrap();
    (temp, root)
}

/// A fake program `name` in `dir` that writes `ran` next to itself.
fn fake(dir: &Path, name: &str) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, "#!/bin/sh\ntouch \"$(dirname \"$0\")/ran\"\n").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[test]
fn real_programs_need_a_root_owned_dir() {
    let (_temp, root) = root();
    let bin = root.join("bin");
    fake(&bin, "crontab");

    let programs = TrustedPrograms::resolve(bin.as_os_str(), &[]);

    // The test's own user owns the dir, so the real rule refuses it.
    assert!(!programs.has(Program::Crontab));
    assert_eq!(programs, TrustedPrograms::none());
}

#[test]
fn a_program_below_a_write_root_never_counts() {
    let (_temp, root) = root();
    let home = root.join("home");
    let bin = home.join("p/app/bin");
    fake(&bin, "crontab");
    fake(&root.join("fake"), "loginctl");

    let path = std::env::join_paths([bin.clone(), root.join("fake")]).unwrap();
    let programs = TrustedPrograms::find(&path, std::slice::from_ref(&home), false);

    assert!(!programs.has(Program::Crontab), "the home dir is a write root");
    assert!(programs.has(Program::Loginctl));
}

#[test]
fn a_dir_or_a_file_that_others_can_write_never_counts() {
    let (_temp, root) = root();
    let open_dir = root.join("open");
    fake(&open_dir, "crontab");
    std::fs::set_permissions(&open_dir, std::fs::Permissions::from_mode(0o777)).unwrap();
    let open_file = root.join("file");
    let program = fake(&open_file, "loginctl");
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o777)).unwrap();

    let path = std::env::join_paths([open_dir, open_file]).unwrap();
    let programs = TrustedPrograms::find(&path, &[], false);

    assert_eq!(programs, TrustedPrograms::none());
}

#[test]
fn a_link_or_a_relative_dir_never_counts() {
    let (_temp, root) = root();
    let real = fake(&root.join("real"), "crontab");
    let links = root.join("links");
    std::fs::create_dir_all(&links).unwrap();
    std::os::unix::fs::symlink(&real, links.join("crontab")).unwrap();

    let path = std::env::join_paths([links, PathBuf::from("real")]).unwrap();
    let programs = TrustedPrograms::find(&path, &[], false);

    assert!(!programs.has(Program::Crontab));
}

#[tokio::test]
async fn a_program_put_in_place_of_the_found_one_never_runs() {
    let (_temp, root) = root();
    let bin = root.join("bin");
    let program = fake(&bin, "crontab");
    let programs = TrustedPrograms::in_test_dir(&bin, &[]);
    assert!(programs.has(Program::Crontab));

    std::fs::remove_file(&program).unwrap();
    fake(&bin, "crontab");

    assert!(programs.command(Program::Crontab).is_none());
    assert!(!bin.join("ran").exists());
}

#[tokio::test]
async fn a_found_program_runs_with_a_clean_environment() {
    let (_temp, root) = root();
    let bin = root.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let program = bin.join("loginctl");
    std::fs::write(&program, "#!/bin/sh\nenv > \"$(dirname \"$0\")/env\"\n").unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    let programs = TrustedPrograms::in_test_dir(&bin, &[]);

    let status = programs.command(Program::Loginctl).unwrap().status().await.unwrap();

    assert!(status.success());
    let env = std::fs::read_to_string(bin.join("env")).unwrap();
    let mut names: Vec<&str> =
        env.lines().filter_map(|line| line.split_once('=')).map(|(name, _)| name).collect();
    names.retain(|name| !matches!(*name, "PWD" | "SHLVL" | "_"));
    names.sort_unstable();
    assert_eq!(names, ["LC_ALL", "PATH"]);
}

#[test]
fn xdg_dbus_proxy_is_found_and_checked_like_the_others() {
    let (_temp, root) = root();
    let bin = root.join("bin");
    let program = fake(&bin, "xdg-dbus-proxy");
    let programs = TrustedPrograms::in_test_dir(&bin, &[]);
    assert!(programs.has(Program::XdgDbusProxy));
    assert_eq!(programs.checked_path(Program::XdgDbusProxy), Some(program.clone()));
    assert!(programs.command(Program::XdgDbusProxy).is_some());

    // A file put in its place: the checked path and the command refuse it alike.
    std::fs::remove_file(&program).unwrap();
    fake(&bin, "xdg-dbus-proxy");
    assert_eq!(programs.checked_path(Program::XdgDbusProxy), None);
    assert!(programs.command(Program::XdgDbusProxy).is_none());
}

#[test]
fn the_installed_xdg_dbus_proxy_passes_the_real_rule() {
    use std::os::unix::fs::MetadataExt as _;
    // Check C4-5 of the phase E plan: a root-owned `/usr/bin/xdg-dbus-proxy` passes the
    // rule of efrd's own `PATH`. A machine without it has nothing to check.
    let installed = Path::new("/usr/bin/xdg-dbus-proxy");
    let Ok(meta) = std::fs::symlink_metadata(installed) else { return };
    if meta.uid() != 0 || !meta.is_file() {
        return;
    }
    let programs = TrustedPrograms::resolve(std::ffi::OsStr::new("/usr/bin"), &[]);
    assert_eq!(programs.checked_path(Program::XdgDbusProxy).as_deref(), Some(installed));
}

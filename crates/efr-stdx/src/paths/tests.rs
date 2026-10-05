use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use pretty_assertions::assert_eq;

use super::{Dirs, MAX_SOCKET_PATH, RootSource, RootSources, XdgBases, usable_run_user};
use crate::StdxError;
use crate::env::{Env, Var};

fn bases() -> Result<XdgBases, StdxError> {
    Ok(XdgBases {
        config: PathBuf::from("/home/u/.config"),
        data: PathBuf::from("/home/u/.local/share"),
        state: PathBuf::from("/home/u/.local/state"),
        runtime: Some(PathBuf::from("/run/user/1000")),
        run_user: None,
    })
}

fn bases_without_runtime() -> Result<XdgBases, StdxError> {
    Ok(XdgBases { runtime: None, ..bases()? })
}

fn bases_with_run_user() -> Result<XdgBases, StdxError> {
    Ok(XdgBases { runtime: None, run_user: Some(PathBuf::from("/run/user/1000")), ..bases()? })
}

fn no_bases() -> Result<XdgBases, StdxError> {
    panic!("the XDG lookup must not run when every root is replaced");
}

fn all_replaced() -> Env {
    Env::fixed([
        (Var::ConfigDir, "/t/config"),
        (Var::DataDir, "/t/data"),
        (Var::StateDir, "/t/state"),
        (Var::RuntimeDir, "/t/runtime"),
    ])
}

fn resolve(env: &Env, bases: fn() -> Result<XdgBases, StdxError>) -> Dirs {
    Dirs::resolve_in(env, bases).unwrap().0
}

fn sources(env: &Env, bases: fn() -> Result<XdgBases, StdxError>) -> RootSources {
    Dirs::resolve_in(env, bases).unwrap().1
}

#[test]
fn xdg_bases_get_the_efr_directory() {
    let env = Env::fixed::<_, &str>([]);
    let dirs = resolve(&env, bases);
    assert_eq!(dirs.config(), Path::new("/home/u/.config/efr"));
    assert_eq!(dirs.data(), Path::new("/home/u/.local/share/efr"));
    assert_eq!(dirs.state(), Path::new("/home/u/.local/state/efr"));
    assert_eq!(dirs.runtime(), Path::new("/run/user/1000/efr"));
    let xdg = RootSource::Xdg;
    assert_eq!(
        sources(&env, bases),
        RootSources { config: xdg, data: xdg, state: xdg, runtime: xdg }
    );
}

#[test]
fn replacements_are_used_as_is_without_an_xdg_lookup() {
    let (dirs, sources) = Dirs::resolve_in(&all_replaced(), no_bases).unwrap();
    assert_eq!(dirs, Dirs::new("/t/config", "/t/data", "/t/state", "/t/runtime").unwrap());
    assert_eq!(sources.data, RootSource::Variable(Var::DataDir));
    assert_eq!(sources.runtime, RootSource::Variable(Var::RuntimeDir));
}

#[test]
fn one_replacement_leaves_the_other_roots_on_xdg() {
    let env = Env::fixed([(Var::DataDir, "/t/data")]);
    let dirs = resolve(&env, bases);
    assert_eq!(dirs.data(), Path::new("/t/data"));
    assert_eq!(dirs.config(), Path::new("/home/u/.config/efr"));
    assert_eq!(dirs.runtime(), Path::new("/run/user/1000/efr"));
}

#[test]
fn efr_home_places_every_root_below_it_without_an_xdg_lookup() {
    let env = Env::fixed([(Var::Home, "/h/efr")]);
    let (dirs, sources) = Dirs::resolve_in(&env, no_bases).unwrap();
    assert_eq!(
        dirs,
        Dirs::new("/h/efr/config", "/h/efr/data", "/h/efr/state", "/h/efr/runtime").unwrap()
    );
    let home = RootSource::EfrHome;
    assert_eq!(sources, RootSources { config: home, data: home, state: home, runtime: home });
}

#[test]
fn a_roots_own_variable_wins_over_efr_home() {
    let env = Env::fixed([(Var::Home, "/h/efr"), (Var::RuntimeDir, "/r")]);
    let (dirs, sources) = Dirs::resolve_in(&env, no_bases).unwrap();
    assert_eq!(dirs.runtime(), Path::new("/r"));
    assert_eq!(dirs.data(), Path::new("/h/efr/data"));
    assert_eq!(sources.runtime, RootSource::Variable(Var::RuntimeDir));
    assert_eq!(sources.data, RootSource::EfrHome);
}

#[test]
fn an_empty_efr_home_counts_as_unset() {
    let env = Env::fixed([(Var::Home, "")]);
    assert_eq!(resolve(&env, bases).data(), Path::new("/home/u/.local/share/efr"));
}

#[test]
fn a_relative_efr_home_is_an_error() {
    let env = Env::fixed([(Var::Home, "efr")]);
    let err = Dirs::resolve_in(&env, bases).unwrap_err();
    assert!(matches!(err, StdxError::RelativeEnvPath { var: Var::Home, .. }), "{err:?}");
}

#[test]
fn empty_replacement_counts_as_unset() {
    let env = Env::fixed([(Var::StateDir, "")]);
    assert_eq!(resolve(&env, bases).state(), Path::new("/home/u/.local/state/efr"));
}

#[test]
fn missing_runtime_base_without_run_user_is_an_error() {
    let err = Dirs::resolve_in(&Env::fixed::<_, &str>([]), bases_without_runtime).unwrap_err();
    assert!(matches!(err, StdxError::RuntimeDirUnset), "{err:?}");
}

#[test]
fn a_missing_runtime_base_falls_back_to_run_user() {
    let env = Env::fixed::<_, &str>([]);
    assert_eq!(resolve(&env, bases_with_run_user).runtime(), Path::new("/run/user/1000/efr"));
    assert_eq!(sources(&env, bases_with_run_user).runtime, RootSource::RunUser);
}

#[test]
fn runtime_replacement_covers_a_missing_runtime_base() {
    let env = Env::fixed([(Var::RuntimeDir, "/t/runtime")]);
    assert_eq!(resolve(&env, bases_without_runtime).runtime(), Path::new("/t/runtime"));
}

#[test]
fn relative_replacement_is_an_error() {
    let env = Env::fixed([(Var::ConfigDir, "config")]);
    let err = Dirs::resolve_in(&env, bases).unwrap_err();
    assert!(matches!(err, StdxError::RelativeEnvPath { var: Var::ConfigDir, .. }), "{err:?}");
}

#[test]
fn failed_xdg_lookup_is_returned() {
    let env = Env::fixed([(Var::DataDir, "/t/data")]);
    let err = Dirs::resolve_in(&env, || Err(StdxError::HomeNotFound)).unwrap_err();
    assert!(matches!(err, StdxError::HomeNotFound), "{err:?}");
}

#[test]
fn daemon_files_live_in_their_roots() {
    let dirs = resolve(&all_replaced(), no_bases);
    assert_eq!(dirs.socket_path(), Path::new("/t/runtime/daemon.sock"));
    assert_eq!(dirs.daemon_json_path(), Path::new("/t/runtime/daemon.json"));
    assert_eq!(dirs.lock_path(), Path::new("/t/data/daemon.lock"));
}

#[test]
fn new_rejects_a_relative_root() {
    let err = Dirs::new("/c", "d", "/s", "/r").unwrap_err();
    match err {
        StdxError::NotAbsolute { path } => assert_eq!(path, PathBuf::from("d")),
        other => panic!("unexpected error {other:?}"),
    }
}

#[test]
fn a_socket_path_longer_than_a_socket_holds_is_refused() {
    let fits = format!("/{}", "r".repeat(MAX_SOCKET_PATH - "/daemon.sock".len() - 1));
    let dirs = Dirs::new("/c", "/d", "/s", &fits).unwrap();
    assert_eq!(dirs.checked_socket_path().unwrap().as_os_str().len(), MAX_SOCKET_PATH);
    let long = format!("{fits}x");
    let dirs = Dirs::new("/c", "/d", "/s", &long).unwrap();
    let err = dirs.checked_socket_path().unwrap_err();
    assert!(matches!(&err, StdxError::SocketPathTooLong { path } if path == &dirs.socket_path()));
    assert!(err.to_string().contains("108 bytes, more than the 107"), "{err}");
}

#[test]
fn run_user_must_be_the_users_own_private_directory() {
    let root = tempfile::tempdir().unwrap();
    let uid = rustix::process::getuid().as_raw();
    let dir = root.path().join("1000");
    assert!(!usable_run_user(&dir, uid), "a missing directory");
    std::fs::create_dir(&dir).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert!(usable_run_user(&dir, uid));
    assert!(!usable_run_user(&dir, uid.wrapping_add(1)), "another user's");
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(!usable_run_user(&dir, uid), "readable by others");
    let link = root.path().join("link");
    std::os::unix::fs::symlink(&dir, &link).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert!(!usable_run_user(&link, uid), "a symbolic link");
}

#[test]
fn sources_name_where_a_root_came_from() {
    assert_eq!(RootSource::Variable(Var::DataDir).to_string(), "EFR_DATA_DIR");
    assert_eq!(RootSource::EfrHome.to_string(), "EFR_HOME");
    assert_eq!(RootSource::Xdg.to_string(), "XDG");
    assert_eq!(RootSource::RunUser.to_string(), "/run/user");
}

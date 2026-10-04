use std::path::{Path, PathBuf};

use pretty_assertions::assert_eq;

use super::{Dirs, XdgBases};
use crate::StdxError;
use crate::env::{Env, Var};

fn bases() -> Result<XdgBases, StdxError> {
    Ok(XdgBases {
        config: PathBuf::from("/home/u/.config"),
        data: PathBuf::from("/home/u/.local/share"),
        state: PathBuf::from("/home/u/.local/state"),
        runtime: Some(PathBuf::from("/run/user/1000")),
    })
}

fn bases_without_runtime() -> Result<XdgBases, StdxError> {
    Ok(XdgBases { runtime: None, ..bases()? })
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

#[test]
fn xdg_bases_get_the_efr_directory() {
    let dirs = Dirs::resolve_in(&Env::fixed::<_, &str>([]), bases).unwrap();
    assert_eq!(dirs.config(), Path::new("/home/u/.config/efr"));
    assert_eq!(dirs.data(), Path::new("/home/u/.local/share/efr"));
    assert_eq!(dirs.state(), Path::new("/home/u/.local/state/efr"));
    assert_eq!(dirs.runtime(), Path::new("/run/user/1000/efr"));
}

#[test]
fn replacements_are_used_as_is_without_an_xdg_lookup() {
    let dirs = Dirs::resolve_in(&all_replaced(), no_bases).unwrap();
    assert_eq!(dirs, Dirs::new("/t/config", "/t/data", "/t/state", "/t/runtime").unwrap());
}

#[test]
fn one_replacement_leaves_the_other_roots_on_xdg() {
    let env = Env::fixed([(Var::DataDir, "/t/data")]);
    let dirs = Dirs::resolve_in(&env, bases).unwrap();
    assert_eq!(dirs.data(), Path::new("/t/data"));
    assert_eq!(dirs.config(), Path::new("/home/u/.config/efr"));
    assert_eq!(dirs.runtime(), Path::new("/run/user/1000/efr"));
}

#[test]
fn empty_replacement_counts_as_unset() {
    let env = Env::fixed([(Var::StateDir, "")]);
    let dirs = Dirs::resolve_in(&env, bases).unwrap();
    assert_eq!(dirs.state(), Path::new("/home/u/.local/state/efr"));
}

#[test]
fn missing_runtime_base_is_an_error() {
    let err = Dirs::resolve_in(&Env::fixed::<_, &str>([]), bases_without_runtime).unwrap_err();
    assert!(matches!(err, StdxError::RuntimeDirUnset), "{err:?}");
}

#[test]
fn runtime_replacement_covers_a_missing_runtime_base() {
    let env = Env::fixed([(Var::RuntimeDir, "/t/runtime")]);
    let dirs = Dirs::resolve_in(&env, bases_without_runtime).unwrap();
    assert_eq!(dirs.runtime(), Path::new("/t/runtime"));
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
    let dirs = Dirs::resolve_in(&all_replaced(), no_bases).unwrap();
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

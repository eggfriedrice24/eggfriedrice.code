use std::fs;
use std::os::unix::fs::{PermissionsExt as _, symlink};
use std::path::Path;

use efr_stdx::paths::Dirs;
use pretty_assertions::assert_eq;
use secrecy::{ExposeSecret as _, SecretString};

use super::{FileStore, MAX_RECORD_BYTES};
use crate::{CredentialId, CredentialRecord, CredentialsError, OAuthTokens, SecretStore};

fn id(name: &str) -> CredentialId {
    CredentialId::new(name).unwrap()
}

fn api_key(key: &str) -> CredentialRecord {
    CredentialRecord::ApiKey { key: SecretString::from(key) }
}

fn mode(path: &Path) -> u32 {
    fs::symlink_metadata(path).unwrap().permissions().mode() & 0o777
}

fn chmod(path: &Path, mode: u32) {
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}

fn key_of(record: Option<CredentialRecord>) -> String {
    match record {
        Some(CredentialRecord::ApiKey { key }) => key.expose_secret().to_owned(),
        other => panic!("unexpected record {other:?}"),
    }
}

/// A store in a fresh temporary directory whose secrets directory does not exist yet.
fn store() -> (tempfile::TempDir, FileStore) {
    let root = tempfile::tempdir().unwrap();
    let store = FileStore::new(root.path().join("data/secrets"));
    (root, store)
}

#[test]
fn standard_place_is_under_the_data_root() {
    let dirs = Dirs::new("/c", "/d/efr", "/s", "/r").unwrap();
    let store = FileStore::in_data_dir(&dirs);
    assert_eq!(store.dir(), Path::new("/d/efr/secrets"));
    assert_eq!(store.path_of(&id("openai-api")), Path::new("/d/efr/secrets/openai-api.json"));
}

#[test]
fn load_without_a_directory_finds_nothing() {
    let (_root, store) = store();
    assert!(store.load(&id("openai-api")).unwrap().is_none());
    assert!(!store.dir().exists(), "a load must not create the directory");
}

#[test]
fn save_creates_a_private_directory_and_file() {
    let (root, store) = store();
    store.save(&id("openai-api"), &api_key("sk-1")).unwrap();
    assert_eq!(mode(store.dir()), 0o700);
    assert_eq!(mode(&root.path().join("data")), 0o700);
    assert_eq!(mode(&store.path_of(&id("openai-api"))), 0o600);
}

#[test]
fn save_then_load_round_trips() {
    let (_root, store) = store();
    let mut tokens = OAuthTokens::new(SecretString::from("access"));
    tokens.refresh_token = Some(SecretString::from("refresh"));
    store.save(&id("openai-subscription"), &CredentialRecord::OAuth(tokens)).unwrap();
    match store.load(&id("openai-subscription")).unwrap() {
        Some(CredentialRecord::OAuth(tokens)) => {
            assert_eq!(tokens.access_token.expose_secret(), "access");
            assert_eq!(tokens.refresh_token.unwrap().expose_secret(), "refresh");
        }
        other => panic!("unexpected record {other:?}"),
    }
}

#[test]
fn save_replaces_the_earlier_record() {
    let (_root, store) = store();
    store.save(&id("openai-api"), &api_key("first")).unwrap();
    store.save(&id("openai-api"), &api_key("second")).unwrap();
    assert_eq!(key_of(store.load(&id("openai-api")).unwrap()), "second");
    assert_eq!(store.list().unwrap(), [id("openai-api")]);
}

#[test]
fn records_are_kept_apart_by_id() {
    let (_root, store) = store();
    store.save(&id("openai-api"), &api_key("api")).unwrap();
    store.save(&id("mcp.github"), &api_key("github")).unwrap();
    assert_eq!(key_of(store.load(&id("openai-api")).unwrap()), "api");
    assert_eq!(key_of(store.load(&id("mcp.github")).unwrap()), "github");
}

#[test]
fn delete_reports_whether_a_record_existed() {
    let (_root, store) = store();
    assert!(!store.delete(&id("openai-api")).unwrap(), "no directory yet");
    store.save(&id("openai-api"), &api_key("sk")).unwrap();
    assert!(store.delete(&id("openai-api")).unwrap());
    assert!(!store.delete(&id("openai-api")).unwrap());
    assert!(store.load(&id("openai-api")).unwrap().is_none());
}

#[test]
fn list_returns_sorted_ids_and_skips_other_entries() {
    let (_root, store) = store();
    store.save(&id("openai-subscription"), &api_key("b")).unwrap();
    store.save(&id("openai-api"), &api_key("a")).unwrap();
    let dir = store.dir();
    fs::write(dir.join(".openai-api.json.1.2.tmp"), b"leftover").unwrap();
    fs::write(dir.join("notes.txt"), b"x").unwrap();
    fs::write(dir.join("Upper.json"), b"{}").unwrap();
    fs::create_dir(dir.join("nested.json")).unwrap();
    assert_eq!(store.list().unwrap(), [id("openai-api"), id("openai-subscription")]);
}

#[test]
fn list_without_a_directory_is_empty() {
    let (_root, store) = store();
    assert_eq!(store.list().unwrap(), Vec::<CredentialId>::new());
}

#[test]
fn load_refuses_a_file_that_others_can_read() {
    let (_root, store) = store();
    store.save(&id("openai-api"), &api_key("sk")).unwrap();
    let path = store.path_of(&id("openai-api"));
    chmod(&path, 0o644);
    match store.load(&id("openai-api")) {
        Err(CredentialsError::InsecurePermissions { path: reported, mode }) => {
            assert_eq!(reported, path);
            assert_eq!(mode, 0o644);
        }
        other => panic!("unexpected result {other:?}"),
    }
    assert_eq!(mode(&path), 0o644, "the store must not repair the mode itself");
}

#[test]
fn load_refuses_a_directory_that_others_can_enter() {
    let (_root, store) = store();
    store.save(&id("openai-api"), &api_key("sk")).unwrap();
    chmod(store.dir(), 0o755);
    let result = store.load(&id("openai-api"));
    assert!(
        matches!(result, Err(CredentialsError::InsecurePermissions { mode: 0o755, .. })),
        "{result:?}"
    );
}

#[test]
fn save_refuses_a_directory_that_others_can_enter() {
    let (_root, store) = store();
    fs::create_dir_all(store.dir()).unwrap();
    chmod(store.dir(), 0o750);
    let result = store.save(&id("openai-api"), &api_key("sk"));
    assert!(matches!(result, Err(CredentialsError::InsecurePermissions { .. })), "{result:?}");
    assert!(!store.path_of(&id("openai-api")).exists());
}

#[test]
fn load_refuses_a_symbolic_link() {
    let (root, store) = store();
    store.save(&id("openai-api"), &api_key("sk")).unwrap();
    let elsewhere = root.path().join("elsewhere.json");
    fs::rename(store.path_of(&id("openai-api")), &elsewhere).unwrap();
    symlink(&elsewhere, store.path_of(&id("openai-api"))).unwrap();
    let result = store.load(&id("openai-api"));
    assert!(
        matches!(result, Err(CredentialsError::UnexpectedFileType { expected: "file", .. })),
        "{result:?}"
    );
}

#[test]
fn a_symbolic_link_as_the_directory_is_refused() {
    let (root, store) = store();
    let real = root.path().join("real");
    fs::create_dir(&real).unwrap();
    chmod(&real, 0o700);
    fs::create_dir(root.path().join("data")).unwrap();
    symlink(&real, store.dir()).unwrap();
    let result = store.save(&id("openai-api"), &api_key("sk"));
    assert!(
        matches!(result, Err(CredentialsError::UnexpectedFileType { expected: "directory", .. })),
        "{result:?}"
    );
}

#[test]
fn load_refuses_an_oversized_file() {
    let (_root, store) = store();
    store.save(&id("openai-api"), &api_key("sk")).unwrap();
    let path = store.path_of(&id("openai-api"));
    let size = usize::try_from(MAX_RECORD_BYTES).unwrap() + 1;
    fs::write(&path, vec![b' '; size]).unwrap();
    let result = store.load(&id("openai-api"));
    assert!(matches!(result, Err(CredentialsError::TooLarge { .. })), "{result:?}");
}

#[test]
fn load_reports_a_corrupt_record_by_id() {
    let (_root, store) = store();
    store.save(&id("openai-api"), &api_key("sk")).unwrap();
    fs::write(store.path_of(&id("openai-api")), b"{\"version\":1,").unwrap();
    match store.load(&id("openai-api")) {
        Err(CredentialsError::Decode { id: failed, .. }) => assert_eq!(failed, id("openai-api")),
        other => panic!("unexpected result {other:?}"),
    }
}

#[test]
fn works_through_a_trait_object() {
    let (_root, store) = store();
    let store: Box<dyn SecretStore> = Box::new(store);
    store.save(&id("openai-api"), &api_key("dyn")).unwrap();
    assert_eq!(key_of(store.load(&id("openai-api")).unwrap()), "dyn");
}

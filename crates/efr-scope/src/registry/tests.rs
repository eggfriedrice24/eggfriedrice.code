use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt as _;
use std::path::{Path, PathBuf};

use efr_protocol::ProjectId;
use pretty_assertions::assert_eq;
use rstest::rstest;

use super::{REGISTRY_FILE, Registry, RegistryProblem};
use crate::ScopeError;

const FILE: &str = "/home/u/.config/efr/projects.toml";

fn id(n: u8) -> ProjectId {
    format!("0192f0c1-7a00-7000-8000-0000000000{n:02}").parse().unwrap()
}

fn parse(text: &str) -> Result<Registry, ScopeError> {
    Registry::from_toml(text, Path::new(FILE))
}

fn invalid(text: &str) -> RegistryProblem {
    match parse(text) {
        Err(ScopeError::InvalidRegistry { path, problem }) => {
            assert_eq!(path, Path::new(FILE));
            problem
        }
        other => panic!("expected an invalid registry, got {other:?}"),
    }
}

#[test]
fn the_file_lives_in_the_config_dir() {
    assert_eq!(REGISTRY_FILE, "projects.toml");
    assert_eq!(Registry::path_in(Path::new("/home/u/.config/efr")), Path::new(FILE));
}

#[test]
fn reads_projects_in_file_order() {
    let registry = parse(
        r#"
# Projects that efr treats as projects.
[[project]]
id = "0192f0c1-7a00-7000-8000-000000000002"
root = "/home/u/p/app/"
name = "app"

[[project]]
id = "0192f0c1-7a00-7000-8000-000000000001"
root = "/etc/nixos"
"#,
    )
    .unwrap();
    let projects: Vec<_> =
        registry.projects().iter().map(|p| (p.id(), p.root().to_owned(), p.name())).collect();
    assert_eq!(
        projects,
        [
            (id(2), PathBuf::from("/home/u/p/app"), Some("app")),
            (id(1), PathBuf::from("/etc/nixos"), None),
        ]
    );
    assert_eq!(registry.get(&id(1)).map(|p| p.root()), Some(Path::new("/etc/nixos")));
    assert!(registry.get(&id(3)).is_none());
}

#[rstest]
#[case::empty("")]
#[case::comment_only("# nothing registered yet\n")]
fn an_empty_file_is_an_empty_registry(#[case] text: &str) {
    assert!(parse(text).unwrap().is_empty());
}

#[test]
fn invalid_files_are_refused() {
    assert_eq!(
        invalid("[[project]]\nid = \"0192f0c1-7a00-7000-8000-000000000001\"\nroot = \"p/app\"\n"),
        RegistryProblem::RootNotAbsolute { root: "p/app".into() }
    );
    assert_eq!(
        invalid(
            "[[project]]\nid = \"0192f0c1-7a00-7000-8000-000000000001\"\nroot = \"/a\"\n\
             [[project]]\nid = \"0192f0c1-7a00-7000-8000-000000000001\"\nroot = \"/b\"\n"
        ),
        RegistryProblem::DuplicateId { id: id(1) }
    );
    assert_eq!(
        invalid(
            "[[project]]\nid = \"0192f0c1-7a00-7000-8000-000000000001\"\nroot = \"/a/b/\"\n\
             [[project]]\nid = \"0192f0c1-7a00-7000-8000-000000000002\"\nroot = \"/a/./b\"\n"
        ),
        RegistryProblem::DuplicateRoot { root: "/a/b".into() }
    );
}

#[rstest]
#[case::unknown_key(
    "[[project]]\nid = \"0192f0c1-7a00-7000-8000-000000000001\"\nroot = \"/a\"\npath = \"/b\"\n"
)]
#[case::unknown_table("[settings]\nauto = true\n")]
#[case::bad_id("[[project]]\nid = \"app\"\nroot = \"/a\"\n")]
#[case::missing_root("[[project]]\nid = \"0192f0c1-7a00-7000-8000-000000000001\"\n")]
#[case::not_toml("[[project]\n")]
fn malformed_files_do_not_parse(#[case] text: &str) {
    assert!(
        matches!(parse(text), Err(ScopeError::ParseRegistry { path, .. }) if path == Path::new(FILE))
    );
}

#[test]
fn messages_name_the_file_and_the_problem() {
    let error = parse("[[project]]\nid = \"0192f0c1-7a00-7000-8000-000000000001\"\nroot = \"x\"\n")
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        format!("the project registry {FILE} is invalid: the root x is not an absolute path")
    );
}

fn registry() -> Registry {
    let mut registry = Registry::empty();
    registry.register(id(1), "/home/u/p", None).unwrap();
    registry.register(id(2), "/home/u/p/app", Some("app".into())).unwrap();
    registry.register(id(3), "/etc/nixos", None).unwrap();
    registry
}

#[rstest]
#[case::root_itself("/home/u/p/app", Some(2))]
#[case::deepest_wins("/home/u/p/app/src/bin", Some(2))]
#[case::outer_project("/home/u/p/other", Some(1))]
#[case::sibling_prefix("/home/u/p/application", Some(1))]
#[case::outside("/home/u/Documents", None)]
#[case::above("/home/u", None)]
#[case::dots("/home/u/p/app/../other", Some(1))]
#[case::system("/etc/nixos/modules", Some(3))]
#[case::relative("p/app", None)]
fn finds_the_project_that_holds_a_path(#[case] path: &str, #[case] expected: Option<u8>) {
    let registry = registry();
    assert_eq!(registry.containing(Path::new(path)).map(|p| p.id()), expected.map(id));
}

#[test]
fn register_refuses_what_the_file_would_refuse() {
    let mut registry = registry();
    let problem = |result: Result<_, ScopeError>| match result {
        Err(ScopeError::InvalidProject { problem }) => problem,
        other => panic!("expected an invalid project, got {other:?}"),
    };
    assert_eq!(
        problem(registry.register(id(1), "/srv", None).map(|_| ())),
        RegistryProblem::DuplicateId { id: id(1) }
    );
    assert_eq!(
        problem(registry.register(id(9), "/home/u/p/app/", None).map(|_| ())),
        RegistryProblem::DuplicateRoot { root: "/home/u/p/app".into() }
    );
    assert_eq!(
        problem(registry.register(id(9), "srv", None).map(|_| ())),
        RegistryProblem::RootNotAbsolute { root: "srv".into() }
    );
    let not_unicode = PathBuf::from(OsString::from_vec(b"/srv/\xff".to_vec()));
    assert_eq!(
        problem(registry.register(id(9), not_unicode.clone(), None).map(|_| ())),
        RegistryProblem::RootNotUnicode { root: not_unicode }
    );
    assert_eq!(registry.projects().len(), 3);
}

#[test]
fn home_and_root_can_be_registered_explicitly() {
    let mut registry = Registry::empty();
    registry.register(id(1), "/home/u", None).unwrap();
    registry.register(id(2), "/", None).unwrap();
    assert_eq!(registry.containing(Path::new("/home/u/notes")).map(|p| p.id()), Some(id(1)));
    assert_eq!(registry.containing(Path::new("/etc")).map(|p| p.id()), Some(id(2)));
}

#[test]
fn remove_takes_a_project_out() {
    let mut registry = registry();
    assert_eq!(registry.remove(&id(2)).map(|p| p.id()), Some(id(2)));
    assert_eq!(registry.remove(&id(2)), None);
    assert_eq!(registry.containing(Path::new("/home/u/p/app")).map(|p| p.id()), Some(id(1)));
}

#[test]
fn writes_a_stable_file() {
    assert_eq!(
        registry().to_toml().unwrap(),
        r#"[[project]]
id = "0192f0c1-7a00-7000-8000-000000000001"
root = "/home/u/p"

[[project]]
id = "0192f0c1-7a00-7000-8000-000000000002"
root = "/home/u/p/app"
name = "app"

[[project]]
id = "0192f0c1-7a00-7000-8000-000000000003"
root = "/etc/nixos"
"#
    );
}

#[test]
fn save_then_load_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config/efr").join(REGISTRY_FILE);
    registry().save(&path).unwrap();
    assert_eq!(Registry::load(&path).unwrap(), registry());

    Registry::empty().save(&path).unwrap();
    assert!(Registry::load(&path).unwrap().is_empty());
}

#[test]
fn a_missing_file_is_an_empty_registry() {
    let dir = tempfile::tempdir().unwrap();
    assert!(Registry::load(&dir.path().join(REGISTRY_FILE)).unwrap().is_empty());
}

#[test]
fn an_unreadable_file_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(REGISTRY_FILE);
    std::fs::create_dir(&path).unwrap();
    assert!(matches!(Registry::load(&path), Err(ScopeError::ReadRegistry { .. })));
}

#[test]
fn load_names_the_file_in_parse_errors() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(REGISTRY_FILE);
    std::fs::write(&path, "[[project]]\nid = 1\n").unwrap();
    assert!(
        matches!(Registry::load(&path), Err(ScopeError::ParseRegistry { path: p, .. }) if p == path)
    );
}

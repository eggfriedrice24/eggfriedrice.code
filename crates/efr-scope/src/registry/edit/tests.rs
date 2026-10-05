use std::path::{Path, PathBuf};

use efr_protocol::ProjectId;
use pretty_assertions::assert_eq;

use super::RegistryEdit;
use crate::{Registry, RegistryProblem, ScopeError};

fn id(n: u8) -> ProjectId {
    format!("0192f0c1-7a00-7000-8000-0000000000{n:02}").parse().unwrap()
}

const COMMENTED: &str = r#"# My projects.

[[project]]
# The harness itself.
id = "0192f0c1-7a00-7000-8000-000000000001"
root = "/home/u/p/efr"   # keep this one
name = "efr"

# Infrastructure.
[[project]]
id = "0192f0c1-7a00-7000-8000-000000000002"
root = "/etc/nixos"
"#;

fn file(dir: &Path, text: &str) -> PathBuf {
    let path = dir.join("projects.toml");
    std::fs::write(&path, text).unwrap();
    path
}

fn roots(path: &Path) -> Vec<PathBuf> {
    Registry::load(path).unwrap().projects().iter().map(|p| p.root().to_owned()).collect()
}

#[test]
fn a_new_project_goes_after_the_others_and_every_comment_stays() {
    let dir = tempfile::tempdir().unwrap();
    let path = file(dir.path(), COMMENTED);

    let mut edit = RegistryEdit::open(&path).unwrap();
    let project = edit.register(id(3), "/home/u/p/app/", Some("app".to_owned())).unwrap();
    edit.save().unwrap();

    assert_eq!(project.root(), Path::new("/home/u/p/app"));
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        format!(
            "{COMMENTED}\n[[project]]\nid = \"0192f0c1-7a00-7000-8000-000000000003\"\nroot = \"/home/u/p/app\"\nname = \"app\"\n"
        )
    );
    assert_eq!(
        roots(&path),
        [PathBuf::from("/home/u/p/efr"), "/etc/nixos".into(), "/home/u/p/app".into()]
    );
}

#[test]
fn removing_a_project_keeps_the_comments_of_the_others() {
    let dir = tempfile::tempdir().unwrap();
    let path = file(dir.path(), COMMENTED);

    let mut edit = RegistryEdit::open(&path).unwrap();
    let removed = edit.remove_root(Path::new("/etc/nixos/")).unwrap().unwrap();
    edit.save().unwrap();

    assert_eq!(removed.id(), id(2));
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.starts_with("# My projects.\n\n[[project]]\n# The harness itself.\n"), "{text}");
    assert!(text.contains("root = \"/home/u/p/efr\"   # keep this one\n"), "{text}");
    assert!(!text.contains("nixos"), "{text}");
    assert_eq!(roots(&path), [PathBuf::from("/home/u/p/efr")]);
}

#[test]
fn removing_a_root_that_is_not_registered_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let path = file(dir.path(), COMMENTED);
    let mut edit = RegistryEdit::open(&path).unwrap();
    assert_eq!(edit.remove_root(Path::new("/home/u/p")).unwrap(), None);
    assert_eq!(edit.text(), COMMENTED);
    assert_eq!(edit.registry().projects().len(), 2);
}

#[test]
fn a_missing_file_starts_with_a_comment_that_says_what_it_is() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("efr").join("projects.toml");

    let mut edit = RegistryEdit::open(&path).unwrap();
    edit.register(id(1), "/home/u/p/app", None).unwrap();
    edit.save().unwrap();

    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.starts_with("# The projects that efr knows."), "{text}");
    assert!(
        text.ends_with(
            "\n[[project]]\nid = \"0192f0c1-7a00-7000-8000-000000000001\"\nroot = \"/home/u/p/app\"\n"
        ),
        "{text}"
    );
    assert_eq!(roots(&path), [PathBuf::from("/home/u/p/app")]);
}

#[test]
fn a_file_of_comments_keeps_them_before_the_first_project() {
    let dir = tempfile::tempdir().unwrap();
    let path = file(dir.path(), "# Nothing yet.\n");

    let mut edit = RegistryEdit::open(&path).unwrap();
    edit.register(id(1), "/srv/site", None).unwrap();
    edit.save().unwrap();

    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.starts_with("# Nothing yet.\n"), "{text}");
    assert_eq!(roots(&path), [PathBuf::from("/srv/site")]);
}

#[test]
fn an_inline_list_of_projects_takes_one_more() {
    let dir = tempfile::tempdir().unwrap();
    let text =
        "project = [\n  { id = \"0192f0c1-7a00-7000-8000-000000000001\", root = \"/a\" },\n]\n";
    let path = file(dir.path(), text);

    let mut edit = RegistryEdit::open(&path).unwrap();
    edit.register(id(2), "/b", Some("b".to_owned())).unwrap();
    edit.save().unwrap();
    assert_eq!(roots(&path), [PathBuf::from("/a"), "/b".into()]);

    let mut edit = RegistryEdit::open(&path).unwrap();
    edit.remove_root(Path::new("/a")).unwrap().unwrap();
    edit.save().unwrap();
    assert_eq!(roots(&path), [PathBuf::from("/b")]);
}

#[test]
fn a_root_or_an_id_that_is_registered_already_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = file(dir.path(), COMMENTED);
    let mut edit = RegistryEdit::open(&path).unwrap();

    let twice = edit.register(id(3), "/etc/nixos", None).unwrap_err();
    assert!(
        matches!(
            twice,
            ScopeError::InvalidProject { problem: RegistryProblem::DuplicateRoot { .. } }
        ),
        "{twice:?}"
    );
    assert_eq!(edit.text(), COMMENTED, "a refused project leaves the file alone");
    let relative = edit.register(id(3), "p/app", None).unwrap_err();
    assert!(
        matches!(
            relative,
            ScopeError::InvalidProject { problem: RegistryProblem::RootNotAbsolute { .. } }
        ),
        "{relative:?}"
    );
}

#[test]
fn a_file_with_an_error_is_never_changed() {
    let dir = tempfile::tempdir().unwrap();
    let text =
        "[[project]]\nid = \"0192f0c1-7a00-7000-8000-000000000001\"\nroot = \"/a\"\nnmae = \"a\"\n";
    let path = file(dir.path(), text);
    let error = RegistryEdit::open(&path).unwrap_err();
    assert!(matches!(error, ScopeError::ParseRegistry { .. }), "{error:?}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
}

#[test]
fn a_change_goes_to_the_file_behind_a_link_and_the_link_stays() {
    let dir = tempfile::tempdir().unwrap();
    let dotfiles = dir.path().join("dotfiles");
    std::fs::create_dir(&dotfiles).unwrap();
    let real = file(&dotfiles, COMMENTED);
    let config = dir.path().join("config");
    std::fs::create_dir(&config).unwrap();
    let path = config.join("projects.toml");
    std::os::unix::fs::symlink(&real, &path).unwrap();

    let mut edit = RegistryEdit::open(&path).unwrap();
    assert_eq!(edit.target(), std::fs::canonicalize(&real).unwrap());
    edit.register(id(3), "/srv/site", None).unwrap();
    edit.save().unwrap();

    assert!(std::fs::symlink_metadata(&path).unwrap().file_type().is_symlink());
    assert_eq!(roots(&real).len(), 3);
}

#[test]
fn a_link_to_nothing_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("projects.toml");
    std::os::unix::fs::symlink(dir.path().join("gone.toml"), &path).unwrap();
    let error = RegistryEdit::open(&path).unwrap_err();
    assert!(matches!(error, ScopeError::DanglingRegistryLink { .. }), "{error:?}");
    assert!(!dir.path().join("gone.toml").exists());
}

#[test]
fn a_file_that_changed_since_the_read_is_not_written() {
    let dir = tempfile::tempdir().unwrap();
    let path = file(dir.path(), COMMENTED);
    let mut edit = RegistryEdit::open(&path).unwrap();
    edit.register(id(3), "/srv/site", None).unwrap();

    std::fs::write(&path, "# rewritten by hand\n").unwrap();
    let error = edit.save().unwrap_err();

    assert!(matches!(error, ScopeError::RegistryChanged { .. }), "{error:?}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "# rewritten by hand\n");
}

#[test]
fn a_file_that_appeared_since_the_read_is_not_replaced() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("projects.toml");
    let mut edit = RegistryEdit::open(&path).unwrap();
    edit.register(id(1), "/srv/site", None).unwrap();

    std::fs::write(&path, "# mine\n").unwrap();
    let error = edit.save().unwrap_err();

    assert!(matches!(error, ScopeError::RegistryChanged { .. }), "{error:?}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "# mine\n");
}

use std::path::{Path, PathBuf};

use efr_protocol::ProjectId;
use pretty_assertions::assert_eq;
use proptest::prelude::*;
use rstest::rstest;

use super::{Locations, PathClass, normalize};
use crate::PermissionsError;

const HOME: &str = "/home/u";
const SCRATCH: &str = "/home/u/.local/share/efr/scratch/2026-10-04-fix-dns-0a1b2c3d";

fn locations() -> Locations {
    Locations::new(HOME)
        .unwrap()
        .with_secret_root("/home/u/.local/share/efr/secrets")
        .unwrap()
        .with_user_config_root("/srv/dotfiles")
        .unwrap()
}

#[rstest]
#[case::scratch_dir(SCRATCH, PathClass::Scratch)]
#[case::scratch_file(
    "/home/u/.local/share/efr/scratch/2026-10-04-fix-dns-0a1b2c3d/a.txt",
    PathClass::Scratch
)]
#[case::other_scratch(
    "/home/u/.local/share/efr/scratch/2026-10-03-other-99999999/a",
    PathClass::UserData
)]
#[case::config_dir("/home/u/.config/nvim/init.lua", PathClass::UserConfig)]
#[case::rc_file("/home/u/.zshrc", PathClass::UserConfig)]
#[case::local_bin("/home/u/.local/bin/tool", PathClass::UserConfig)]
#[case::dotfiles_root("/srv/dotfiles/zsh/.zshrc", PathClass::UserConfig)]
#[case::documents("/home/u/Documents/plan.md", PathClass::UserData)]
#[case::projects("/home/u/p/app/src/main.rs", PathClass::UserData)]
#[case::home_itself("/home/u", PathClass::UserData)]
#[case::local_share("/home/u/.local/share/fonts/a.ttf", PathClass::UserData)]
#[case::local_state("/home/u/.local/state/efr/logs/a.json", PathClass::UserData)]
#[case::cache("/home/u/.cache/pip/x", PathClass::UserData)]
#[case::efr_database("/home/u/.local/share/efr/efr.sqlite", PathClass::UserData)]
#[case::etc("/etc/hosts", PathClass::System)]
#[case::usr("/usr/bin/zsh", PathClass::System)]
#[case::root("/", PathClass::System)]
#[case::other_home("/home/u2/.zshrc", PathClass::System)]
#[case::parent_of_home("/home", PathClass::System)]
#[case::ssh_dir("/home/u/.ssh", PathClass::Secrets)]
#[case::ssh_key("/home/u/.ssh/id_ed25519", PathClass::Secrets)]
#[case::gnupg("/home/u/.gnupg/pubring.kbx", PathClass::Secrets)]
#[case::pass("/home/u/.password-store/mail.gpg", PathClass::Secrets)]
#[case::keyrings("/home/u/.local/share/keyrings/login.keyring", PathClass::Secrets)]
#[case::netrc("/home/u/.netrc", PathClass::Secrets)]
#[case::daemon_secrets(
    "/home/u/.local/share/efr/secrets/openai-subscription.json",
    PathClass::Secrets
)]
#[case::shadow("/etc/shadow", PathClass::Secrets)]
#[case::gshadow("/etc/gshadow", PathClass::Secrets)]
#[case::similar_name("/home/u/.sshx/config", PathClass::UserConfig)]
#[case::similar_system_name("/etc/shadowsocks/config.json", PathClass::System)]
fn classifies(#[case] path: &str, #[case] expected: PathClass) {
    assert_eq!(locations().classify(Path::new(path), Path::new(SCRATCH)), Some(expected));
}

#[rstest]
#[case::dot_segments("/home/u/./.config/../.ssh/id_ed25519", PathClass::Secrets)]
#[case::scratch_escape(
    "/home/u/.local/share/efr/scratch/2026-10-04-fix-dns-0a1b2c3d/../../secrets/x.json",
    PathClass::Secrets
)]
#[case::far_escape(
    "/home/u/.local/share/efr/scratch/2026-10-04-fix-dns-0a1b2c3d/../../../../../../etc/hosts",
    PathClass::System
)]
#[case::double_slash("/home/u//.ssh//id_ed25519", PathClass::Secrets)]
#[case::trailing_slash("/home/u/.ssh/", PathClass::Secrets)]
fn classifies_after_resolving_dots(#[case] path: &str, #[case] expected: PathClass) {
    assert_eq!(locations().classify(Path::new(path), Path::new(SCRATCH)), Some(expected));
}

#[test]
fn relative_paths_have_no_class() {
    let locations = locations();
    assert_eq!(locations.classify(Path::new("notes.txt"), Path::new(SCRATCH)), None);
    assert_eq!(locations.classify(Path::new("../.ssh/id_ed25519"), Path::new(SCRATCH)), None);
    assert_eq!(locations.classify(Path::new(""), Path::new(SCRATCH)), None);
}

#[rstest]
#[case::home(HOME)]
#[case::root("/")]
#[case::above_home("/home")]
#[case::relative("scratch")]
fn scratch_that_covers_home_makes_nothing_scratch(#[case] scratch: &str) {
    let locations = locations();
    assert_eq!(
        locations.classify(Path::new("/home/u/notes.txt"), Path::new(scratch)),
        Some(PathClass::UserData)
    );
    assert_eq!(
        locations.classify(Path::new("/etc/hosts"), Path::new(scratch)),
        Some(PathClass::System)
    );
}

#[test]
fn scratch_outside_home_is_scratch() {
    let scratch = Path::new("/tmp/efr-data/scratch/2026-10-04-a-00000000");
    assert_eq!(locations().classify(&scratch.join("out.txt"), scratch), Some(PathClass::Scratch));
}

#[test]
fn secrets_win_over_scratch() {
    let scratch = Path::new("/home/u/.ssh/scratch");
    assert_eq!(locations().classify(&scratch.join("a"), scratch), Some(PathClass::Secrets));
}

#[test]
fn locations_need_an_absolute_home_that_is_not_root() {
    assert_eq!(
        Locations::new("home/u").unwrap_err(),
        PermissionsError::NotAbsolute { path: PathBuf::from("home/u") }
    );
    assert_eq!(Locations::new("/").unwrap_err(), PermissionsError::HomeIsRoot);
    assert_eq!(Locations::new("/home/..").unwrap_err(), PermissionsError::HomeIsRoot);
    assert_eq!(Locations::new("/home/u/").unwrap().home(), Path::new(HOME));
}

#[test]
fn extra_locations_must_be_absolute() {
    let home = || Locations::new(HOME).unwrap();
    assert!(matches!(home().with_secret_root("x"), Err(PermissionsError::NotAbsolute { .. })));
    assert!(matches!(home().with_user_config_root("x"), Err(PermissionsError::NotAbsolute { .. })));
    let id: ProjectId = "0192f0c1-7a00-7000-8000-000000000001".parse().unwrap();
    assert!(matches!(home().with_project(id, "p/app"), Err(PermissionsError::NotAbsolute { .. })));
}

#[test]
fn project_roots_are_kept_but_never_widen_from_home_or_above() {
    let app: ProjectId = "0192f0c1-7a00-7000-8000-000000000001".parse().unwrap();
    let home: ProjectId = "0192f0c1-7a00-7000-8000-000000000002".parse().unwrap();
    let root: ProjectId = "0192f0c1-7a00-7000-8000-000000000003".parse().unwrap();
    let above: ProjectId = "0192f0c1-7a00-7000-8000-000000000004".parse().unwrap();
    let elsewhere: ProjectId = "0192f0c1-7a00-7000-8000-000000000005".parse().unwrap();
    let locations = Locations::new(HOME)
        .unwrap()
        .with_project(app, "/home/u/p/app/")
        .unwrap()
        .with_project(home, "/home/u")
        .unwrap()
        .with_project(root, "/")
        .unwrap()
        .with_project(above, "/home")
        .unwrap()
        .with_project(elsewhere, "/srv/site")
        .unwrap();
    assert_eq!(locations.project_root(&home), Some(Path::new(HOME)));
    assert_eq!(locations.widening_project_root(&app), Some(Path::new("/home/u/p/app")));
    assert_eq!(locations.widening_project_root(&elsewhere), Some(Path::new("/srv/site")));
    assert_eq!(locations.widening_project_root(&home), None);
    assert_eq!(locations.widening_project_root(&root), None);
    assert_eq!(locations.widening_project_root(&above), None);
}

/// `/home` links to `/var/home`, so tools declare paths under this form.
const CANONICAL_HOME: &str = "/var/home/u";

fn linked_locations() -> Locations {
    locations().with_home_alias(CANONICAL_HOME).unwrap()
}

#[rstest]
#[case::ssh_key("/var/home/u/.ssh/id_rsa", PathClass::Secrets)]
#[case::ssh_dir("/var/home/u/.ssh", PathClass::Secrets)]
#[case::gnupg("/var/home/u/.gnupg/pubring.kbx", PathClass::Secrets)]
#[case::keyrings("/var/home/u/.local/share/keyrings/login.keyring", PathClass::Secrets)]
#[case::netrc("/var/home/u/.netrc", PathClass::Secrets)]
#[case::daemon_secrets(
    "/var/home/u/.local/share/efr/secrets/openai-subscription.json",
    PathClass::Secrets
)]
#[case::scratch(
    "/var/home/u/.local/share/efr/scratch/2026-10-04-fix-dns-0a1b2c3d/a.txt",
    PathClass::Scratch
)]
#[case::rc_file("/var/home/u/.zshrc", PathClass::UserConfig)]
#[case::config_dir("/var/home/u/.config/nvim/init.lua", PathClass::UserConfig)]
#[case::documents("/var/home/u/Documents/plan.md", PathClass::UserData)]
#[case::cache("/var/home/u/.cache/pip/x", PathClass::UserData)]
#[case::home_itself(CANONICAL_HOME, PathClass::UserData)]
#[case::lexical_ssh_key("/home/u/.ssh/id_rsa", PathClass::Secrets)]
#[case::lexical_documents("/home/u/Documents/plan.md", PathClass::UserData)]
#[case::above_alias("/var/home", PathClass::System)]
#[case::other_user("/var/home/u2/.ssh/id_rsa", PathClass::System)]
#[case::escape_by_dots("/var/home/u/p/../../u2/.ssh/id_rsa", PathClass::System)]
#[case::back_in_by_dots("/var/home/u2/../u/.ssh/id_rsa", PathClass::Secrets)]
fn a_linked_home_classifies_both_forms_alike(#[case] path: &str, #[case] expected: PathClass) {
    assert_eq!(linked_locations().classify(Path::new(path), Path::new(SCRATCH)), Some(expected));
}

#[test]
fn a_scratch_in_the_resolved_form_matches_paths_in_either_form() {
    let scratch = Path::new("/var/home/u/.local/share/efr/scratch/2026-10-04-fix-dns-0a1b2c3d");
    let locations = linked_locations();
    for path in [format!("{SCRATCH}/a"), format!("{}/a", scratch.display())] {
        assert_eq!(locations.classify(Path::new(&path), scratch), Some(PathClass::Scratch));
    }
}

#[rstest]
#[case::canonical_home(CANONICAL_HOME)]
#[case::above_canonical_home("/var/home")]
#[case::var("/var")]
fn a_scratch_that_covers_any_form_of_home_makes_nothing_scratch(#[case] scratch: &str) {
    let locations = linked_locations();
    for path in ["/home/u/notes.txt", "/var/home/u/notes.txt"] {
        assert_eq!(
            locations.classify(Path::new(path), Path::new(scratch)),
            Some(PathClass::UserData),
            "{path}"
        );
    }
}

#[test]
fn roots_in_either_form_match_paths_in_either_form_whatever_the_order() {
    let id: ProjectId = "0192f0c1-7a00-7000-8000-000000000001".parse().unwrap();
    let above: ProjectId = "0192f0c1-7a00-7000-8000-000000000002".parse().unwrap();
    let home: ProjectId = "0192f0c1-7a00-7000-8000-000000000003".parse().unwrap();
    let locations = Locations::new(HOME)
        .unwrap()
        // Added in the resolved form before the alias is known.
        .with_secret_root("/var/home/u/vault")
        .unwrap()
        .with_project(id, "/var/home/u/p/app")
        .unwrap()
        .with_home_alias(CANONICAL_HOME)
        .unwrap()
        .with_user_config_root("/var/home/u/dotfiles")
        .unwrap()
        .with_project(above, "/var/home")
        .unwrap()
        .with_project(home, CANONICAL_HOME)
        .unwrap();
    for form in [HOME, CANONICAL_HOME] {
        assert_eq!(
            locations.classify(&Path::new(form).join("vault/key"), Path::new(SCRATCH)),
            Some(PathClass::Secrets),
            "{form}"
        );
        assert_eq!(
            locations.classify(&Path::new(form).join("dotfiles/zshrc"), Path::new(SCRATCH)),
            Some(PathClass::UserConfig),
            "{form}"
        );
    }
    assert_eq!(locations.project_root(&id), Some(Path::new("/home/u/p/app")));
    assert_eq!(locations.widening_project_root(&id), Some(Path::new("/home/u/p/app")));
    assert_eq!(locations.widening_project_root(&above), None);
    assert_eq!(locations.widening_project_root(&home), None);
}

#[test]
fn a_home_alias_must_be_absolute_and_apart_from_every_form_of_home() {
    let home = || Locations::new(HOME).unwrap();
    assert_eq!(
        home().with_home_alias("var/home/u").unwrap_err(),
        PermissionsError::NotAbsolute { path: PathBuf::from("var/home/u") }
    );
    assert_eq!(home().with_home_alias("/").unwrap_err(), PermissionsError::HomeIsRoot);
    for overlapping in ["/home", "/home/u/link"] {
        assert_eq!(
            home().with_home_alias(overlapping).unwrap_err(),
            PermissionsError::HomeAliasOverlaps { alias: PathBuf::from(overlapping) }
        );
    }
    assert_eq!(
        home().with_home_alias(CANONICAL_HOME).unwrap().with_home_alias("/var/home").unwrap_err(),
        PermissionsError::HomeAliasOverlaps { alias: PathBuf::from("/var/home") }
    );
    assert_eq!(home().with_home_alias("/home/./u/").unwrap().home_aliases(), [] as [PathBuf; 0]);
    let twice = home().with_home_alias("/var/home/u/").unwrap().with_home_alias(CANONICAL_HOME);
    assert_eq!(twice.unwrap().home_aliases(), [PathBuf::from(CANONICAL_HOME)]);
}

#[rstest]
#[case("/", "/")]
#[case("/a/b", "/a/b")]
#[case("/a/./b/", "/a/b")]
#[case("/a/../b", "/b")]
#[case("/../../a", "/a")]
#[case("//a//b", "/a/b")]
#[case("/a/b/..", "/a")]
fn normalizes(#[case] path: &str, #[case] expected: &str) {
    assert_eq!(normalize(Path::new(path)), Some(PathBuf::from(expected)));
}

#[test]
fn class_names_and_words() {
    let names: Vec<_> = PathClass::ALL.iter().map(|class| class.as_str()).collect();
    assert_eq!(names, ["scratch", "user_config", "user_data", "system", "secrets"]);
    let words: Vec<_> = PathClass::ALL.iter().map(ToString::to_string).collect();
    assert_eq!(words, ["scratch", "user config", "user data", "system", "secrets"]);
}

fn segment() -> impl Strategy<Value = &'static str> {
    prop::sample::select(vec![
        "home", "u", ".ssh", ".config", "etc", "p", "app", ".local", "share", "efr", "secrets",
        "scratch", "..", ".",
    ])
}

proptest! {
    #[test]
    fn normal_form_is_stable(segments in prop::collection::vec(segment(), 0..10)) {
        let path = PathBuf::from(format!("/{}", segments.join("/")));
        let once = normalize(&path).unwrap();
        prop_assert_eq!(normalize(&once), Some(once.clone()));
        prop_assert!(!once.components().any(|c| matches!(
            c,
            std::path::Component::CurDir | std::path::Component::ParentDir
        )));
    }

    #[test]
    fn a_detour_does_not_change_the_class(
        segments in prop::collection::vec(segment(), 0..8),
        detour in segment(),
    ) {
        // `x/..` returns to where it started only when `x` names a directory; after
        // `..` or `.`, the `..` climbs one level instead.
        prop_assume!(detour != ".." && detour != ".");
        let locations = locations();
        let plain = PathBuf::from(format!("/{}", segments.join("/")));
        let with_detour = plain.join(detour).join("..");
        prop_assert_eq!(
            locations.classify(&plain, Path::new(SCRATCH)),
            locations.classify(&with_detour, Path::new(SCRATCH))
        );
    }

    #[test]
    fn both_forms_of_a_linked_home_get_the_same_class(
        segments in prop::collection::vec(segment().prop_filter("stays below home", |s| *s != ".."), 0..8),
    ) {
        let locations = linked_locations();
        let rest = segments.join("/");
        let lexical = PathBuf::from(format!("{HOME}/{rest}"));
        let resolved = PathBuf::from(format!("{CANONICAL_HOME}/{rest}"));
        prop_assert_eq!(
            locations.classify(&lexical, Path::new(SCRATCH)),
            locations.classify(&resolved, Path::new(SCRATCH))
        );
    }
}

use std::path::{Path, PathBuf};

use super::{depth, expand_home, is_normal, is_within, normalize, too_wide};

#[test]
fn normal_form_applies_dots_and_refuses_relative_paths() {
    assert_eq!(normalize(Path::new("/a/./b/../c/")), Some(PathBuf::from("/a/c")));
    assert_eq!(normalize(Path::new("/../..")), Some(PathBuf::from("/")));
    assert_eq!(normalize(Path::new("a/b")), None);
    assert!(is_normal(Path::new("/a/c")));
    assert!(!is_normal(Path::new("/a/../c")));
    assert!(!is_normal(Path::new("/a/c/")));
}

#[test]
fn containment_is_by_component() {
    assert!(is_within(Path::new("/home/u/p"), Path::new("/home/u")));
    assert!(is_within(Path::new("/home/u"), Path::new("/home/u")));
    assert!(!is_within(Path::new("/home/user"), Path::new("/home/u")));
    assert_eq!(depth(Path::new("/")), 0);
    assert_eq!(depth(Path::new("/home/u/.cargo")), 3);
}

#[test]
fn the_tilde_component_expands_and_nothing_else_does() {
    let home = Path::new("/home/u");
    assert_eq!(expand_home(Path::new("~"), home), PathBuf::from("/home/u"));
    assert_eq!(expand_home(Path::new("~/.cargo"), home), PathBuf::from("/home/u/.cargo"));
    assert_eq!(expand_home(Path::new("~x/a"), home), PathBuf::from("~x/a"));
    assert_eq!(expand_home(Path::new("/etc"), home), PathBuf::from("/etc"));
}

#[test]
fn home_and_everything_above_it_is_too_wide() {
    let home = Path::new("/home/u");
    for path in ["/", "/home", "/home/u"] {
        assert!(too_wide(Path::new(path), home), "{path}");
    }
    for path in ["/home/u/p", "/srv", "/home/v"] {
        assert!(!too_wide(Path::new(path), home), "{path}");
    }
}

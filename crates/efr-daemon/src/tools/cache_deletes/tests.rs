use std::path::{Path, PathBuf};

use efr_config::Settings;
use pretty_assertions::assert_eq;

use crate::tools::cache_deletes::{Checked, caches, check, masks};

fn home() -> PathBuf {
    PathBuf::from("/home/u")
}

#[test]
fn the_caches_and_masks_come_from_the_settings_with_the_cargo_pins() {
    let mut settings = Settings::default();
    settings.sandbox.caches = vec!["~/.cargo".into(), "~/.cache".into(), "relative".into()];
    settings.sandbox.mask = vec!["~/.cache/private".into()];

    let caches = caches(&home(), &settings);
    let masks = masks(&home(), &settings, vec![PathBuf::from("/home/u/.ssh")]);

    let targets: Vec<&Path> = caches.iter().map(|cache| cache.target.as_path()).collect();
    assert_eq!(targets, [Path::new("/home/u/.cargo"), Path::new("/home/u/.cache")]);
    assert!(caches[0].pins.contains(&PathBuf::from("/home/u/.cargo/bin")));
    assert!(masks.contains(&PathBuf::from("/home/u/.ssh")));
    assert!(masks.contains(&PathBuf::from("/home/u/.cache/mozilla")), "a sandbox mask");
    assert!(masks.contains(&PathBuf::from("/home/u/.cache/private")));
}

#[test]
fn only_a_normal_path_inside_a_cache_and_clear_of_pins_and_masks_passes() {
    let mut settings = Settings::default();
    settings.sandbox.caches = vec!["~/.cargo".into(), "~/.cache".into(), "~/.cache/go".into()];
    let caches = caches(&home(), &settings);
    let masks = masks(&home(), &settings, Vec::new());
    let ok = |path: &str| check(&[PathBuf::from(path)], &caches, &masks);

    assert_eq!(
        ok("/home/u/.cache/go/build/x"),
        Ok(vec![Checked { cache: "/home/u/.cache/go".into(), relative: "build/x".into() }]),
        "the innermost cache"
    );
    assert!(ok("/home/u/.cargo/registry/cache").is_ok());
    for refused in [
        "/home/u/.cargo/bin/x",
        "/home/u/.cargo/bin",
        "/home/u/.cargo/config.toml",
        "/home/u/.cargo",
        "/home/u/.cache/mozilla/profile",
        "/home/u/.cache/../.ssh/id",
        "/home/u/Documents/thesis.tex",
        "relative/path",
    ] {
        assert_eq!(ok(refused), Err(PathBuf::from(refused)), "{refused}");
    }
    let one_bad = [PathBuf::from("/home/u/.cache/go/a"), PathBuf::from("/home/u/.cargo/bin/x")];
    assert_eq!(check(&one_bad, &caches, &masks), Err(PathBuf::from("/home/u/.cargo/bin/x")));
}

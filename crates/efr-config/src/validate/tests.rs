use crate::Settings;

use super::{check, is_effort};

#[test]
fn the_defaults_pass_every_check() {
    assert_eq!(check(&Settings::default()), Ok(()));
}

#[test]
fn an_effort_is_one_lowercase_word() {
    for effort in ["low", "medium", "high", "xhigh", "minimal", "none", "very-high", "max_2"] {
        assert!(is_effort(effort), "{effort}");
    }
    for effort in ["", "High", "very high", "high!", "\u{e9}"] {
        assert!(!is_effort(effort), "{effort}");
    }
}

#[test]
fn a_secret_path_below_home_must_start_with_the_tilde_component() {
    let mut settings = Settings::default();
    settings.permissions.secret_paths = vec!["~/.vault".into(), "/srv/vault".into()];
    assert_eq!(check(&settings), Ok(()));

    settings.permissions.secret_paths = vec!["~vault".into()];
    let invalid = check(&settings).unwrap_err();
    assert_eq!(invalid.key, "permissions.secret_paths");
    assert_eq!(invalid.value, "\"~vault\"");
}

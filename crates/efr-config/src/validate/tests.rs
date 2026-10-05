use crate::Settings;

use super::check;

#[test]
fn the_defaults_pass_every_check() {
    assert_eq!(check(&Settings::default()), Ok(()));
}

#[test]
fn an_effort_is_one_lowercase_word_of_at_most_32_bytes() {
    let with = |effort: &str| {
        let mut settings = Settings::default();
        settings.model.effort = Some(effort.to_owned());
        check(&settings)
    };
    let longest = "a".repeat(32);
    for effort in
        ["low", "medium", "high", "xhigh", "minimal", "none", "very-high", "max_2", &longest]
    {
        assert_eq!(with(effort), Ok(()), "{effort}");
    }
    let too_long = "a".repeat(33);
    for effort in ["", "High", "very high", "high!", "\u{e9}", &too_long] {
        assert_eq!(with(effort).map_err(|invalid| invalid.key), Err("model.effort"), "{effort}");
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

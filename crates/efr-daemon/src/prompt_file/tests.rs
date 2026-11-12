use std::os::unix::fs::PermissionsExt as _;

use efr_config::{ColorValue, RenderColors, Settings};
use efr_protocol::Mode;
use pretty_assertions::assert_eq;

use super::{colors, path, text};

fn settings(mode: Mode) -> Settings {
    let mut settings = Settings::default();
    settings.permissions.mode = mode;
    settings
}

fn written(settings: &Settings, refused: bool, colors: &RenderColors) -> String {
    String::from_utf8(text(settings, refused, colors)).unwrap()
}

#[test]
fn the_mode_is_the_default_of_a_prompt_that_names_none() {
    let none = RenderColors::default();
    assert_eq!(written(&Settings::default(), false, &none), "mode\tcautious\n");
    for mode in Mode::ALL {
        assert_eq!(written(&settings(mode), false, &none), format!("mode\t{}\n", mode.as_str()));
    }
}

#[test]
fn a_full_default_that_efrd_refuses_runs_as_auto() {
    let none = RenderColors::default();
    assert_eq!(written(&settings(Mode::Full), true, &none), "mode\tauto\n");
    assert_eq!(written(&settings(Mode::Cautious), true, &none), "mode\tcautious\n");
}

#[test]
fn the_roles_that_the_plugin_paints_follow_with_their_colours() {
    let mut set = RenderColors::default();
    set.muted = Some(ColorValue::Text("#8695B7".to_owned()));
    set.error = Some(ColorValue::Number(1));
    set.accent = Some(ColorValue::Text("yellow".to_owned()));
    // A role that the plugin does not paint has no line.
    set.link = Some(ColorValue::Text("#c084fc".to_owned()));
    assert_eq!(
        written(&settings(Mode::Auto), false, &set),
        "mode\tauto\ncolor.muted\t#8695b7\ncolor.accent\t3\ncolor.error\t1\n"
    );
}

#[test]
fn the_config_wins_over_the_theme_file_and_a_bad_theme_file_adds_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let theme = dir.path().join("theme.toml");
    std::fs::write(&theme, "[colors]\nmuted = \"#8695b7\"\ntext = \"#d8d3c3\"\n").unwrap();
    let mut settings = Settings::default();
    settings.render.palette = Some(theme.clone());
    settings.render.colors.text = Some(ColorValue::Number(7));
    assert_eq!(
        written(&settings, false, &colors(&settings, dir.path())),
        "mode\tcautious\ncolor.text\t7\ncolor.muted\t#8695b7\n"
    );

    std::fs::write(&theme, "[colors]\nmuted = \"not a colour\"\n").unwrap();
    assert_eq!(
        written(&settings, false, &colors(&settings, dir.path())),
        "mode\tcautious\ncolor.text\t7\n"
    );
}

#[test]
fn the_file_is_written_once_and_private() {
    let dir = tempfile::tempdir().unwrap();
    let file = path(dir.path());
    let bytes = text(&Settings::default(), false, &RenderColors::default());
    assert!(crate::projects_file::sync(&file, &bytes).unwrap());
    assert!(!crate::projects_file::sync(&file, &bytes).unwrap(), "the same text is not written");
    assert_eq!(std::fs::read(&file).unwrap(), b"mode\tcautious\n");
    let mode = std::fs::metadata(&file).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
}

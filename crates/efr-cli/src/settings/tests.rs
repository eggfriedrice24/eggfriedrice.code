use std::path::Path;

use efr_render::Theme;
use pretty_assertions::assert_eq;

use super::{CONFIG_FILE, Settings, Warning};

const PATH: &str = "/home/user/.config/efr/config.toml";

fn parse(text: &str) -> Settings {
    Settings::parse(Path::new(PATH), text)
}

#[test]
fn without_a_render_table_the_defaults_apply() {
    let settings = parse("[providers.openai]\nmodel = \"gpt-5\"\n");
    assert_eq!(settings.theme, Theme::ANSI);
    assert!(settings.warnings.is_empty());
}

#[test]
fn the_theme_comes_from_the_render_table() {
    let settings = parse("[render]\ntheme = \"Catppuccin Mocha\"\n");
    assert_eq!(settings.theme, Theme::from_name("catppuccin-mocha").unwrap());
    assert!(settings.warnings.is_empty());
}

#[test]
fn an_unknown_theme_warns_and_keeps_the_default() {
    let settings = parse("[render]\ntheme = \"neon-dreams\"\n");
    assert_eq!(settings.theme, Theme::ANSI);
    let [warning] = settings.warnings.as_slice() else { panic!("one warning expected") };
    assert!(matches!(warning, Warning::UnknownTheme { name, .. } if name == "neon-dreams"));
    assert_eq!(
        warning.to_string(),
        format!("{PATH} names the theme \"neon-dreams\", which does not exist")
    );
}

#[test]
fn a_file_that_is_not_toml_warns_and_keeps_the_defaults() {
    let settings = parse("[render\ntheme = ");
    assert_eq!(settings.theme, Theme::ANSI);
    let [warning] = settings.warnings.as_slice() else { panic!("one warning expected") };
    assert!(matches!(warning, Warning::Invalid { .. }));
    assert!(warning.to_string().starts_with(&format!("{PATH} is not valid: ")));
}

#[test]
fn a_theme_of_the_wrong_type_is_invalid() {
    let settings = parse("[render]\ntheme = 7\n");
    assert!(matches!(settings.warnings.as_slice(), [Warning::Invalid { .. }]));
}

#[tokio::test]
async fn a_missing_file_is_not_a_warning() {
    let dir = tempfile::tempdir().unwrap();
    let settings = Settings::load(dir.path()).await;
    assert_eq!(settings.theme, Theme::ANSI);
    assert!(settings.warnings.is_empty());
}

#[tokio::test]
async fn load_reads_config_toml_in_the_config_root() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(CONFIG_FILE), "[render]\ntheme = \"nord\"\n").unwrap();
    let settings = Settings::load(dir.path()).await;
    assert_eq!(settings.theme.name(), "nord");
}

#[tokio::test]
async fn an_unreadable_file_warns() {
    let dir = tempfile::tempdir().unwrap();
    // A directory where the file should be cannot be read as one.
    std::fs::create_dir(dir.path().join(CONFIG_FILE)).unwrap();
    let settings = Settings::load(dir.path()).await;
    assert!(matches!(settings.warnings.as_slice(), [Warning::Unreadable { .. }]));
}

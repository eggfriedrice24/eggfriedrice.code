use std::path::Path;

use efr_render::{Colour, Role, Theme};
use pretty_assertions::assert_eq;

use efr_config::{COLOR_ROLES, CONFIG_FILE};

use super::{Settings, Source, Warning, problem};
use crate::terminal::Background;

const PATH: &str = "/home/user/.config/efr/config.toml";

fn parse(text: &str) -> Settings {
    Settings::parse(Path::new(PATH), text)
}

#[test]
fn without_a_render_table_the_defaults_apply() {
    let settings = parse("[model]\nname = \"gpt-5\"\n");
    assert_eq!(settings.theme, Theme::ANSI);
    assert_eq!(settings.theme_source, Source::Default);
    assert!(settings.warnings.is_empty());
}

#[test]
fn a_key_the_daemon_would_refuse_warns_and_keeps_the_defaults() {
    let settings = parse("[render]\ntheme = \"nord\"\n[providers.openai]\nmodel = \"gpt-5\"\n");
    assert_eq!(settings.theme, Theme::ANSI, "the theme of a refused file is not used");
    let [warning] = settings.warnings.as_slice() else { panic!("one warning expected") };
    assert!(matches!(warning, Warning::Invalid { .. }));
    let text = warning.to_string();
    assert!(text.starts_with(&format!("{PATH} is not valid: ")), "{text}");
    assert!(text.ends_with("(providers.openai, line 3, column 2)"), "{text}");
}

#[test]
fn a_value_out_of_range_names_its_key_and_place() {
    let settings = parse("[conversation]\nmax_queued = 0\n");
    let [warning] = settings.warnings.as_slice() else { panic!("one warning expected") };
    assert_eq!(
        warning.to_string(),
        format!(
            "{PATH} is not valid: conversation.max_queued = 0 is not between 1 and 1024 \
             (conversation.max_queued, line 2, column 14)"
        )
    );
}

#[test]
fn the_theme_comes_from_the_render_table() {
    let settings = parse("[render]\ntheme = \"Catppuccin Mocha\"\n");
    assert_eq!(settings.theme, Theme::from_name("catppuccin-mocha").unwrap());
    assert_eq!(settings.theme_source, Source::File(PATH.into()));
    assert!(settings.warnings.is_empty());
}

#[test]
fn an_unknown_theme_warns_and_keeps_the_default() {
    let settings = parse("[render]\ntheme = \"neon-dreams\"\n");
    assert_eq!(settings.theme, Theme::ANSI);
    assert_eq!(settings.theme_source, Source::Default);
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
    let settings = Settings::load(dir.path(), None, dir.path()).await;
    assert_eq!(settings.theme, Theme::ANSI);
    assert!(settings.warnings.is_empty());
}

#[tokio::test]
async fn load_reads_config_toml_in_the_config_root() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(CONFIG_FILE), "[render]\ntheme = \"nord\"\n").unwrap();
    let settings = Settings::load(dir.path(), None, dir.path()).await;
    assert_eq!(settings.theme.name(), "nord");
    assert_eq!(settings.theme_source, Source::File(dir.path().join(CONFIG_FILE)));
}

#[tokio::test]
async fn an_unreadable_file_warns() {
    let dir = tempfile::tempdir().unwrap();
    // A directory where the file should be cannot be read as one.
    std::fs::create_dir(dir.path().join(CONFIG_FILE)).unwrap();
    let settings = Settings::load(dir.path(), None, dir.path()).await;
    assert!(matches!(settings.warnings.as_slice(), [Warning::Unreadable { .. }]));
}

#[test]
fn the_role_names_of_the_config_are_the_roles_of_the_renderer() {
    let roles: Vec<&str> = Role::ALL.iter().map(|role| role.name()).collect();
    assert_eq!(COLOR_ROLES.to_vec(), roles);
}

#[test]
fn auto_takes_the_dark_or_the_light_theme_by_the_background() {
    let text = "[render]\ntheme = \"auto\"\ntheme_light = \"github\"\n";
    let path = Path::new(PATH);
    let light = Settings::parse_on(path, Some(text), Some("light"));
    assert_eq!(light.theme.name(), "github");
    assert_eq!(light.background, Some(Background::Light));
    assert!(light.auto_theme());
    let dark = Settings::parse_on(path, Some(text), Some("dark"));
    assert_eq!(dark.theme.name(), "catppuccin-mocha");
    // Without a background, auto means dark.
    let unknown = Settings::parse_on(path, Some(text), None);
    assert_eq!(unknown.theme.name(), "catppuccin-mocha");
    assert!(unknown.warnings.is_empty());
    // A value that names neither is a warning, and dark applies.
    let odd = Settings::parse_on(path, Some(text), Some("grey"));
    assert_eq!(odd.theme.name(), "catppuccin-mocha");
    let [warning] = odd.warnings.as_slice() else { panic!("one warning expected") };
    assert!(matches!(warning, Warning::Background { value } if value == "grey"));
    // The background matters without a config file too, for `efr config show`.
    let none = Settings::parse_on(path, None, Some("light"));
    assert_eq!((none.background, none.theme), (Some(Background::Light), Theme::ANSI));
}

#[test]
fn an_auto_theme_that_does_not_exist_warns() {
    let text = "[render]\ntheme = \"auto\"\ntheme_dark = \"neon\"\n";
    let settings = Settings::parse_on(Path::new(PATH), Some(text), Some("dark"));
    assert_eq!(settings.theme, Theme::ANSI);
    assert!(
        matches!(settings.warnings.as_slice(), [Warning::UnknownTheme { name, .. }] if name == "neon")
    );
}

/// A config root in a temporary directory with `config` as its file, and the files
/// `others` beside it.
fn root(config: &str, others: &[(&str, &[u8])]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(CONFIG_FILE), config).unwrap();
    for (name, bytes) in others {
        std::fs::write(dir.path().join(name), bytes).unwrap();
    }
    dir
}

/// A `.tmTheme` file that syntect reads.
fn tmtheme() -> Vec<u8> {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../efr-render/fixtures/sample_theme.tmTheme");
    std::fs::read(path).unwrap()
}

#[tokio::test]
async fn render_colors_win_over_the_theme_file_which_wins_over_the_default() {
    let theme = b"code_theme = \"code.tmTheme\"\n[colors]\naccent = \"#f2c14e\"\nmuted = 8\ndiff.add = \"green\"\n";
    let config = "[render]\npalette = \"~/theme.toml\"\n[render.colors]\nmuted = \"bright-blue\"\n";
    let dir = root(config, &[("theme.toml", theme), ("code.tmTheme", &tmtheme())]);
    let settings = Settings::load(dir.path(), None, dir.path()).await;
    assert!(settings.warnings.is_empty(), "{:?}", settings.warnings);
    let palette = &settings.palette;
    assert_eq!(palette.get(Role::Accent), Some(Colour::Rgb(0xf2, 0xc1, 0x4e)));
    assert_eq!(palette.get(Role::Muted), Some(Colour::Palette(12)));
    assert_eq!(palette.get(Role::DiffAdd), Some(Colour::Palette(2)));
    assert_eq!(palette.get(Role::Error), None);
    // The code theme's path is taken from the theme file's directory.
    assert!(settings.code_theme.is_some());
    assert_eq!(settings.code_theme_path, Some(dir.path().join("code.tmTheme")));
}

#[tokio::test]
async fn a_theme_file_that_cannot_be_used_warns_and_the_config_colours_stay() {
    let config = "[render]\npalette = \"~/theme.toml\"\n[render.colors]\nerror = 9\n";
    let dir = root(config, &[("theme.toml", b"[colors]\naccent = \"purple\"\n")]);
    let settings = Settings::load(dir.path(), None, dir.path()).await;
    let [warning] = settings.warnings.as_slice() else { panic!("one warning expected") };
    assert!(matches!(warning, Warning::Palette { .. }));
    assert!(warning.to_string().contains("colors.accent"), "{warning}");
    assert_eq!(settings.palette.get(Role::Accent), None);
    assert_eq!(settings.palette.get(Role::Error), Some(Colour::Palette(9)));

    let missing = root("[render]\npalette = \"~/none.toml\"\n", &[]);
    let settings = Settings::load(missing.path(), None, missing.path()).await;
    assert!(matches!(settings.warnings.as_slice(), [Warning::Palette { .. }]));
}

#[tokio::test]
async fn a_code_theme_that_is_not_a_tmtheme_warns_and_the_theme_applies() {
    let theme: &[u8] = b"code_theme = \"bad.tmTheme\"\n";
    let config = "[render]\ntheme = \"nord\"\npalette = \"~/theme.toml\"\n";
    let dir = root(config, &[("theme.toml", theme), ("bad.tmTheme", b"not a plist")]);
    let settings = Settings::load(dir.path(), None, dir.path()).await;
    assert!(matches!(settings.warnings.as_slice(), [Warning::CodeThemeInvalid { .. }]));
    assert!(settings.code_theme.is_none());
    assert_eq!(settings.theme.name(), "nord");
}

#[tokio::test]
async fn a_problem_names_a_theme_a_theme_file_or_a_code_theme_that_cannot_be_used() {
    let home = tempfile::tempdir().unwrap();
    let check = |text: &str| {
        let text = text.to_owned();
        let home = home.path().to_path_buf();
        async move { problem(Path::new(PATH), Some(&text), &home).await }
    };
    assert_eq!(check("[render]\ntheme = \"auto\"\n").await, None);
    assert_eq!(
        check("[render]\ntheme_light = \"neon\"\n").await.as_deref(),
        Some("render.theme_light names the theme \"neon\", which does not exist")
    );
    let missing = check("[render]\npalette = \"~/theme.toml\"\n").await.unwrap();
    assert!(missing.starts_with("render.palette: "), "{missing}");
    std::fs::write(home.path().join("theme.toml"), "[colors]\nlink = 99\n").unwrap();
    let invalid = check("[render]\npalette = \"~/theme.toml\"\n").await.unwrap();
    assert!(invalid.contains("colors.link"), "{invalid}");
    // The parser's quote of the file with a caret stays out; the place follows.
    std::fs::write(home.path().join("theme.toml"), "[colors]\n\"diff.add\" = \"green\"\n").unwrap();
    let parse = check("[render]\npalette = \"~/theme.toml\"\n").await.unwrap();
    let file = home.path().join("theme.toml");
    let expected = format!(
        "render.palette: the theme file {} is not valid: unknown field `diff.add`, expected one of",
        file.display()
    );
    assert!(parse.starts_with(&expected), "{parse}");
    assert!(parse.ends_with("(colors.diff.add, line 2, column 1)"), "{parse}");
    assert!(!parse.contains("TOML parse error") && !parse.contains('|'), "{parse}");
    std::fs::write(home.path().join("theme.toml"), "code_theme = \"x.tmTheme\"\n").unwrap();
    std::fs::write(home.path().join("x.tmTheme"), "nope").unwrap();
    let code = check("[render]\npalette = \"~/theme.toml\"\n").await.unwrap();
    assert!(code.contains("x.tmTheme"), "{code}");
    std::fs::write(home.path().join("x.tmTheme"), tmtheme()).unwrap();
    assert_eq!(check("[render]\npalette = \"~/theme.toml\"\n").await, None);
}

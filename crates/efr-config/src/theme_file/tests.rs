use std::path::{Path, PathBuf};

use pretty_assertions::assert_eq;

use super::ThemeFile;
use crate::{ConfigError, Location, RoleColor, Settings};

const PATH: &str = "/home/u/.config/efr/theme.toml";

/// A theme file of the eggfriedrice design system: every role in hex and a code theme
/// next to it.
const HEX: &str = r##"code_theme = "../bat/eggfriedrice.tmTheme"

[colors]
text = "#e8e2d4"
muted = "#7a7266"
accent = "#f2c14e"
heading = "#f2c14e"
link = "#7fb4ca"
code = "#9fc27a"
success = "#8fbf6a"
warning = "#e8a33d"
error = "#e05d4f"
quote = "#b8b0a0"
diff.add = "#8fbf6a"
diff.remove = "#e05d4f"
diff.hunk = "#7fb4ca"
"##;

#[test]
fn a_hex_theme_file_sets_every_role() {
    let file = ThemeFile::parse(Path::new(PATH), HEX).unwrap();
    let colors = file.colors.colors();
    assert_eq!(colors.len(), 13);
    assert_eq!(colors[2], ("accent", RoleColor::Rgb(0xf2, 0xc1, 0x4e)));
    assert_eq!(colors[12], ("diff.hunk", RoleColor::Rgb(0x7f, 0xb4, 0xca)));
    assert_eq!(file.code_theme.as_deref(), Some(Path::new("../bat/eggfriedrice.tmTheme")));
}

#[test]
fn the_code_theme_resolves_from_the_theme_files_directory_or_home() {
    let home = Path::new("/home/u");
    let with = |code_theme: &str| {
        ThemeFile::parse(Path::new(PATH), &format!("code_theme = {code_theme:?}\n")).unwrap()
    };
    assert_eq!(
        with("../bat/x.tmTheme").code_theme_path(home),
        Some(PathBuf::from("/home/u/.config/efr/../bat/x.tmTheme"))
    );
    assert_eq!(
        with("~/themes/x.tmTheme").code_theme_path(home),
        Some(PathBuf::from("/home/u/themes/x.tmTheme"))
    );
    assert_eq!(
        with("/usr/share/x.tmTheme").code_theme_path(home),
        Some(PathBuf::from("/usr/share/x.tmTheme"))
    );
    let empty = ThemeFile::parse(Path::new(PATH), "").unwrap();
    assert_eq!(empty.code_theme_path(home), None);
    assert_eq!(empty.colors.colors(), []);
}

#[test]
fn a_bad_colour_is_an_error_naming_its_key_and_place() {
    let error =
        ThemeFile::parse(Path::new(PATH), "[colors]\nmuted = 8\naccent = \"gold\"\n").unwrap_err();
    assert!(matches!(&error, ConfigError::ThemeInvalid { key: "colors.accent", .. }), "{error:?}");
    assert_eq!(error.key().as_deref(), Some("colors.accent"));
    assert_eq!(error.location(), Some(Location { line: 3, column: 10 }));
    assert_eq!(
        error.to_string(),
        format!(
            "the theme value colors.accent = \"gold\" in {PATH} is not a colour: \"#rrggbb\", \
             an ANSI slot from 0 to 15, or a name such as \"yellow\""
        )
    );
    let error = ThemeFile::parse(Path::new(PATH), "[colors.diff]\nhunk = 16\n").unwrap_err();
    assert_eq!(error.key().as_deref(), Some("colors.diff.hunk"));
}

#[test]
fn an_unknown_key_or_a_wrong_type_is_an_error_naming_its_key() {
    let error = ThemeFile::parse(Path::new(PATH), "[colors]\naccnet = 3\n").unwrap_err();
    assert!(matches!(error, ConfigError::ThemeParse { .. }), "{error:?}");
    assert_eq!(error.key().as_deref(), Some("colors.accnet"));
    let error = ThemeFile::parse(Path::new(PATH), "[colors]\nlink = true\n").unwrap_err();
    assert_eq!(error.key().as_deref(), Some("colors.link"));
    assert_eq!(error.location().map(|at| at.line), Some(2));
    let error = ThemeFile::parse(Path::new(PATH), "theme = \"nord\"\n").unwrap_err();
    assert_eq!(error.key().as_deref(), Some("theme"));
    let error = ThemeFile::parse(Path::new(PATH), "code_theme = \"\"\n").unwrap_err();
    assert_eq!(error.key().as_deref(), Some("code_theme"));
}

#[test]
fn load_reads_the_file_and_names_a_missing_one() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("theme.toml");
    std::fs::write(&path, HEX).unwrap();
    let file = ThemeFile::load(&path).unwrap();
    assert_eq!(file.path, path);
    assert_eq!(file.colors.colors().len(), 13);
    let missing = dir.path().join("none.toml");
    let error = ThemeFile::load(&missing).unwrap_err();
    assert!(matches!(&error, ConfigError::ThemeRead { path, .. } if *path == missing));
}

/// `[render.colors]` wins over the theme file, and the theme file over the defaults.
#[test]
fn the_config_colours_win_over_the_theme_file() {
    let config = "[render]\npalette = \"~/theme.toml\"\n\n[render.colors]\naccent = 5\n";
    let settings = Settings::parse(Path::new("/c/config.toml"), Some(config)).unwrap();
    let file = ThemeFile::parse(Path::new(PATH), HEX).unwrap();
    let merged = settings.render.colors.over(&file.colors).colors();
    assert_eq!(merged[2], ("accent", RoleColor::Slot(5)));
    assert_eq!(merged[1], ("muted", RoleColor::Rgb(0x7a, 0x72, 0x66)));
    assert_eq!(merged.len(), 13);
}

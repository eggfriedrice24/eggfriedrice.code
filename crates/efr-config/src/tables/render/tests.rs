use std::path::Path;

use pretty_assertions::assert_eq;

use super::{
    COLOR_ROLES, CONFIG_COLOR_KEYS, ColorValue, RenderColors, RenderSettings, RoleColor,
    THEME_COLOR_KEYS,
};

fn text(value: &str) -> Option<RoleColor> {
    ColorValue::Text(value.to_owned()).color()
}

#[test]
fn a_colour_is_hex_a_slot_or_a_name() {
    assert_eq!(text("#f2c14e"), Some(RoleColor::Rgb(0xf2, 0xc1, 0x4e)));
    assert_eq!(text("#F2C14E"), Some(RoleColor::Rgb(0xf2, 0xc1, 0x4e)));
    assert_eq!(text("3"), Some(RoleColor::Slot(3)));
    assert_eq!(text("15"), Some(RoleColor::Slot(15)));
    assert_eq!(ColorValue::Number(0).color(), Some(RoleColor::Slot(0)));
    assert_eq!(ColorValue::Number(15).color(), Some(RoleColor::Slot(15)));
    assert_eq!(text("black"), Some(RoleColor::Slot(0)));
    assert_eq!(text("yellow"), Some(RoleColor::Slot(3)));
    assert_eq!(text("white"), Some(RoleColor::Slot(7)));
    assert_eq!(text("bright-black"), Some(RoleColor::Slot(8)));
    assert_eq!(text("bright-blue"), Some(RoleColor::Slot(12)));
    assert_eq!(text("bright-white"), Some(RoleColor::Slot(15)));
    assert_eq!(text("03"), Some(RoleColor::Slot(3)));
}

#[test]
fn anything_else_is_no_colour() {
    for bad in [
        "",
        "#fff",
        "#f2c14",
        "#f2c14e0",
        "#gggggg",
        "16",
        "-1",
        "purple",
        "bright-",
        "bright-purple",
        "Yellow",
        " red",
        "003",
        "3 bold",
    ] {
        assert_eq!(text(bad), None, "{bad:?}");
    }
    assert_eq!(ColorValue::Number(16).color(), None);
    assert_eq!(ColorValue::Number(-1).color(), None);
}

#[test]
fn a_value_reads_from_a_number_or_a_string_and_shows_as_toml() {
    #[derive(serde::Deserialize)]
    struct One {
        value: ColorValue,
    }
    let number: One = toml::from_str("value = 3").unwrap();
    assert_eq!(number.value, ColorValue::Number(3));
    assert_eq!(number.value.as_toml(), "3");
    let string: One = toml::from_str("value = \"#00ff00\"").unwrap();
    assert_eq!(string.value, ColorValue::Text("#00ff00".to_owned()));
    assert_eq!(string.value.as_toml(), "\"#00ff00\"");
    let error = toml::from_str::<One>("value = true").err().unwrap();
    assert!(error.message().contains("a colour"), "{}", error.message());
}

#[test]
fn entries_follow_the_roles_and_the_keys() {
    let colors = RenderColors::default();
    let names: Vec<&str> = colors.entries().iter().map(|(role, _)| *role).collect();
    assert_eq!(names, COLOR_ROLES);
    for (index, role) in COLOR_ROLES.iter().enumerate() {
        assert_eq!(CONFIG_COLOR_KEYS[index], format!("render.colors.{role}"));
        assert_eq!(THEME_COLOR_KEYS[index], format!("colors.{role}"));
    }
}

fn parse(text: &str) -> RenderColors {
    toml::from_str(text).unwrap()
}

#[test]
fn colours_over_a_base_win_where_they_are_set() {
    let config = parse("accent = \"magenta\"\ndiff.add = 10\n");
    let file =
        parse("accent = \"#f2c14e\"\nmuted = 8\ndiff.add = \"green\"\ndiff.hunk = \"cyan\"\n");
    let merged = config.over(&file);
    assert_eq!(
        merged.colors(),
        [
            ("muted", RoleColor::Slot(8)),
            ("accent", RoleColor::Slot(5)),
            ("diff.add", RoleColor::Slot(10)),
            ("diff.hunk", RoleColor::Slot(6)),
        ]
    );
    assert_eq!(RenderColors::default().over(&file), file);
    assert_eq!(file.over(&RenderColors::default()), file);
}

#[test]
fn the_first_invalid_role_is_found_in_the_order_of_the_keys() {
    assert_eq!(parse("accent = 3\n").first_invalid(), None);
    let colors = parse("code = \"teal\"\ndiff.remove = 99\n");
    assert_eq!(colors.first_invalid(), Some((5, &ColorValue::Text("teal".to_owned()))));
    let colors = parse("diff.remove = 99\n");
    assert_eq!(colors.first_invalid(), Some((11, &ColorValue::Number(99))));
}

#[test]
fn the_palette_path_expands_the_home_directory() {
    let home = Path::new("/home/u");
    let settings = |palette: &str| RenderSettings {
        palette: Some(palette.into()),
        ..RenderSettings::default()
    };
    assert_eq!(
        settings("~/themes/efr.toml").palette_path(home).as_deref(),
        Some(Path::new("/home/u/themes/efr.toml"))
    );
    assert_eq!(
        settings("/etc/efr/theme.toml").palette_path(home).as_deref(),
        Some(Path::new("/etc/efr/theme.toml"))
    );
    assert_eq!(RenderSettings::default().palette_path(home), None);
}

use pretty_assertions::assert_eq;

use super::CodeTheme;
use crate::RenderError;

use crate::elements::SAMPLE_TMTHEME as SAMPLE;

#[test]
fn a_tmtheme_file_parses_with_its_name() {
    let theme = CodeTheme::from_tmtheme(SAMPLE).unwrap();
    assert_eq!(theme.name(), Some("efr sample"));
    assert_eq!(theme.syntect().scopes.len(), 4);
}

#[test]
fn clones_are_equal_and_two_parses_are_not() {
    let theme = CodeTheme::from_tmtheme(SAMPLE).unwrap();
    assert_eq!(theme.clone(), theme);
    assert_ne!(CodeTheme::from_tmtheme(SAMPLE).unwrap(), theme);
}

#[test]
fn bytes_that_are_not_a_tmtheme_are_an_error() {
    let error = CodeTheme::from_tmtheme(b"[colors]\naccent = 3\n").unwrap_err();
    assert!(matches!(error, RenderError::CodeTheme { .. }));
    assert_eq!(error.to_string(), "the code theme is not a valid .tmTheme file");
}

#[test]
fn debug_shows_the_name_only() {
    let theme = CodeTheme::from_tmtheme(SAMPLE).unwrap();
    assert_eq!(format!("{theme:?}"), "CodeTheme { name: Some(\"efr sample\"), .. }");
}

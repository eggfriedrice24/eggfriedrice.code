use pretty_assertions::assert_eq;
use unicode_width::UnicodeWidthStr as _;

use super::Glyphs;

#[test]
fn the_unicode_glyphs_are_the_code_points_of_the_legend() {
    let names: Vec<(&str, String)> = Glyphs::UNICODE
        .all()
        .into_iter()
        .map(|(name, glyph)| {
            let points: Vec<String> =
                glyph.chars().map(|c| format!("U+{:04X}", u32::from(c))).collect();
            (name, points.join(" "))
        })
        .collect();
    let want = [
        ("separator", "U+00B7"),
        ("home", "U+2302"),
        ("prompt", "U+203A"),
        ("conversation", "U+21AA"),
        ("switch", "U+21C4"),
        ("arrow", "U+2192"),
        ("moved", "U+2192"),
        ("ok", "U+2713"),
        ("failed", "U+2717"),
        ("refused", "U+0021"),
        ("stopped", "U+0021"),
        ("denied", "U+00B7"),
        ("interrupted", "U+25A0"),
        ("thought", "U+25B8"),
        ("waiting", "U+21B3"),
        ("wrap", "U+005C"),
        ("word_cut", "U+21A9"),
        ("more", "U+2026"),
        ("rule", "U+2500"),
        ("spinner", "U+280B"),
        ("spinner", "U+2819"),
        ("spinner", "U+2839"),
        ("spinner", "U+2838"),
        ("spinner", "U+283C"),
        ("spinner", "U+2834"),
        ("spinner", "U+2826"),
        ("spinner", "U+2827"),
        ("spinner", "U+2807"),
        ("spinner", "U+280F"),
        ("still", "U+2022"),
        ("question", "U+003F"),
        ("shell", "U+0024"),
        ("hunk", "U+0040 U+0040"),
    ];
    let want: Vec<(&str, String)> =
        want.into_iter().map(|(name, points)| (name, points.to_owned())).collect();
    assert_eq!(names, want);
}

#[test]
fn every_unicode_glyph_is_one_cell_and_every_ascii_form_is_ascii() {
    for (name, glyph) in Glyphs::UNICODE.all() {
        // The hunk mark is two characters in both forms; every other glyph is one.
        let cells = if name == "hunk" { 2 } else { 1 };
        assert_eq!(glyph.width(), cells, "{name} {glyph:?}");
    }
    for (name, glyph) in Glyphs::ASCII.all() {
        assert!(glyph.is_ascii() && !glyph.is_empty(), "{name} {glyph:?}");
    }
}

#[test]
fn the_decided_ascii_forms_are_home_tilde_and_prompt_greater_than() {
    assert_eq!(Glyphs::ASCII.home, "~");
    assert_eq!(Glyphs::ASCII.prompt, ">");
    // The mark of a moved call stands in the one-cell scan column.
    assert_eq!(Glyphs::ASCII.moved.width(), 1);
    assert_eq!(*Glyphs::for_locale(true), Glyphs::UNICODE);
    assert_eq!(*Glyphs::for_locale(false), Glyphs::ASCII);
    assert_eq!(Glyphs::default(), Glyphs::UNICODE);
}

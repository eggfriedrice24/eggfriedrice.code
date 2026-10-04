use pretty_assertions::assert_eq;

use super::{
    BLUE, Colour, GREEN, Painter, RED, Span, Style, display_width, expand_tabs, push_span,
    sanitize, to_ansi16,
};
use crate::options::ColourMode;

fn paint(mode: ColourMode, hyperlinks: bool, spans: &[Span]) -> String {
    let mut out = String::new();
    Painter::new(mode, hyperlinks).paint(&mut out, spans);
    out
}

#[test]
fn plain_text_has_no_escapes() {
    assert_eq!(paint(ColourMode::Ansi16, true, &[Span::plain("hello")]), "hello\n");
}

#[test]
fn every_styled_line_resets_at_its_end() {
    let spans = [Span::new("bold", Style::PLAIN.bold()), Span::plain(" then plain")];
    assert_eq!(paint(ColourMode::Ansi16, true, &spans), "\x1b[1mbold\x1b[0m then plain\n");
    let spans = [Span::plain("plain "), Span::new("red", Style::fg(RED))];
    assert_eq!(paint(ColourMode::Ansi16, true, &spans), "plain \x1b[31mred\x1b[0m\n");
}

#[test]
fn attributes_combine_into_one_sgr() {
    let style = Style::fg(GREEN).bold().italic().underline().strike().dim();
    assert_eq!(
        paint(ColourMode::Ansi16, true, &[Span::new("x", style)]),
        "\x1b[1;2;3;4;9;32mx\x1b[0m\n"
    );
}

#[test]
fn bright_cube_and_rgb_colours_have_their_codes() {
    let line = |colour| paint(ColourMode::TrueColor, true, &[Span::new("x", Style::fg(colour))]);
    assert_eq!(line(Colour::Palette(9)), "\x1b[91mx\x1b[0m\n");
    assert_eq!(line(Colour::Palette(200)), "\x1b[38;5;200mx\x1b[0m\n");
    assert_eq!(line(Colour::Rgb(1, 2, 3)), "\x1b[38;2;1;2;3mx\x1b[0m\n");
    let background = Style::PLAIN.on(Colour::Rgb(4, 5, 6));
    assert_eq!(
        paint(ColourMode::TrueColor, true, &[Span::new("x", background)]),
        "\x1b[48;2;4;5;6mx\x1b[0m\n"
    );
}

#[test]
fn no_colour_mode_keeps_attributes_and_drops_colours() {
    let spans = [Span::new("x", Style::fg(RED).bold()), Span::new("y", Style::fg(BLUE))];
    assert_eq!(paint(ColourMode::None, true, &spans), "\x1b[1mx\x1b[0my\n");
}

#[test]
fn sixteen_colour_mode_maps_rgb_to_the_nearest_palette_entry() {
    assert_eq!(to_ansi16(Colour::Rgb(250, 10, 10)), Colour::Palette(9));
    assert_eq!(to_ansi16(Colour::Rgb(0, 190, 0)), Colour::Palette(2));
    assert_eq!(to_ansi16(Colour::Palette(196)), Colour::Palette(9));
    assert_eq!(to_ansi16(Colour::Palette(4)), Colour::Palette(4));
    assert_eq!(to_ansi16(Colour::Palette(232)), Colour::Palette(0));
    let spans = [Span::new("x", Style::fg(Colour::Rgb(250, 10, 10)))];
    assert_eq!(paint(ColourMode::Ansi16, true, &spans), "\x1b[91mx\x1b[0m\n");
}

#[test]
fn hyperlinks_open_and_close_on_the_same_line() {
    let link = Some("https://example.com".to_owned());
    let spans = [Span::plain("see "), Span::linked("site", Style::PLAIN, link), Span::plain(".")];
    assert_eq!(
        paint(ColourMode::Ansi16, true, &spans),
        "see \x1b]8;;https://example.com\x1b\\site\x1b]8;;\x1b\\.\n"
    );
}

#[test]
fn a_hyperlink_at_the_end_of_a_line_is_closed() {
    let link = Some("https://example.com".to_owned());
    let spans = [Span::linked("site", Style::PLAIN.underline(), link)];
    assert_eq!(
        paint(ColourMode::Ansi16, true, &spans),
        "\x1b]8;;https://example.com\x1b\\\x1b[4msite\x1b[0m\x1b]8;;\x1b\\\n"
    );
}

#[test]
fn hyperlinks_off_writes_only_the_text() {
    let spans = [Span::linked("site", Style::PLAIN, Some("https://example.com".to_owned()))];
    assert_eq!(paint(ColourMode::Ansi16, false, &spans), "site\n");
}

#[test]
fn trailing_invisible_spaces_are_dropped() {
    let spans = [Span::plain("a  "), Span::new("   ", Style::fg(RED))];
    assert_eq!(paint(ColourMode::Ansi16, true, &spans), "a\n");
    let spans = [Span::plain("a"), Span::new("  ", Style::PLAIN.underline())];
    assert_eq!(paint(ColourMode::Ansi16, true, &spans), "a\x1b[4m  \x1b[0m\n");
    assert_eq!(paint(ColourMode::Ansi16, true, &[Span::plain("   ")]), "\n");
}

#[test]
fn push_span_merges_equal_neighbours_and_skips_empty_text() {
    let mut line = Vec::new();
    push_span(&mut line, Span::plain("a"));
    push_span(&mut line, Span::plain("b"));
    push_span(&mut line, Span::plain(""));
    push_span(&mut line, Span::new("c", Style::PLAIN.bold()));
    assert_eq!(line, [Span::plain("ab"), Span::new("c", Style::PLAIN.bold())]);
}

#[test]
fn sanitize_makes_control_characters_visible() {
    assert_eq!(sanitize("plain"), "plain");
    assert_eq!(sanitize("a\x1b]52;c;x\x07b"), "a\u{241b}]52;c;x\u{2407}b");
    assert_eq!(sanitize("a\tb\x7f\u{9b}"), "a b\u{2421}\u{fffd}");
}

#[test]
fn expand_tabs_goes_to_the_next_multiple_of_four() {
    assert_eq!(expand_tabs("\tx"), "    x");
    assert_eq!(expand_tabs("ab\tc"), "ab  c");
    assert_eq!(expand_tabs("abcd\te"), "abcd    e");
    assert_eq!(expand_tabs("none"), "none");
}

#[test]
fn display_width_skips_escape_sequences() {
    assert_eq!(display_width("\x1b[1;31mred\x1b[0m"), 3);
    assert_eq!(display_width("\x1b]8;;https://x\x1b\\link\x1b]8;;\x1b\\"), 4);
    assert_eq!(display_width("\x1b]8;;https://x\x07link"), 4);
    assert_eq!(display_width("wide \u{4e16}\u{754c}"), 9);
}

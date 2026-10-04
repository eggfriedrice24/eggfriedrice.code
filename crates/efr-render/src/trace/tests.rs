use pretty_assertions::assert_eq;

use super::render_trace;
use crate::options::{ColourMode, RenderOptions};

#[test]
fn a_trace_is_one_dim_line() {
    assert_eq!(
        render_trace("shell  ls -la\n  /tmp", &RenderOptions::new(80)),
        "\x1b[2mshell ls -la /tmp\x1b[0m\n"
    );
}

#[test]
fn a_long_trace_is_cut_to_the_width() {
    let line = render_trace("abcdefghijklmnop", &RenderOptions::new(10));
    assert_eq!(line, "\x1b[2mabcdefghi\u{2026}\x1b[0m\n");
}

#[test]
fn dim_is_not_colour_so_no_colour_keeps_it() {
    let options = RenderOptions::new(80).with_colour(ColourMode::None);
    assert_eq!(render_trace("read_file a.rs", &options), "\x1b[2mread_file a.rs\x1b[0m\n");
}

#[test]
fn without_a_terminal_the_trace_is_plain() {
    let options = RenderOptions::new(5).with_terminal(false);
    assert_eq!(render_trace("read_file\na.rs", &options), "read_file a.rs\n");
}

#[test]
fn control_characters_are_made_visible() {
    assert_eq!(
        render_trace("x\u{1b}[2Jy", &RenderOptions::new(80)),
        "\x1b[2mx\u{241b}[2Jy\x1b[0m\n"
    );
}

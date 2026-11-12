//! The output of the renderer against the frames of the terminal design
//! (`efr_test_support::design`). The pattern for every slice of the terminal look:
//! build the options of a depth, render mock text at the frame's width, and compare
//! the rows with the frame, cell by cell. One module for each board: `markdown`
//! (TM-Markdown and the Markdown marks of TM-Legend).

mod markdown;

use efr_render::{Colour, ColourMode, Palette, RenderOptions, Role, render};
use efr_test_support::design::{DESIGN_ROLES, Depth, Frame, assert_rows, rgb};

/// The options of `depth` at `cols` columns: the palette of the design in truecolor,
/// the default palette in 16 colours and without colour, as a user with and without
/// the palette file of the design system sees efr.
fn options(depth: Depth, cols: u16) -> RenderOptions {
    let options = RenderOptions::new(cols);
    match depth {
        Depth::TrueColor => {
            let mut palette = Palette::new();
            for (name, hex) in DESIGN_ROLES {
                let (Some(role), Some((r, g, b))) = (Role::from_name(name), rgb(hex)) else {
                    panic!("{name} {hex} is no role of efr-render");
                };
                palette.set(role, Colour::Rgb(r, g, b));
            }
            options.with_colour(ColourMode::TrueColor).with_palette(palette)
        }
        Depth::Ansi16 => options.with_colour(ColourMode::Ansi16),
        Depth::NoColor => options.with_colour(ColourMode::None),
    }
}

/// TM-Legend frame 6, prose: the text in `text` and inline code in `code`, green in
/// truecolor and slot 2 in 16 colours. The Legend draws no `NO_COLOR` frame of it; its
/// note says the backticks stay there, which the Markdown board draws (frame 2).
#[test]
fn prose_with_inline_code_is_the_legend() {
    let frame = Frame::load("TM-Legend", 6).unwrap();
    for depth in [Depth::TrueColor, Depth::Ansi16] {
        let out = render("The steer goes into `pending` while\n", &options(depth, frame.cols));
        assert_rows(&frame, .., &out, depth);
    }
}

#[test]
fn every_role_of_the_design_is_a_role_of_the_palette() {
    let names: Vec<&str> = Role::ALL.iter().map(|role| role.name()).collect();
    let design: Vec<&str> = DESIGN_ROLES.iter().map(|(name, _)| *name).collect();
    assert_eq!(names, design);
}

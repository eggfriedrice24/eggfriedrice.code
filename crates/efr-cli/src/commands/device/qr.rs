//! The pairing QR code as lines of text, for the terminal's own colours.
//!
//! Each line holds two rows of modules in half blocks (`▀`, `▄`, `█` and a space), so
//! the code keeps its square shape in a cell that is twice as high as it is wide. A
//! quiet zone of [`QUIET`] modules goes around it, as the standard asks. efr paints no
//! colour: a module that is lit shows in the terminal's foreground, the others in its
//! background. On a terminal with a light text on a dark background, the light modules
//! are lit, so the code reads dark on light, as a camera expects. `invert` lights the
//! dark modules instead, for a terminal with a dark text on a light background.

use qrcode::{Color, EcLevel, QrCode};

/// The modules of light space around the code.
pub(crate) const QUIET: usize = 4;

/// The lines of the code of `text`, or why `text` gives no code.
pub(crate) fn lines(text: &str, invert: bool) -> Result<Vec<String>, String> {
    // NOTE: level M survives a smudged screen or glare and keeps a pairing URL at
    // version 7 or 8, about 55 columns with the quiet zone.
    let code = QrCode::with_error_correction_level(text.as_bytes(), EcLevel::M)
        .map_err(|error| error.to_string())?;
    let width = code.width();
    let colors = code.to_colors();
    let size = width + 2 * QUIET;
    let inside = QUIET..QUIET + width;
    let lit = |x: usize, y: usize| {
        let light = if inside.contains(&x) && inside.contains(&y) {
            colors[(y - QUIET) * width + (x - QUIET)] == Color::Light
        } else {
            true
        };
        light != invert
    };
    let mut out = Vec::with_capacity(size.div_ceil(2));
    for y in (0..size).step_by(2) {
        let line = (0..size)
            .map(|x| {
                // NOTE: below the last row, when the count is odd, is the terminal's
                // background.
                let bottom = y + 1 < size && lit(x, y + 1);
                match (lit(x, y), bottom) {
                    (true, true) => '\u{2588}',
                    (true, false) => '\u{2580}',
                    (false, true) => '\u{2584}',
                    (false, false) => ' ',
                }
            })
            .collect();
        out.push(line);
    }
    Ok(out)
}

#[cfg(test)]
mod tests;

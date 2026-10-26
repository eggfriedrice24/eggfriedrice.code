use pretty_assertions::assert_eq;

use super::{QUIET, lines};

const URL: &str = "efr://pair?v=1&d=019a9b1c-3d00-7a10-8b20-000000000007&h=100.64.0.1&p=7419\
                   &f=box.tail1234.ts.net&t=q0Zx9mYb3Lk2pR7tW1nV5cHd";

const FULL: char = '\u{2588}';
const UPPER: char = '\u{2580}';
const LOWER: char = '\u{2584}';

#[test]
fn a_pairing_url_fits_an_80_column_window() {
    let code = lines(URL, false).unwrap();

    let width = code[0].chars().count();
    assert!(code.iter().all(|line| line.chars().count() == width));
    assert!(width <= 61, "{width} columns");
    assert_eq!(code.len(), width.div_ceil(2), "two rows of modules in each line");
}

#[test]
fn the_quiet_zone_is_lit_around_the_code() {
    let code = lines(URL, false).unwrap();
    let width = code[0].chars().count();

    for line in &code[..QUIET / 2] {
        assert_eq!(line.chars().filter(|c| *c == FULL).count(), width, "{line:?}");
    }
    for line in &code[QUIET / 2..code.len() - QUIET / 2] {
        let chars: Vec<char> = line.chars().collect();
        assert!(chars[..QUIET].iter().all(|c| *c == FULL), "{line:?}");
        assert!(chars[width - QUIET..].iter().all(|c| *c == FULL), "{line:?}");
    }
    // NOTE: the count of rows is odd, so the last line has its upper half only.
    assert!(code.last().unwrap().chars().all(|c| c == UPPER), "{:?}", code.last());
}

#[test]
fn the_finder_pattern_shows_dark_in_the_corner() {
    let code = lines(URL, false).unwrap();

    // Rows 4 to 10 and columns 4 to 10 hold the top left finder pattern: a dark ring,
    // a light ring and a dark core of 3 by 3. The line of rows 4 and 5 shows the dark
    // top of the ring, and below it the light ring between two dark modules.
    let first: Vec<char> = code[QUIET / 2].chars().collect();
    assert_eq!(first[QUIET], ' ', "{first:?}");
    assert!(first[QUIET + 1..QUIET + 6].iter().all(|c| *c == LOWER), "{first:?}");
    assert_eq!(first[QUIET + 6], ' ', "{first:?}");
    assert_eq!(first[QUIET + 7], FULL, "the separator after the pattern is light");
}

#[test]
fn invert_lights_the_other_modules() {
    let plain = lines(URL, false).unwrap();
    let inverted = lines(URL, true).unwrap();

    let swap = |c: char| match c {
        FULL => ' ',
        ' ' => FULL,
        UPPER => LOWER,
        LOWER => UPPER,
        other => other,
    };
    let last = plain.len() - 1;
    for (at, (plain, inverted)) in plain.iter().zip(&inverted).enumerate().take(last) {
        let swapped: String = plain.chars().map(swap).collect();
        assert_eq!(&swapped, inverted, "line {at}");
    }
    assert!(inverted[last].chars().all(|c| c == ' '), "{:?}", inverted[last]);
}

#[test]
fn a_text_too_long_for_any_code_is_an_error() {
    assert!(lines(&"x".repeat(8_000), false).is_err());
}

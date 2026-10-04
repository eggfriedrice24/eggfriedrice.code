use pretty_assertions::assert_eq;
use pulldown_cmark::Alignment;

use super::Table;
use crate::style::{Span, line_text};

fn table(aligns: Vec<Alignment>, head: &[&str], rows: &[&[&str]]) -> Table {
    let mut table = Table::new(aligns);
    table.start_row();
    for cell in head {
        table.push_cell(vec![Span::plain(*cell)]);
    }
    table.end_head();
    for row in rows {
        table.start_row();
        for cell in *row {
            table.push_cell(if cell.is_empty() { Vec::new() } else { vec![Span::plain(*cell)] });
        }
        table.end_row();
    }
    table
}

fn texts(table: &Table, width: usize) -> Vec<String> {
    table.layout(width).iter().map(|line| line_text(line).trim_end().to_owned()).collect()
}

#[test]
fn a_table_that_fits_is_a_grid() {
    let t = table(vec![Alignment::None; 2], &["name", "size"], &[&["a", "1"], &["bbb", "22"]]);
    let rule = format!("{}\u{253c}{}", "\u{2500}".repeat(5), "\u{2500}".repeat(5));
    assert_eq!(
        texts(&t, 80),
        ["name \u{2502} size", rule.as_str(), "a    \u{2502} 1", "bbb  \u{2502} 22"]
    );
}

#[test]
fn alignment_pads_on_the_right_side() {
    let t = table(
        vec![Alignment::Left, Alignment::Right, Alignment::Center],
        &["l", "r", "c"],
        &[&["xxx", "yyy", "zzzzz"]],
    );
    let lines = texts(&t, 80);
    assert_eq!(lines[0], "l   \u{2502}   r \u{2502}   c");
}

#[test]
fn header_cells_are_bold() {
    let t = table(vec![Alignment::None], &["h"], &[&["v"]]);
    let lines = t.layout(80);
    assert!(lines[0].iter().filter(|span| span.text == "h").all(|span| span.style.bold));
    assert!(lines[2].iter().all(|span| !span.style.bold));
}

#[test]
fn a_table_wider_than_the_width_becomes_records() {
    let t = table(
        vec![Alignment::None; 2],
        &["name", "description"],
        &[&["a", "the first one"], &["b", "the second one"]],
    );
    assert_eq!(
        texts(&t, 20),
        ["name: a", "description: the first one", "", "name: b", "description: the second one"]
    );
}

#[test]
fn the_grid_needs_the_full_width_including_gaps() {
    let t = table(vec![Alignment::None; 2], &["aa", "bb"], &[]);
    assert_eq!(texts(&t, 7)[0], "aa \u{2502} bb");
    assert_eq!(texts(&t, 6), ["aa", "bb"]);
}

#[test]
fn missing_cells_are_empty() {
    let t = table(vec![Alignment::None; 2], &["a", "b"], &[&["1"], &["", "2"]]);
    assert_eq!(texts(&t, 80)[2..], ["1 \u{2502}", "  \u{2502} 2"]);
}

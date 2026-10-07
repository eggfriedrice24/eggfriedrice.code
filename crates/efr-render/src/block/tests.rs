use pretty_assertions::assert_eq;

use super::{Continuation, Ctx, Flow, ListTail, ends_with_blank_line, render_slice};
use crate::code::CodeBlock;
use crate::highlight::ASSETS;
use crate::options::{ColourMode, RenderOptions};
use crate::palette::{Palette, Role};
use crate::style::Colour;

/// Removes CSI and OSC sequences, so a test can look at the visible text.
fn strip(painted: &str) -> String {
    let mut out = String::new();
    let mut chars = painted.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('[') => {
                for c in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&c) {
                        break;
                    }
                }
            }
            Some(']') => {
                while let Some(c) = chars.next() {
                    if c == '\x1b' {
                        chars.next();
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out
}

fn paint_with(markdown: &str, options: RenderOptions, flow: &mut Flow) -> String {
    let ctx = Ctx::new(options, &ASSETS);
    render_slice(&ctx, flow, markdown).0
}

fn paint(markdown: &str, options: RenderOptions) -> String {
    paint_with(markdown, options, &mut Flow::default())
}

fn visible(markdown: &str, width: u16) -> String {
    strip(&paint(markdown, RenderOptions::new(width)))
}

#[test]
fn blocks_are_separated_by_one_blank_line() {
    assert_eq!(visible("one\n\n\n\ntwo\n# three\n", 80), "one\n\ntwo\n\nthree\n");
}

#[test]
fn soft_breaks_keep_the_authors_lines_and_prose_is_not_wrapped() {
    let long = "a long line of prose that is much wider than the twenty columns";
    assert_eq!(visible(&format!("{long}\nsecond line\n"), 20), format!("{long}\nsecond line\n"));
}

#[test]
fn headings_have_one_colour_and_show_their_level_by_weight() {
    let options = RenderOptions::new(80);
    assert_eq!(paint("# One\n", options.clone()), "\x1b[1;4;33mOne\x1b[0m\n");
    assert_eq!(paint("## Two\n", options.clone()), "\x1b[1;33mTwo\x1b[0m\n");
    assert_eq!(paint("### Three\n", options.clone()), "\x1b[1mThree\x1b[0m\n");
    assert_eq!(paint("#### Four\n", options.clone()), "\x1b[1;3mFour\x1b[0m\n");
    assert_eq!(paint("###### Six\n", options.clone()), "\x1b[1;3mSix\x1b[0m\n");
    assert_eq!(paint("Setext\n===\n", options), "\x1b[1;4;33mSetext\x1b[0m\n");
    let no_colour = RenderOptions::new(80).with_colour(ColourMode::None);
    assert_eq!(paint("# One\n", no_colour.clone()), "\x1b[1;4mOne\x1b[0m\n");
    assert_eq!(paint("## Two\n", no_colour), "\x1b[1mTwo\x1b[0m\n");
}

#[test]
fn headings_take_the_accent_or_their_own_colour() {
    let accent = Palette::new().with(Role::Accent, Colour::Palette(5));
    let options = RenderOptions::new(80).with_palette(accent.clone());
    assert_eq!(paint("## Two\n", options), "\x1b[1;35mTwo\x1b[0m\n");
    let own = accent.with(Role::Heading, Colour::Palette(4));
    let options = RenderOptions::new(80).with_palette(own);
    assert_eq!(paint("## Two\n", options.clone()), "\x1b[1;34mTwo\x1b[0m\n");
    assert_eq!(paint("### Three\n", options), "\x1b[1mThree\x1b[0m\n");
}

#[test]
fn emphasis_strong_and_strikethrough_are_sgr() {
    assert_eq!(
        paint("*a* **b** ~~c~~\n", RenderOptions::new(80)),
        "\x1b[3ma\x1b[0m \x1b[1mb\x1b[0m \x1b[9mc\x1b[0m\n"
    );
}

#[test]
fn inline_code_is_coloured_or_keeps_backticks_without_colour() {
    assert_eq!(paint("run `ls`\n", RenderOptions::new(80)), "run \x1b[36mls\x1b[0m\n");
    let no_colour = RenderOptions::new(80).with_colour(ColourMode::None);
    assert_eq!(paint("run `ls`\n", no_colour), "run `ls`\n");
}

#[test]
fn bullets_change_with_nesting() {
    assert_eq!(
        visible("- a\n  - b\n    - c\n      - d\n", 80),
        "\u{2022} a\n  \u{25e6} b\n    \u{25aa} c\n      \u{2022} d\n"
    );
}

#[test]
fn ordered_lists_number_from_their_start() {
    assert_eq!(visible("1. a\n1. b\n1. c\n", 80), "1. a\n2. b\n3. c\n");
    assert_eq!(visible("7. a\n8. b\n", 80), "7. a\n8. b\n");
}

#[test]
fn task_items_show_their_boxes() {
    assert_eq!(visible("- [ ] todo\n- [x] done\n", 80), "[ ] todo\n[x] done\n");
    assert_eq!(visible("1. [x] first\n", 80), "1. [x] first\n");
}

#[test]
fn a_blank_line_between_items_in_the_source_stays() {
    assert_eq!(visible("- a\n\n- b\n- c\n", 80), "\u{2022} a\n\n\u{2022} b\n\u{2022} c\n");
}

#[test]
fn blocks_inside_an_item_follow_each_other_without_blank_lines() {
    assert_eq!(visible("- a\n\n  more\n- b\n", 80), "\u{2022} a\n  more\n\u{2022} b\n");
}

#[test]
fn list_items_wrap_and_keep_their_indent() {
    assert_eq!(
        visible("- one two three four five six seven\n", 20),
        "\u{2022} one two three four\n  five six seven\n"
    );
    assert_eq!(
        visible("10. one two three four five six\n", 20),
        "10. one two three\n    four five six\n"
    );
}

#[test]
fn an_empty_item_still_shows_its_marker() {
    assert_eq!(visible("-\n- b\n", 80), "\u{2022}\n\u{2022} b\n");
}

#[test]
fn quotes_have_a_bar_on_every_line() {
    assert_eq!(
        visible("> one\n>\n> two two two two two two\n", 20),
        "\u{2502} one\n\u{2502}\n\u{2502} two two two two\n\u{2502} two two\n"
    );
    let painted = paint("> q\n", RenderOptions::new(80));
    assert_eq!(painted, "\x1b[2m\u{2502} \x1b[0m\x1b[3mq\x1b[0m\n");
}

#[test]
fn quote_text_is_italic_in_items_too_and_code_in_it_is_not() {
    let painted = paint("> - item\n>\n> ```\n> code\n> ```\n", RenderOptions::new(80));
    assert_eq!(
        painted,
        "\x1b[2m\u{2502} \x1b[0m\u{2022} \x1b[3mitem\x1b[0m\n\
         \x1b[2m\u{2502}\x1b[0m\n\
         \x1b[2m\u{2502} \x1b[0mcode\n"
    );
}

#[test]
fn code_blocks_have_a_muted_label_and_are_never_wrapped() {
    let code = "fn main() { println!(\"a line far wider than twenty columns\"); }";
    let painted = paint(&format!("```rust\n{code}\n```\n"), RenderOptions::new(20));
    assert!(painted.starts_with("\x1b[2mrust\x1b[0m\n"));
    assert_eq!(strip(&painted), format!("rust\n{code}\n"));
}

#[test]
fn a_plain_code_block_has_no_label_and_a_file_name_is_one() {
    assert_eq!(visible("```text\nsome log line\n```\n", 80), "some log line\n");
    assert_eq!(
        visible("```rust src/parse.rs\nfn parse() {}\n```\n", 80),
        "src/parse.rs\nfn parse() {}\n"
    );
}

#[test]
fn code_inside_an_item_is_indented() {
    assert_eq!(
        visible("1. run:\n\n   ```sh\n   ls -l\n   ```\n2. done\n", 80),
        "1. run:\n   sh\n   ls -l\n2. done\n"
    );
}

#[test]
fn indented_code_is_a_code_block() {
    assert_eq!(visible("para\n\n    let x;\n", 80), "para\n\nlet x;\n");
}

#[test]
fn tables_lay_out_as_a_grid_or_as_records() {
    let table = "| a | b |\n|---|--:|\n| x | 10 |\n";
    assert_eq!(
        visible(table, 80),
        "a \u{2502}  b\n\u{2500}\u{2500}\u{253c}\u{2500}\u{2500}\u{2500}\nx \u{2502} 10\n"
    );
    let wide = "| name | description |\n|---|---|\n| a | the first entry |\n| b | the second |\n";
    assert_eq!(
        visible(wide, 20),
        "name: a\ndescription: the first entry\n\nname: b\ndescription: the second\n"
    );
}

#[test]
fn links_are_hyperlinks_or_show_their_target() {
    let markdown = "see [docs](https://example.com/docs)\n";
    assert_eq!(
        paint(markdown, RenderOptions::new(80)),
        "see \x1b]8;;https://example.com/docs\x1b\\\x1b[4;34mdocs\x1b[0m\x1b]8;;\x1b\\\n"
    );
    let off = RenderOptions::new(80).with_hyperlinks(false);
    assert_eq!(strip(&paint(markdown, off.clone())), "see docs (https://example.com/docs)\n");
    assert_eq!(strip(&paint("<https://example.com>\n", off)), "https://example.com\n");
}

#[test]
fn links_without_a_target_show_their_destination() {
    assert_eq!(visible("open [main](src/main.rs)\n", 80), "open main (src/main.rs)\n");
    assert_eq!(visible("[x](javascript:alert(1))\n", 80), "x (javascript:alert(1))\n");
}

#[test]
fn absolute_paths_link_to_files() {
    let painted = paint("[hosts](/etc/hosts) and `/var/log/syslog`\n", RenderOptions::new(80));
    assert!(painted.contains("\x1b]8;;file:///etc/hosts\x1b\\"));
    assert!(painted.contains("\x1b]8;;file:///var/log/syslog\x1b\\"));
}

#[test]
fn bare_urls_become_hyperlinks() {
    let painted = paint("go to https://example.com/a_b now\n", RenderOptions::new(80));
    assert_eq!(
        painted,
        "go to \x1b]8;;https://example.com/a_b\x1b\\\x1b[4;34mhttps://example.com/a_b\x1b]8;;\x1b\\\x1b[0m now\n"
    );
}

#[test]
fn images_show_their_alt_text() {
    assert_eq!(visible("![a cat](https://x/cat.png)\n", 80), "[image: a cat]\n");
    assert_eq!(visible("![](https://x/cat.png)\n", 80), "[image]\n");
    let off = RenderOptions::new(80).with_hyperlinks(false);
    assert_eq!(strip(&paint("![cat](https://x/c.png)\n", off)), "[image: cat] (https://x/c.png)\n");
}

#[test]
fn a_rule_spans_the_width_up_to_forty_columns() {
    assert_eq!(visible("---\n", 10), format!("{}\n", "\u{2500}".repeat(10)));
    assert_eq!(visible("- a\n\n  ***\n", 10), format!("\u{2022} a\n  {}\n", "\u{2500}".repeat(8)));
    assert_eq!(visible("---\n", 120), format!("{}\n", "\u{2500}".repeat(40)));
    assert_eq!(visible("---\n", 2), format!("{}\n", "\u{2500}".repeat(3)));
}

#[test]
fn html_blocks_are_shown_dim() {
    assert_eq!(
        paint("<div>\nhi\n</div>\n", RenderOptions::new(80)),
        "\x1b[2m<div>\x1b[0m\n\x1b[2mhi\x1b[0m\n\x1b[2m</div>\x1b[0m\n"
    );
}

#[test]
fn control_characters_in_prose_are_made_visible() {
    assert_eq!(visible("a\u{1b}]0;title\u{7}b\n", 80), "a\u{241b}]0;title\u{2407}b\n");
}

#[test]
fn a_slice_after_written_output_starts_with_a_blank_line() {
    let mut flow = Flow::default();
    let options = RenderOptions::new(80);
    assert_eq!(paint_with("one\n", options.clone(), &mut flow), "one\n");
    assert_eq!(paint_with("two\n", options, &mut flow), "\ntwo\n");
}

#[test]
fn a_continuing_slice_owes_no_blank_line() {
    let options = RenderOptions::new(80);
    let mut flow = Flow::default();
    paint_with("one\n", options.clone(), &mut flow);
    flow.continuation = Continuation::Paragraph;
    assert_eq!(paint_with("more\n", options, &mut flow), "more\n");
}

#[test]
fn a_continuing_list_keeps_its_numbers_and_spacing() {
    let options = RenderOptions::new(80);
    let mut flow = Flow::default();
    let ctx = Ctx::new(options.clone(), &ASSETS);
    let (first, tail) = render_slice(&ctx, &mut flow, "1. a\n1. b\n");
    assert_eq!(strip(&first), "1. a\n2. b\n");
    assert_eq!(tail, Some(ListTail { next: Some(3) }));
    flow.continuation = Continuation::List { next: Some(3), blank_before: true };
    assert_eq!(strip(&paint_with("1. c\n1. d\n", options, &mut flow)), "\n3. c\n4. d\n");
}

#[test]
fn a_continuing_bullet_list_stays_unnumbered() {
    let mut flow = Flow {
        continuation: Continuation::List { next: None, blank_before: false },
        ..Flow::default()
    };
    assert_eq!(strip(&paint_with("- c\n", RenderOptions::new(80), &mut flow)), "\u{2022} c\n");
}

#[test]
fn a_continuing_code_block_renders_only_its_new_lines() {
    let options = RenderOptions::new(80);
    let ctx = Ctx::new(options.clone(), &ASSETS);
    let mut block = CodeBlock::new("", &ctx.code);
    block.start();
    block.line("first");
    let mut flow = Flow { continuation: Continuation::Code { block, skip: 1 }, ..Flow::default() };
    assert_eq!(paint_with("```\nfirst\nsecond\n```\n", options, &mut flow), "second\n");
}

#[test]
fn blank_line_detection() {
    assert!(ends_with_blank_line("- a\n\n"));
    assert!(ends_with_blank_line("- a\n  \n"));
    assert!(!ends_with_blank_line("- a\n"));
    assert!(!ends_with_blank_line("- a\n  b\n"));
    assert!(!ends_with_blank_line(""));
}

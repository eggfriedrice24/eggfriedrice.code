use std::fmt::Write as _;

use pretty_assertions::assert_eq;
use proptest::prelude::{Just, Strategy, prop, prop_oneof, proptest};

use super::{Renderer, render};
use crate::block::{Ctx, Flow, render_slice};
use crate::elements::{ELEMENTS, element};
use crate::highlight::{ASSETS, Assets};
use crate::options::{ColourMode, RenderOptions, Theme};
use crate::width::WidthMethod;

fn options() -> RenderOptions {
    RenderOptions::new(80)
}

/// Pushes `markdown` cut at `cuts` (byte offsets, moved back to character boundaries)
/// and returns everything committed, finish included.
fn chunked(markdown: &str, cuts: &[usize], options: &RenderOptions) -> String {
    let mut renderer = Renderer::new(options.clone());
    let mut out = String::new();
    let mut from = 0;
    for to in boundaries(markdown, cuts) {
        out.push_str(renderer.push(&markdown[from..to]).committed());
        from = to;
    }
    out.push_str(&renderer.finish());
    out
}

/// True when painted text contains an SGR colour parameter (foreground or
/// background, palette or RGB).
fn has_colour(painted: &str) -> bool {
    painted.split("\x1b[").skip(1).any(|sequence| {
        let Some(params) = sequence.split('m').next() else { return false };
        params.split(';').any(|param| {
            param.parse::<u16>().is_ok_and(|n| (30..=49).contains(&n) || (90..=107).contains(&n))
        })
    })
}

/// The renderer as it was before it followed open fences and kept long live zones:
/// each push renders the whole live zone again.
fn plain(options: &RenderOptions) -> Renderer {
    Renderer::tuned(options.clone(), usize::MAX, false)
}

/// Pushes `markdown` cut at `cuts` into `fast` and into a [`plain`] renderer, and
/// checks that each push commits the same and shows the same live zone. Returns
/// everything committed and whether `fast` followed a fence at some point.
fn same_as_plain(
    markdown: &str,
    cuts: &[usize],
    options: &RenderOptions,
    mut fast: Renderer,
) -> (String, bool) {
    let mut reference = plain(options);
    let mut out = String::new();
    let mut followed = false;
    let mut from = 0;
    for to in boundaries(markdown, cuts) {
        let piece = &markdown[from..to];
        let got = fast.push(piece);
        let want = reference.push(piece);
        assert_eq!(got, want, "after {to} bytes of {markdown:?}");
        followed |= fast.follows_fence();
        out.push_str(got.committed());
        from = to;
    }
    let rest = fast.finish();
    assert_eq!(rest, reference.finish());
    out.push_str(&rest);
    (out, followed)
}

/// `cuts` (byte offsets of `markdown`, moved back to character boundaries) sorted,
/// with the end of the text last.
fn boundaries(markdown: &str, cuts: &[usize]) -> Vec<usize> {
    let mut boundaries: Vec<usize> = cuts
        .iter()
        .map(|cut| {
            let mut at = cut % (markdown.len() + 1);
            while !markdown.is_char_boundary(at) {
                at -= 1;
            }
            at
        })
        .collect();
    boundaries.sort_unstable();
    boundaries.push(markdown.len());
    boundaries
}

/// Every offset of `markdown` that is a multiple of `size`.
fn every(markdown: &str, size: usize) -> Vec<usize> {
    (0..markdown.len()).step_by(size).collect()
}

/// A `1.` item that holds a Rust fence of `lines` lines, as a model writes a step of
/// a plan with its code, and the next item.
fn nested_fence(lines: usize) -> String {
    let mut text = String::from("Here is the change:\n\n1. Change the parser:\n\n   ```rust\n");
    for at in 0..lines {
        let _ = writeln!(text, "   let value_{at} = parse(&input[{at}..], Kind::Item)?;");
        if at % 7 == 3 {
            text.push_str("   /* a comment\n\n      over lines */\n");
        }
    }
    text.push_str("   ```\n\n2. Run the tests.\n");
    text
}

/// A fenced diff of `lines` lines of Rust, as `efr diff` renders one.
fn fenced_diff(lines: usize) -> String {
    let mut text = String::from("```diff\n--- a/src/lib.rs\n+++ b/src/lib.rs\n");
    let _ = writeln!(text, "@@ -1,{lines} +1,{lines} @@");
    for at in 0..lines {
        let mark = [' ', '-', '+'][at % 3];
        let _ = writeln!(text, "{mark}    let value_{at} = parse(&input[{at}..])?; // \"{at}\"");
    }
    text.push_str("```\n");
    text
}

#[test]
fn render_equals_one_push_of_a_long_fenced_diff() {
    let markdown = fenced_diff(20_000);
    let mut renderer = Renderer::new(options());
    let mut pushed = renderer.push(&markdown).into_committed();
    pushed.push_str(&renderer.finish());
    let rendered = render(&markdown, &options());
    assert_eq!(rendered.lines().count(), 20_003);
    assert_eq!(rendered, pushed);
}

#[test]
fn a_long_fence_in_the_last_list_item_streams_as_the_plain_path_does() {
    let markdown = nested_fence(120);
    for options in [options(), options().with_theme(Theme::from_name("nord").unwrap())] {
        let fast = Renderer::tuned(options.clone(), usize::MAX, true);
        let (out, followed) = same_as_plain(&markdown, &every(&markdown, 16), &options, fast);
        assert!(followed, "the renderer did not follow the fence");
        assert_eq!(out, render(&markdown, &options));
        assert!(out.contains("value_119"));
    }
}

#[test]
fn after_a_long_fence_a_partial_line_that_is_not_code_shows_when_it_ends() {
    let markdown = nested_fence(120);
    let (head, rest) = markdown.split_at(markdown.find("2. Run").unwrap());
    let mut renderer = Renderer::new(options());
    let before = renderer.push(head).live().to_owned();
    assert!(renderer.follows_fence());
    let update = renderer.push(&rest[..4]);
    assert_eq!(update.committed(), "");
    assert_eq!(update.live(), before, "the partial line shows late");
    let update = renderer.push(&rest[4..]);
    assert_eq!(update.committed(), before);
    assert_eq!(update.live(), "\n2. Run the tests.\n");
}

#[test]
fn an_open_fence_shows_its_partial_line_at_once() {
    let mut renderer = Renderer::new(options().with_colour(ColourMode::None));
    renderer.push("1. Step:\n\n   ```\n   one\n");
    assert!(renderer.follows_fence());
    let update = renderer.push("   tw");
    assert_eq!(update.committed(), "");
    assert_eq!(update.live(), "1. Step:\n   one\n   tw\n");
    assert_eq!(update.live_rows(), 3);
    // The fence closes on a partial line, and the next item ends the first one.
    assert_eq!(renderer.push("o\n   ``").live(), "1. Step:\n   one\n   two\n   ``\n");
    assert_eq!(renderer.push("`").live(), "1. Step:\n   one\n   two\n");
    assert!(renderer.follows_fence());
    let update = renderer.push("\n2. Next\n");
    assert!(!renderer.follows_fence());
    assert_eq!(update.committed(), "1. Step:\n   one\n   two\n");
    assert_eq!(update.live(), "2. Next\n");
}

#[test]
fn a_fence_in_a_quote_and_a_nested_list_is_followed() {
    let options = options();
    for markdown in [
        "> Look:\n>\n> ```rust\n> fn a() {}\n>\n> fn b() {}\n> ```\nafter\n",
        "> ```\n> code\n> ```\n\nA blank line ends the quote.\n",
        "1. ```\n   code\n   ```\n\n\n   More of the item.\n2. Next\n",
        "- outer\n  1. inner\n     ```python\n     x = 1\n\n     y = 2\n     ```\n- next\n",
        "1. ```\n   first line in the item\n   ```\n",
        "- ~~~~ text\n  ~~~\n  still code\n  ~~~~\n",
        "1. a\n\n\t```\n\tcode with a tab\n\t```\n",
    ] {
        let (out, followed) = same_as_plain(
            markdown,
            &(0..markdown.len()).collect::<Vec<_>>(),
            &options,
            Renderer::new(options.clone()),
        );
        assert!(followed, "{markdown:?}");
        assert_eq!(out, render(markdown, &options), "{markdown:?}");
    }
}

#[test]
fn a_long_open_block_keeps_its_live_zone_until_a_line_is_complete() {
    let mut renderer = Renderer::tuned(options(), 16, true);
    renderer.push("| a | b |\n|---|---|\n| 1 | 2 |\n");
    let update = renderer.push("| 3");
    assert!(!update.live().contains('3'), "the partial row shows late");
    let update = renderer.push(" | 4 |\n");
    assert!(update.live().contains('3') && update.live().contains('4'));
    let update = renderer.push("\n");
    assert!(update.committed().contains('4'));
    assert_eq!(update.live(), "");
    // Below the size, the partial line shows at once.
    let mut renderer = Renderer::new(options());
    renderer.push("| a | b |\n|---|---|\n");
    assert!(renderer.push("| 3").live().contains('3'));
}

#[test]
fn render_equals_push_then_finish() {
    let markdown = element("bullet_list");
    let mut renderer = Renderer::new(options());
    let mut out = renderer.push(markdown).into_committed();
    out.push_str(&renderer.finish());
    assert_eq!(out, render(markdown, &options()));
}

#[test]
fn a_partial_line_is_never_committed() {
    let mut renderer = Renderer::new(options());
    let update = renderer.push("# Tit");
    assert_eq!(update.committed(), "");
    assert_eq!(update.live(), "\x1b[1;4;33mTit\x1b[0m\n");
    let update = renderer.push("le\n");
    assert_eq!(update.committed(), "\x1b[1;4;33mTitle\x1b[0m\n");
    assert_eq!(update.live(), "");
}

#[test]
fn a_paragraph_commits_at_its_end() {
    let mut renderer = Renderer::new(options());
    let update = renderer.push("one\ntwo\n");
    assert_eq!(update.committed(), "");
    assert_eq!(update.live(), "one\ntwo\n");
    let update = renderer.push("\n");
    assert_eq!(update.committed(), "one\ntwo\n");
    assert_eq!(update.live(), "");
    let update = renderer.push("next");
    assert_eq!(update.committed(), "");
    assert_eq!(update.live(), "\nnext\n");
}

#[test]
fn a_growing_paragraph_commits_its_earlier_lines() {
    let mut renderer = Renderer::new(options());
    assert_eq!(renderer.push("a\nb\nc\n").committed(), "");
    let update = renderer.push("d\n");
    assert_eq!(update.committed(), "a\nb\nc\n");
    assert_eq!(update.live(), "d\n");
    assert_eq!(renderer.finish(), "d\n");
}

#[test]
fn code_blocks_commit_line_by_line() {
    let mut renderer = Renderer::new(options().with_colour(ColourMode::None));
    assert_eq!(renderer.push("```rust\n").committed(), "\x1b[2mrust\x1b[0m\n");
    let update = renderer.push("fn a() {}\nfn b");
    assert_eq!(update.committed(), "fn a() {}\n");
    assert_eq!(update.live(), "fn b\n");
    assert_eq!(renderer.push("() {}\n").committed(), "fn b() {}\n");
    assert_eq!(renderer.push("```\n").committed(), "");
    assert_eq!(renderer.push("after\n").committed(), "");
    assert_eq!(renderer.finish(), "\nafter\n");
}

#[test]
fn a_streamed_code_block_still_ends_at_its_fence() {
    let options = options().with_colour(ColourMode::None);
    let markdown = "```\none\ntwo\n```\nafter\n\n```\nthree\n";
    assert_eq!(render(markdown, &options), "one\ntwo\n\nafter\n\nthree\n");
    assert_eq!(
        chunked(markdown, &(0..markdown.len()).collect::<Vec<_>>(), &options),
        render(markdown, &options)
    );
}

#[test]
fn a_table_commits_only_when_complete() {
    let mut renderer = Renderer::new(options());
    assert_eq!(renderer.push("| a | b |\n|---|---|\n| 1 | 2 |\n").committed(), "");
    let update = renderer.push("| 333 | 4 |\n");
    assert_eq!(update.committed(), "");
    assert!(update.live().contains("333"));
    let update = renderer.push("\n");
    assert!(update.committed().contains("333"));
    assert_eq!(update.live(), "");
}

#[test]
fn list_items_commit_when_the_next_one_starts() {
    let mut renderer = Renderer::new(options());
    assert_eq!(renderer.push("1. a\n").committed(), "");
    assert_eq!(renderer.push("   more\n").committed(), "");
    let update = renderer.push("1. b\n");
    assert_eq!(update.committed(), "1. a\n   more\n");
    assert_eq!(update.live(), "2. b\n");
    let update = renderer.push("\n1. c\n");
    assert_eq!(update.committed(), "2. b\n");
    assert_eq!(update.live(), "\n3. c\n");
    assert_eq!(renderer.finish(), "\n3. c\n");
}

#[test]
fn the_live_zone_is_what_finishing_now_would_commit() {
    let markdown = "# T\n\nsome *text*\n- item\n";
    for end in 0..=markdown.len() {
        let mut renderer = Renderer::new(options());
        let live = renderer.push(&markdown[..end]).live().to_owned();
        assert_eq!(live, renderer.finish(), "after {end} bytes");
    }
}

#[test]
fn live_rows_count_wrapped_rows() {
    let mut renderer = Renderer::new(RenderOptions::new(10));
    let update = renderer.push("0123456789abc\nshort");
    assert_eq!(update.live_rows(), 3);
    assert_eq!(update.live_tail(1), "short\n");
    assert_eq!(update.live_tail(2), "short\n");
    assert_eq!(update.live_tail(3), update.live());
    assert_eq!(update.live_tail(0), "");
}

/// The rows of a live zone of ten `piece`s and a space each, 20 columns wide, when the
/// terminal counts by `method`.
fn live_rows_of(piece: &str, method: WidthMethod) -> usize {
    let options = RenderOptions::new(20).with_width_method(method);
    let mut renderer = Renderer::new(options);
    let update = renderer.push(&format!("{piece} ").repeat(10));
    assert_eq!(update.live_tail(update.live_rows()), update.live());
    assert_eq!(update.live_tail(update.live_rows() - 1), "");
    update.live_rows()
}

#[test]
fn live_rows_count_as_the_terminal_counts_widths() {
    let family = "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}";
    let heart = "\u{2764}\u{fe0f}";
    let flag = "\u{1f1fa}\u{1f1f8}";
    // 69 columns by code point, 29 by grapheme cluster.
    assert_eq!(live_rows_of(family, WidthMethod::CodePoint), 4);
    assert_eq!(live_rows_of(family, WidthMethod::Grapheme), 2);
    // 19 columns by code point, 29 by grapheme cluster.
    assert_eq!(live_rows_of(heart, WidthMethod::CodePoint), 1);
    assert_eq!(live_rows_of(heart, WidthMethod::Grapheme), 2);
    // 29 columns both ways.
    assert_eq!(live_rows_of(flag, WidthMethod::CodePoint), 2);
    assert_eq!(live_rows_of(flag, WidthMethod::Grapheme), 2);
}

#[test]
fn without_a_terminal_markdown_passes_through_unchanged() {
    let options = options().with_terminal(false);
    let mut renderer = Renderer::new(options.clone());
    let update = renderer.push("# Ti");
    assert_eq!(update.committed(), "# Ti");
    assert_eq!(update.live(), "");
    assert_eq!(update.live_rows(), 0);
    assert_eq!(renderer.push("tle\n\x1b[2J").committed(), "tle\n\x1b[2J");
    assert_eq!(renderer.finish(), "");
    let document: String = ELEMENTS.iter().map(|(_, markdown)| *markdown).collect();
    assert_eq!(render(&document, &options), document);
}

#[test]
fn no_colour_mode_writes_no_colour_codes() {
    let document: String = ELEMENTS.iter().map(|(_, markdown)| *markdown).collect();
    let coloured = render(&document, &options());
    assert!(has_colour(&coloured));
    let plain = render(&document, &options().with_colour(ColourMode::None));
    assert!(!has_colour(&plain));
    assert!(plain.contains("\x1b[1m"), "bold stays without colour");
    let mocha = Theme::from_name("catppuccin-mocha").unwrap();
    let truecolor = options().with_colour(ColourMode::None).with_theme(mocha);
    assert!(!has_colour(&render(&document, &truecolor)));
}

#[test]
fn prose_loads_no_grammar_and_the_first_code_block_does() {
    let assets: &'static Assets = Box::leak(Box::new(Assets::new()));
    let mut renderer = Renderer::with_assets(options(), assets);
    renderer.push("# Title\n\nSome prose, a [link](https://x.io) and `code`.\n\n");
    assert!(!assets.grammars_loaded());
    renderer.push("```rust\nfn x() {}\n");
    assert!(assets.grammars_loaded());
}

#[test]
fn debug_output_leaves_out_the_reply() {
    let mut renderer = Renderer::new(options());
    renderer.push("secret reply text");
    let debug = format!("{renderer:?}");
    assert!(!debug.contains("secret"));
    assert!(debug.contains("pending_bytes: 17"));
}

/// Fragments that stress the commit rules on top of the element samples: blocks that
/// change meaning when a later line arrives, and blocks without their closing line.
const TRICKY: &[&str] = &[
    "Title\n---\n",
    "para\n| a | b |\n|---|---|\n| 1 | 2 |\n",
    "one\ntwo\nthree\nfour\nfive\n2. not a list\n    indented\nsix\n",
    "- a\n\n- b\n  continued\n\n  second paragraph\n- c\n",
    "1. x\n1. y\n\n1. z\n",
    "> quote\nlazy continuation\n\n> another\n",
    "    indented code\n\n    more code\n",
    "```\nunclosed fence\n",
    "~~~python\nprint('tilde')\n~~~\n",
    "[ref]\n\n[ref]: https://example.com\n",
    "**bold across\nlines**\n",
    "text with trailing spaces  \nhard break\n",
    "<!-- comment -->\n",
    "* * *\n",
    "- [ ] a\n- [x] b\n",
    "crlf line\r\nsecond\r\n\r\nnext\r\n",
    "\u{4e16}\u{754c} wide text and an emoji-free line\n",
];

fn document() -> impl Strategy<Value = String> {
    let fragment = prop_oneof![
        prop::sample::select(ELEMENTS.iter().map(|(_, markdown)| *markdown).collect::<Vec<_>>()),
        prop::sample::select(TRICKY.to_vec()),
    ];
    let separator = prop::sample::select(vec!["", "\n", "\n\n"]);
    prop::collection::vec((fragment, separator), 1..6).prop_map(|parts| {
        parts.into_iter().map(|(fragment, separator)| format!("{fragment}{separator}")).collect()
    })
}

fn noise() -> impl Strategy<Value = String> {
    let alphabet = prop::sample::select(vec![
        "a",
        "b",
        " ",
        "\n",
        "\n",
        "#",
        "*",
        "_",
        "`",
        "-",
        "+",
        ">",
        "|",
        "[",
        "]",
        "(",
        ")",
        "!",
        "1",
        ".",
        ":",
        "~",
        "\t",
        "<",
        "/",
        "x",
        "=",
        "```",
        "    ",
        "https://e.io",
    ]);
    prop::collection::vec(alphabet, 0..80).prop_map(|pieces| pieces.concat())
}

fn any_options() -> impl Strategy<Value = RenderOptions> {
    let width = prop::sample::select(vec![20_u16, 40, 80]);
    let colour =
        prop_oneof![Just(ColourMode::None), Just(ColourMode::Ansi16), Just(ColourMode::TrueColor)];
    let theme = prop::sample::select(vec!["ansi", "catppuccin-mocha"]);
    (width, colour, theme, proptest::bool::ANY).prop_map(|(width, colour, theme, hyperlinks)| {
        RenderOptions::new(width)
            .with_colour(colour)
            .with_theme(Theme::from_name(theme).unwrap_or_default())
            .with_hyperlinks(hyperlinks)
    })
}

/// A block that holds a fence, as its opening lines, and the lines that can follow:
/// code, blank lines, closing fences that match or not, lines that end the block, and
/// lines with tabs and carriage returns.
fn fenced_block() -> impl Strategy<Value = String> {
    let opening = prop::sample::select(vec![
        "1. Change it:\n\n   ```rust\n",
        "- ```\n",
        "- ```diff\n",
        "1. Step\n   ~~~~ python\n",
        "> Look:\n>\n> ```rust\n",
        "> ```\n",
        "- outer\n  1. inner\n\n     ```toml\n",
        "1. a\n\n\t```sh\n",
        "Para\n\n10. ```text\n",
    ]);
    let line = prop::sample::select(vec![
        "   let x = 1;\n",
        "   /* open\n",
        "   still a comment */\n",
        "     deeper\n",
        "\n",
        "   \n",
        "> fn quoted() {}\n",
        ">\n",
        "  - not an item\n",
        "   ```\n",
        "   ````\n",
        "   ~~~\n",
        "   ~~~~\n",
        "     ```\n",
        "```\n",
        "> ```\n",
        "2. next item\n",
        "- other list\n",
        "unindented text\n",
        "\tcode after a tab\n",
        "   crlf line\r\n",
        "   a lone\rcarriage return\n",
        "   diff --git a/x.rs b/x.rs\n",
        "   +added\n",
    ]);
    (
        opening,
        prop::collection::vec(line, 0..14),
        prop::sample::select(vec!["", "   tail", "   ``"]),
    )
        .prop_map(|(opening, lines, tail)| format!("{opening}{}{tail}", lines.concat()))
}

proptest! {
    /// Following an open fence and keeping its live zone changes nothing: each push
    /// commits and shows what the plain path does.
    #[test]
    fn following_fences_commits_and_shows_what_the_plain_path_does(
        markdown in prop_oneof![fenced_block(), document(), noise()],
        cuts in prop::collection::vec(proptest::num::usize::ANY, 0..24),
        options in any_options(),
    ) {
        let fast = Renderer::tuned(options.clone(), usize::MAX, true);
        let (out, _) = same_as_plain(&markdown, &cuts, &options, fast);
        assert_eq!(out, render(&markdown, &options));
    }

    #[test]
    fn byte_by_byte_streaming_of_a_fence_shows_what_the_plain_path_does(
        markdown in fenced_block(),
        options in any_options(),
    ) {
        let cuts: Vec<usize> = (0..markdown.len()).collect();
        let fast = Renderer::tuned(options.clone(), usize::MAX, true);
        let (out, _) = same_as_plain(&markdown, &cuts, &options, fast);
        assert_eq!(out, render(&markdown, &options));
    }

    /// A long open block keeps its live zone between lines, and that changes no
    /// commit.
    #[test]
    fn keeping_live_zones_commits_what_render_does(
        markdown in prop_oneof![fenced_block(), document(), noise()],
        cuts in prop::collection::vec(proptest::num::usize::ANY, 0..24),
        options in any_options(),
    ) {
        let mut renderer = Renderer::tuned(options.clone(), 0, true);
        let mut out = String::new();
        let mut from = 0;
        for to in boundaries(&markdown, &cuts) {
            out.push_str(renderer.push(&markdown[from..to]).committed());
            from = to;
        }
        out.push_str(&renderer.finish());
        assert_eq!(out, render(&markdown, &options));
    }

    #[test]
    fn any_chunking_of_a_document_commits_what_render_does(
        markdown in document(),
        cuts in prop::collection::vec(proptest::num::usize::ANY, 0..12),
        options in any_options(),
    ) {
        assert_eq!(chunked(&markdown, &cuts, &options), render(&markdown, &options));
    }

    #[test]
    fn any_chunking_of_markdown_noise_commits_what_render_does(
        markdown in noise(),
        cuts in prop::collection::vec(proptest::num::usize::ANY, 0..12),
        options in any_options(),
    ) {
        assert_eq!(chunked(&markdown, &cuts, &options), render(&markdown, &options));
    }

    /// Committing early (paragraph lines, list items, code lines) must not change what
    /// the elements look like: streamed, they render as one parse of the whole
    /// document renders them.
    #[test]
    fn streaming_renders_elements_as_one_whole_parse_does(
        parts in prop::collection::vec(
            prop::sample::select(ELEMENTS.iter().map(|(_, markdown)| *markdown).collect::<Vec<_>>()),
            1..6,
        ),
        options in any_options(),
    ) {
        let markdown = parts.join("\n");
        let ctx = Ctx::new(options.clone(), &ASSETS);
        let (whole, _) = render_slice(&ctx, &mut Flow::default(), &markdown);
        assert_eq!(render(&markdown, &options), whole);
    }

    #[test]
    fn byte_by_byte_streaming_commits_what_render_does(markdown in document()) {
        let cuts: Vec<usize> = (0..markdown.len()).collect();
        assert_eq!(chunked(&markdown, &cuts, &options()), render(&markdown, &options()));
    }
}

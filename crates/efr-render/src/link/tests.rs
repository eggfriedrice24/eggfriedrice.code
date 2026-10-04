use pretty_assertions::assert_eq;

use super::{code_target, encode, file_url, find_urls, link_target};

fn urls(text: &str) -> Vec<&str> {
    find_urls(text).into_iter().map(|range| &text[range]).collect()
}

#[test]
fn links_with_allowed_schemes_are_targets() {
    assert_eq!(
        link_target("https://example.com/a?b=c").as_deref(),
        Some("https://example.com/a?b=c")
    );
    assert_eq!(link_target("HTTP://EXAMPLE.COM").as_deref(), Some("HTTP://EXAMPLE.COM"));
    assert_eq!(link_target("mailto:me@example.com").as_deref(), Some("mailto:me@example.com"));
}

#[test]
fn other_schemes_and_relative_paths_are_not_targets() {
    assert_eq!(link_target("javascript:alert(1)"), None);
    assert_eq!(link_target("src/main.rs"), None);
    assert_eq!(link_target("#heading"), None);
    assert_eq!(link_target("//example.com"), None);
    assert_eq!(link_target("https://"), None);
}

#[test]
fn absolute_paths_become_file_urls() {
    assert_eq!(link_target("/etc/hosts").as_deref(), Some("file:///etc/hosts"));
    assert_eq!(file_url("/tmp/a b/c%d.rs"), "file:///tmp/a%20b/c%25d.rs");
    assert_eq!(file_url("/src/main.rs:42"), "file:///src/main.rs");
    assert_eq!(file_url("/src/main.rs:42:7"), "file:///src/main.rs");
    assert_eq!(file_url("/src/a:b"), "file:///src/a%3Ab");
}

#[test]
fn encoding_leaves_nothing_that_ends_an_escape_sequence() {
    assert_eq!(encode("https://x/\x1b\\\x07 y"), "https://x/%1B\\%07%20y");
    assert_eq!(encode("https://x/\u{e9}"), "https://x/%C3%A9");
}

#[test]
fn inline_code_targets_whole_urls_and_paths_with_two_components() {
    assert_eq!(code_target("https://example.com").as_deref(), Some("https://example.com"));
    assert_eq!(code_target("/etc/hosts").as_deref(), Some("file:///etc/hosts"));
    assert_eq!(
        code_target("/home/me/src/main.rs:3").as_deref(),
        Some("file:///home/me/src/main.rs")
    );
    assert_eq!(code_target("/help"), None);
    assert_eq!(code_target("/tmp/"), None);
    assert_eq!(code_target("cargo build"), None);
    assert_eq!(code_target("src/main.rs"), None);
    assert_eq!(code_target(""), None);
}

#[test]
fn bare_urls_are_found_in_prose() {
    assert_eq!(urls("see https://example.com/x for more"), ["https://example.com/x"]);
    assert_eq!(urls("two: http://a.io and https://b.io/c"), ["http://a.io", "https://b.io/c"]);
}

#[test]
fn trailing_punctuation_is_not_part_of_a_url() {
    assert_eq!(urls("at https://example.com."), ["https://example.com"]);
    assert_eq!(urls("(see https://example.com/a)"), ["https://example.com/a"]);
    assert_eq!(
        urls("https://en.wikipedia.org/wiki/A_(b))."),
        ["https://en.wikipedia.org/wiki/A_(b)"]
    );
    assert_eq!(urls("<https://example.com>"), ["https://example.com"]);
}

#[test]
fn text_that_only_resembles_a_url_is_left_alone() {
    assert_eq!(urls("http and https://"), Vec::<&str>::new());
    assert_eq!(urls("xhttps://example.com"), Vec::<&str>::new());
    assert_eq!(urls("the httpd daemon"), Vec::<&str>::new());
}

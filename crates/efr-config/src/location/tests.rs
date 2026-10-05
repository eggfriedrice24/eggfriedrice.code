use pretty_assertions::assert_eq;
use toml_edit::Document;

use super::{key_at, location, span_of, span_of_rule};
use crate::Location;

const TEXT: &str = "\
log = \"info\"

[shell]
login = true # a comment
idle_minutes = 5

[[permissions.rules]]
action = \"read\"
resource = { under = \"~/x\" }
effect = \"allow\"

[[permissions.rules]]
action = \"write\"
resource = \"any\"
effect = \"deny\"
";

fn document() -> Document<String> {
    Document::parse(TEXT.to_owned()).unwrap()
}

fn offset_of(needle: &str) -> usize {
    TEXT.find(needle).unwrap()
}

#[test]
fn an_offset_becomes_a_line_and_a_column_counted_from_one() {
    assert_eq!(location(TEXT, 0), Location { line: 1, column: 1 });
    assert_eq!(location(TEXT, offset_of("idle_minutes")), Location { line: 5, column: 1 });
    assert_eq!(location(TEXT, offset_of("true")), Location { line: 4, column: 9 });
    assert_eq!(location("é = 1", 3), Location { line: 1, column: 3 }, "columns count characters");
    assert_eq!(location(TEXT, usize::MAX).line, 16);
}

#[test]
fn the_key_at_an_offset_is_the_deepest_one() {
    let document = document();

    assert_eq!(key_at(&document, offset_of("info")).as_deref(), Some("log"));
    assert_eq!(key_at(&document, offset_of("idle_minutes")).as_deref(), Some("shell.idle_minutes"));
    assert_eq!(key_at(&document, offset_of("5\n")).as_deref(), Some("shell.idle_minutes"));
    assert_eq!(
        key_at(&document, offset_of("\"~/x\"")).as_deref(),
        Some("permissions.rules[0].resource.under")
    );
    assert_eq!(
        key_at(&document, offset_of("\"deny\"")).as_deref(),
        Some("permissions.rules[1].effect")
    );
    assert_eq!(key_at(&document, offset_of("# a comment")), None);
}

#[test]
fn the_span_of_a_key_and_of_a_rule_is_found() {
    let document = document();

    let login = span_of(&document, "shell.login").unwrap();
    assert_eq!(&TEXT[login], "true");
    assert!(span_of(&document, "shell.program").is_none());
    assert!(span_of(&document, "log").is_some());
    let rule = span_of_rule(&document, 1).unwrap();
    assert_eq!(location(TEXT, rule.start).line, 12);
    assert!(span_of_rule(&document, 2).is_none());
}

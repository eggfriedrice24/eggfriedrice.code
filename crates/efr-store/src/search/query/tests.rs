use pretty_assertions::assert_eq;
use proptest::prelude::*;

use super::*;

fn fts(query: &str) -> Option<String> {
    parse(query).unwrap().fts
}

#[test]
fn parser_table() {
    let cases: &[(&str, Option<&str>)] = &[
        ("", None),
        ("   ", None),
        ("rsync", Some("\"rsync\"*")),
        ("rsync lap", Some("\"rsync\" AND \"lap\"*")),
        ("rsync lap ", Some("\"rsync\" AND \"lap\"")),
        ("rsync la", Some("\"rsync\" AND \"la\"")),
        ("src/a", Some("\"src/a\"")),
        ("efr-c", Some("\"efr-c\"")),
        ("efr-co", Some("\"efr-co\"")),
        ("efr-con", Some("\"efr-con\"*")),
        ("foo_b", Some("\"foo_b\"")),
        ("a.b", None),
        ("x y", None),
        ("\"x y\"", None),
        ("\"rsync laptop\"", Some("\"rsync laptop\"")),
        ("\"rsync lap", Some("\"rsync lap\"*")),
        ("say \"hi there\" now", Some("\"say\" AND \"hi there\" AND \"now\"*")),
        ("a\"b\"c", None),
        ("we\"ird", Some("\"we\" AND \"ird\"*")),
        ("it's", Some("\"it's\"")),
        ("don\"\"t", Some("\"don\"")),
        ("café", Some("\"café\"*")),
        ("日本語", Some("\"日本語\"*")),
        ("a docker", Some("\"docker\"*")),
        ("docker a", Some("\"docker\"")),
        ("-", None),
        ("abc\0def", Some("\"abc\" AND \"def\"*")),
        ("\"ab\0cd\"", Some("\"ab cd\"")),
        ("rsync\0", Some("\"rsync\"")),
        ("\u{1b}[2J", Some("\"[2J\"")),
    ];
    for (query, expected) in cases {
        assert_eq!(fts(query).as_deref(), *expected, "query {query:?}");
    }
}

#[test]
fn short_terms_go_to_the_title_only() {
    let query = parse("a docker b").unwrap();
    assert_eq!(query.words(), vec!["a", "docker", "b"]);
    let indexed: Vec<bool> = query.terms.iter().map(|term| term.indexed).collect();
    assert_eq!(indexed, vec![false, true, false]);
    assert_eq!(query.fts.as_deref(), Some("\"docker\""));
}

#[test]
fn a_quote_never_reaches_a_term() {
    // A `"` ends a word and starts a phrase, so no term holds one; the parser doubles
    // one anyway, so the FTS5 string stays valid if that rule changes.
    let query = parse("say\"").unwrap();
    assert!(query.terms.iter().all(|term| !term.text.contains('"')));
    assert_eq!(query.fts.as_deref(), Some("\"say\"*"));
}

#[test]
fn limits() {
    let long = "a".repeat(MAX_CHARS + 1);
    assert_eq!(parse(&long), Err(QueryError::TooLong { chars: MAX_CHARS + 1 }));
    assert!(parse(&"a".repeat(MAX_CHARS)).is_ok());
    let surrounded = format!("  {}  ", "a".repeat(MAX_CHARS));
    assert!(parse(&surrounded).is_ok(), "white space around the query does not count");
    let many = vec!["word"; MAX_TERMS + 1].join(" ");
    assert_eq!(parse(&many), Err(QueryError::TooManyTerms { terms: MAX_TERMS + 1 }));
    let sixteen = vec!["word"; MAX_TERMS].join(" ");
    assert_eq!(parse(&sixteen).unwrap().terms.len(), MAX_TERMS);
    let multibyte = "é".repeat(MAX_CHARS);
    assert!(parse(&multibyte).is_ok(), "the limit counts code points, not bytes");
}

#[test]
fn tokens_follow_unicode61() {
    assert_eq!(tokens("src/efr-cli/main.rs"), vec!["src", "efr", "cli", "main", "rs"]);
    assert_eq!(tokens("snake_case"), vec!["snake", "case"]);
    assert_eq!(tokens("Grüße 42"), vec!["Grüße", "42"]);
    assert!(tokens("--- ...").is_empty());
}

proptest! {
    #[test]
    fn parser_never_panics_and_has_at_most_one_prefix(query in "\\PC{0,300}") {
        if let Ok(parsed) = parse(&query) {
            let fts = parsed.fts.unwrap_or_default();
            prop_assert!(fts.is_empty() || parsed.terms.iter().any(|term| term.indexed));
            prop_assert!(parsed.terms.iter().filter(|term| term.prefix).count() <= 1);
            prop_assert!(parsed.terms.len() <= MAX_TERMS);
        }
    }

    #[test]
    fn no_control_character_reaches_a_term(query in "[a-z\\x00-\\x1f\\x7f\" ]{0,40}") {
        let parsed = parse(&query).unwrap();
        prop_assert!(parsed.terms.iter().all(|term| !term.text.contains(char::is_control)));
        prop_assert!(!parsed.fts.unwrap_or_default().contains(char::is_control));
    }

    #[test]
    fn quote_heavy_queries_never_panic(query in "[a-z\" ]{0,40}") {
        let _ = parse(&query);
    }
}

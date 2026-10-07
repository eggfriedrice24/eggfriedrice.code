use efr_config::Progress;
use pretty_assertions::assert_eq;

use super::{version, wanted};
use crate::terminal::TermFacts;
use crate::testing::terminal_facts;

/// A terminal with these `TERM_PROGRAM` and `TERM_PROGRAM_VERSION`, behind a multiplexer
/// or not, and with `WT_SESSION` or not.
fn facts(program: Option<&str>, version: Option<&str>, multiplexer: bool, wt: bool) -> TermFacts {
    TermFacts {
        term_program: program.map(str::to_owned),
        term_program_version: version.map(str::to_owned),
        multiplexer,
        wt_session: wt,
        ..terminal_facts()
    }
}

#[test]
fn auto_sends_the_bar_only_to_terminals_known_to_draw_it_and_never_through_a_multiplexer() {
    let cases = [
        (Some("ghostty"), Some("1.3.1"), false, false, true),
        (Some("ghostty"), Some("1.2.0"), false, false, true),
        (Some("ghostty"), Some("1.2.0-main+5d6a8e5"), false, false, true),
        (Some("ghostty"), Some("1.1.3"), false, false, false),
        (Some("ghostty"), None, false, false, false),
        (Some("ghostty"), Some("1.3.1"), true, false, false),
        (Some("kitty"), Some("0.47.0"), false, false, true),
        (Some("kitty"), Some("0.46.2"), false, false, false),
        (Some("WezTerm"), Some("20240203"), false, false, false),
        (Some("tmux"), Some("3.5a"), true, false, false),
        // GNU screen and zellij keep the outer TERM_PROGRAM.
        (Some("ghostty"), Some("1.3.1"), true, false, false),
        (Some("kitty"), Some("0.47.0"), true, false, false),
        (None, None, false, true, true),
        (None, None, true, true, false),
        (Some("vscode"), Some("1.99.0"), false, false, false),
        (None, None, false, false, false),
    ];
    for (program, version, multiplexer, wt, expected) in cases {
        let facts = facts(program, version, multiplexer, wt);
        assert_eq!(wanted(Progress::Auto, &facts), expected, "{facts:?}");
        assert!(wanted(Progress::On, &facts), "on sends it everywhere: {facts:?}");
        assert!(!wanted(Progress::Off, &facts), "off never sends it: {facts:?}");
    }
}

#[test]
fn nothing_goes_out_when_stdout_is_not_a_terminal() {
    let ghostty = facts(Some("ghostty"), Some("1.3.1"), false, false);
    let piped = TermFacts { stdout_tty: false, ..ghostty.clone() };
    let dumb = TermFacts { term: Some("dumb".to_owned()), ..ghostty };
    for facts in [piped, dumb] {
        assert!(!wanted(Progress::Auto, &facts));
        assert!(!wanted(Progress::On, &facts));
    }
}

#[test]
fn versions_compare_by_their_first_three_numbers() {
    assert_eq!(version("1.3.1"), [1, 3, 1]);
    assert_eq!(version("1.2"), [1, 2, 0]);
    assert_eq!(version("0.47.0-dev+abc"), [0, 47, 0]);
    assert_eq!(version("tip"), [0, 0, 0]);
}

use pretty_assertions::assert_eq;

use crate::records::Records;
use crate::state::{SandboxState, quote};

#[test]
fn functions_and_exports_stay_and_dropped_names_never_enter() {
    let mut state = SandboxState::default();
    state.apply(&Records {
        exports: vec![
            ("VIRTUAL_ENV".to_owned(), "/home/u/p/app/.venv".to_owned()),
            ("PATH".to_owned(), "/home/u/p/app/.venv/bin:/usr/bin".to_owned()),
            ("LD_PRELOAD".to_owned(), "/tmp/x.so".to_owned()),
            ("bad name".to_owned(), "x".to_owned()),
        ],
        functions: vec![
            ("deactivate".to_owned(), "unset VIRTUAL_ENV".to_owned()),
            ("_efr_hs_sbx".to_owned(), "evil".to_owned()),
        ],
        aliases: vec![("ll".to_owned(), "ls -l".to_owned())],
        ..Records::default()
    });
    assert_eq!(state.exports.keys().collect::<Vec<_>>(), ["PATH", "VIRTUAL_ENV"]);
    assert_eq!(state.functions.keys().collect::<Vec<_>>(), ["deactivate"]);

    state.apply(&Records {
        unsets: vec!["VIRTUAL_ENV".to_owned()],
        removed_functions: vec!["deactivate".to_owned()],
        removed_aliases: vec!["ll".to_owned()],
        ..Records::default()
    });
    assert!(!state.exports.contains_key("VIRTUAL_ENV"));
    assert!(state.unsets.contains("VIRTUAL_ENV"));
    assert!(state.functions.is_empty());
    assert!(state.removed_functions.contains("deactivate"));

    let back = SandboxState::from_json(&state.to_json().unwrap()).unwrap();
    assert_eq!(back, state);
}

#[test]
fn the_rendered_state_quotes_every_word() {
    let mut state = SandboxState::default();
    state.exports.insert("MSG".to_owned(), "it's $(rm -rf ~)".to_owned());
    state.functions.insert("f".to_owned(), "echo 'hi'".to_owned());
    state.aliases.insert("g".to_owned(), "git".to_owned());
    state.unsets.insert("OLD".to_owned());
    state.removed_functions.insert("theme".to_owned());
    let text = state.render();
    assert!(text.contains("builtin typeset -gx -- MSG='it'\\''s $(rm -rf ~)'\n"), "{text}");
    assert!(text.contains("functions['f']='echo '\\''hi'\\'''\n"), "{text}");
    assert!(text.contains("aliases['g']='git'\n"), "{text}");
    assert!(text.contains("builtin unset -- OLD\n"), "{text}");
    assert!(
        text.contains("(( ${+functions['theme']} )) && builtin unfunction -- 'theme'\n"),
        "{text}"
    );
    assert_eq!(quote(""), "''");
}

use pretty_assertions::assert_eq;
use syntect::parsing::{ParseState, Scope, ScopeStack, ScopeStackOp};

use super::{Language, refine};
use crate::highlight::ASSETS;

/// The top scope of each token of `lines` in `label`, after [`refine`]: the token is
/// the text between two operations.
fn tops(label: &str, lines: &[&str]) -> Vec<Vec<(String, String)>> {
    let syntax = ASSETS.syntax_for_label(label).unwrap();
    let language = Language::of(syntax.scope).unwrap();
    let mut parse = ParseState::new(syntax);
    let mut stack = ScopeStack::new();
    let mut out = Vec::new();
    for (at, line) in lines.iter().enumerate() {
        let text = format!("{line}\n");
        let ops = parse.parse_line(&text, ASSETS.syntaxes()).unwrap();
        let mut plain = stack.clone();
        for (_, op) in &ops {
            plain.apply(op).unwrap();
        }
        let ops = refine(language, &stack, at == 0, &text, ops);
        let mut row = Vec::new();
        let mut from = 0;
        for (position, op) in &ops {
            if *position > from {
                let top =
                    stack.as_slice().last().map(|scope| scope.build_string()).unwrap_or_default();
                row.push((text[from..*position].to_owned(), top));
                from = *position;
            }
            stack.apply(op).unwrap();
        }
        assert_eq!(stack, plain, "the added scopes close in line {at}: {line}");
        out.push(row);
    }
    out
}

fn top_of(rows: &[Vec<(String, String)>], line: usize, token: &str) -> String {
    rows[line]
        .iter()
        .find(|(text, _)| text == token)
        .map(|(_, top)| top.clone())
        .unwrap_or_else(|| panic!("no token {token:?} in line {line}: {:?}", rows[line]))
}

#[test]
fn rust_gets_the_scopes_of_the_captures_of_the_editor() {
    let rows = tops(
        "rust",
        &[
            "pub fn finish(&mut self) -> usize {",
            "    let mut drained = 0;",
            "    self.pending.push(msg);",
            "    tracing::debug!(\"{}\", self.id, MAX_LEN);",
            "    let v = vec![1];",
            "    self.state = State::Done;",
            "    use std::fs;",
            "    let n = foo(y);",
            "}",
            "#[test]",
        ],
    );
    let cases = [
        (0, "self", "variable.language.efr"),
        (0, "usize", "support.type.primitive.efr"),
        (1, "drained", "variable.other.readwrite.efr"),
        (2, "pending", "variable.other.member.efr"),
        (2, "msg", "variable.other.readwrite.efr"),
        (3, "debug", "support.function.macro.efr"),
        (3, "!", "support.function.macro.efr"),
        (3, "id", "variable.other.member.efr"),
        (4, "vec!", "support.function.macro.efr"),
        (5, "Done", "support.type.efr"),
        (6, "fs", "entity.name.namespace.efr"),
        (7, "foo", "support.function.rust"),
        (7, "y", "variable.other.readwrite.efr"),
        (9, "#", "punctuation.section.efr"),
        (9, "test", "support.function.macro.efr"),
    ];
    for (line, token, scope) in cases {
        assert_eq!(top_of(&rows, line, token), scope, "{token} in line {line}");
    }
    // Strings, keywords, numbers and paths keep the grammar's scope.
    assert_eq!(top_of(&rows, 3, "tracing"), "meta.path.rust");
    assert_eq!(top_of(&rows, 1, "0"), "constant.numeric.integer.decimal.rust");
    assert_eq!(top_of(&rows, 1, "let"), "storage.type.rust");
}

#[test]
fn an_upper_case_name_of_typescript_is_a_constant() {
    let rows = tops("ts", &["const ID = 1;", "const m = ID;"]);
    assert_eq!(top_of(&rows, 0, "ID"), "constant.other.efr");
    assert_eq!(top_of(&rows, 1, "m"), "variable.other.constant.ts");
}

#[test]
fn shell_gets_numbers_test_operators_and_the_directive() {
    let rows = tops(
        "bash",
        &["#!/usr/bin/env bash", "sleep 2", "if [[ $? -ne 0 ]]; then", "  tail -n 20 log2", "fi"],
    );
    let shebang = ("#!/usr/bin/env bash".to_owned(), "keyword.other.directive.efr".to_owned());
    assert_eq!(rows[0], [shebang]);
    assert_eq!(top_of(&rows, 1, "2"), "constant.numeric.integer.efr");
    assert_eq!(top_of(&rows, 2, "-"), "keyword.operator.comparison.efr");
    assert_eq!(top_of(&rows, 2, "ne"), "keyword.operator.comparison.efr");
    assert_eq!(top_of(&rows, 2, "0"), "constant.numeric.integer.efr");
    assert_eq!(top_of(&rows, 3, "20"), "constant.numeric.integer.efr");
    // An option outside `[[ ]]` stays an option.
    assert_eq!(top_of(&rows, 3, "n"), "variable.parameter.option.shell");
}

#[test]
fn a_hash_line_after_the_first_stays_a_comment() {
    let rows = tops("bash", &["echo", "#!/bin/sh"]);
    assert!(
        rows[1].iter().all(|(_, top)| top.starts_with("comment") || top.starts_with("punctuation"))
    );
}

#[test]
fn a_mark_across_operations_of_the_grammar_is_split_at_each() {
    let push = |name: &str| ScopeStackOp::Push(Scope::new(name).unwrap());
    let added = Scope::new("added.efr").unwrap();
    let ops = vec![(0, push("a")), (2, ScopeStackOp::Pop(1)), (4, push("b"))];
    let merged = super::merge(ops, &[(0, 6, added)]);
    let want = vec![
        (0, push("a")),
        (0, ScopeStackOp::Push(added)),
        (2, ScopeStackOp::Pop(1)),
        (2, ScopeStackOp::Pop(1)),
        (2, ScopeStackOp::Push(added)),
        (4, ScopeStackOp::Pop(1)),
        (4, push("b")),
        (4, ScopeStackOp::Push(added)),
        (6, ScopeStackOp::Pop(1)),
    ];
    assert_eq!(format!("{merged:?}"), format!("{want:?}"));
}

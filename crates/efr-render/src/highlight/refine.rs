//! Scopes that the grammars of syntect do not give and the design's syntax table
//! needs (TM-Code). The table follows the tree-sitter captures of the editor, where
//! every identifier is a capture (`@variable`, `@variable.member`, `@type`,
//! `@constant`, `@function.macro`), and syntect's grammars leave plain variables,
//! fields and macro names of Rust without a scope, give the parameter `self` the
//! scope of each parameter, `usize` the scope of `let`, and so on.
//!
//! This module adds a scope on top of such a token, in the TextMate name of the
//! capture, so the code theme colours it by its own rule for that kind. It adds no
//! colour: a theme without a rule for the scope shows the token as before. The parse
//! state of the grammar does not see the scopes, so the next line parses as before.

use std::sync::OnceLock;

use syntect::parsing::{ClearAmount, Scope, ScopeStack, ScopeStackOp};

/// The languages that this module knows, by the scope of the grammar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Language {
    Rust,
    /// TypeScript and JavaScript, with and without JSX.
    Script,
    Shell,
    Markdown,
}

impl Language {
    /// The language of a grammar whose top scope is `scope` (`source.rust`).
    pub(crate) fn of(scope: Scope) -> Option<Language> {
        let s = scopes();
        if s.source_rust.is_prefix_of(scope) {
            Some(Language::Rust)
        } else if s.source_shell.is_prefix_of(scope) {
            Some(Language::Shell)
        } else if s.markdown.is_prefix_of(scope) {
            Some(Language::Markdown)
        } else if [s.source_ts, s.source_tsx, s.source_js, s.source_jsx]
            .iter()
            .any(|prefix| prefix.is_prefix_of(scope))
        {
            Some(Language::Script)
        } else {
            None
        }
    }
}

/// The scopes that the rules read and add, made once.
struct Scopes {
    source_rust: Scope,
    source_shell: Scope,
    source_ts: Scope,
    source_tsx: Scope,
    source_js: Scope,
    source_jsx: Scope,
    markdown: Scope,
    markup_list: Scope,
    list_mark: Scope,
    source: Scope,
    meta: Scope,
    string: Scope,
    comment: Scope,
    meta_path: Scope,
    meta_generic: Scope,
    meta_macro: Scope,
    variable_parameter: Scope,
    variable_other: Scope,
    storage_type: Scope,
    arguments: Scope,
    option: Scope,
    double_brace_begin: Scope,
    double_brace_end: Scope,
    support_macro: Scope,
    variable_annotation: Scope,
    annotation_mark: Scope,
    impl_name: Scope,
    module_name: Scope,
    // The scopes that the rules add.
    variable: Scope,
    member: Scope,
    function_call: Scope,
    macro_name: Scope,
    module: Scope,
    bracket: Scope,
    separator: Scope,
    type_name: Scope,
    constant: Scope,
    builtin: Scope,
    primitive: Scope,
    number: Scope,
    test_operator: Scope,
    directive: Scope,
}

fn scopes() -> &'static Scopes {
    static SCOPES: OnceLock<Scopes> = OnceLock::new();
    SCOPES.get_or_init(|| {
        let s = |name: &str| Scope::new(name).unwrap_or_default();
        Scopes {
            source_rust: s("source.rust"),
            source_shell: s("source.shell"),
            source_ts: s("source.ts"),
            source_tsx: s("source.tsx"),
            source_js: s("source.js"),
            source_jsx: s("source.jsx"),
            markdown: s("text.html.markdown"),
            markup_list: s("markup.list"),
            list_mark: s("punctuation.definition.list_item"),
            source: s("source"),
            meta: s("meta"),
            string: s("string"),
            comment: s("comment"),
            meta_path: s("meta.path"),
            meta_generic: s("meta.generic"),
            meta_macro: s("meta.macro"),
            variable_parameter: s("variable.parameter"),
            variable_other: s("variable.other"),
            storage_type: s("storage.type"),
            arguments: s("meta.function-call.arguments"),
            option: s("variable.parameter.option"),
            double_brace_begin: s("support.function.double-brace.begin"),
            double_brace_end: s("support.function.double-brace.end"),
            support_macro: s("support.macro"),
            variable_annotation: s("variable.annotation"),
            annotation_mark: s("punctuation.definition.annotation"),
            impl_name: s("entity.name.impl"),
            module_name: s("entity.name.module"),
            variable: s("variable.other.readwrite.efr"),
            member: s("variable.other.member.efr"),
            function_call: s("variable.function.efr"),
            macro_name: s("support.function.macro.efr"),
            module: s("entity.name.namespace.efr"),
            bracket: s("punctuation.section.efr"),
            separator: s("punctuation.separator.efr"),
            type_name: s("support.type.efr"),
            constant: s("constant.other.efr"),
            builtin: s("variable.language.efr"),
            primitive: s("support.type.primitive.efr"),
            number: s("constant.numeric.integer.efr"),
            test_operator: s("keyword.operator.comparison.efr"),
            directive: s("keyword.other.directive.efr"),
        }
    })
}

/// The primitive types of Rust, which the grammar scopes as `storage.type` (the
/// scope of `let`).
const RUST_PRIMITIVES: &[&str] = &[
    "bool", "char", "str", "u8", "u16", "u32", "u64", "u128", "usize", "i8", "i16", "i32", "i64",
    "i128", "isize", "f32", "f64",
];

/// A run of the line `[start, end)` under one scope stack: the text between two
/// operations of the grammar.
struct Segment<'a> {
    start: usize,
    end: usize,
    stack: &'a [Scope],
}

/// The operations of one line of `language` with the added scopes. `stack` is the
/// scope stack before the line, `first` says that the line is the first of the block,
/// `text` is the line with its newline. The added scopes always close in the line.
pub(crate) fn refine(
    language: Language,
    stack: &ScopeStack,
    first: bool,
    text: &str,
    ops: Vec<(usize, ScopeStackOp)>,
) -> Vec<(usize, ScopeStackOp)> {
    let content = text.trim_end_matches(['\n', '\r']).len();
    if language == Language::Shell && first && text.starts_with("#!") {
        return shebang(stack, text, &ops, content);
    }
    if language == Language::Markdown {
        return markdown(stack, ops);
    }
    let mut marks: Vec<(usize, usize, Scope)> = Vec::new();
    let mut stack = stack.clone();
    let mut at = 0;
    let mut index = 0;
    let mut in_test = false;
    while at < content {
        // Apply the operations at `at`, then the segment runs to the next position.
        while index < ops.len() && ops[index].0 <= at {
            if stack.apply(&ops[index].1).is_err() {
                return ops;
            }
            index += 1;
        }
        let end = ops.get(index).map_or(content, |(position, _)| (*position).min(content));
        let segment = Segment { start: at, end, stack: stack.as_slice() };
        match language {
            Language::Rust => rust(text, &segment, &mut marks),
            Language::Script => script(text, &segment, &mut marks),
            Language::Shell => shell(text, &segment, &mut in_test, &mut marks),
            Language::Markdown => {}
        }
        at = end;
    }
    if marks.is_empty() {
        return ops;
    }
    merge(ops, &marks)
}

/// The words of a segment: each run of `[A-Za-z0-9_]` that starts with no such
/// character before it.
fn words<'a>(text: &'a str, segment: &Segment<'_>) -> impl Iterator<Item = (usize, &'a str)> {
    let bytes = text.as_bytes();
    let (start, end) = (segment.start, segment.end);
    let mut at = start;
    std::iter::from_fn(move || {
        while at < end {
            let word_char = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
            if !word_char(bytes[at]) || (at > 0 && word_char(bytes[at - 1])) {
                at += 1;
                continue;
            }
            let from = at;
            while at < bytes.len() && word_char(bytes[at]) {
                at += 1;
            }
            // A word that runs past the segment belongs to the grammar's next token.
            if at > end {
                return None;
            }
            return text.get(from..at).map(|word| (from, word));
        }
        None
    })
}

fn top(stack: &[Scope]) -> Option<Scope> {
    stack.last().copied()
}

fn has(stack: &[Scope], prefix: Scope) -> bool {
    stack.iter().any(|scope| prefix.is_prefix_of(*scope))
}

/// No scope of its own: the top of the stack is the grammar's root or a `meta` scope.
fn bare(stack: &[Scope]) -> bool {
    let s = scopes();
    top(stack).is_none_or(|scope| s.source.is_prefix_of(scope) || s.meta.is_prefix_of(scope))
}

/// A `use` line of Rust: its names are modules and items (`use app::{Store, open};`).
fn is_use(text: &str) -> bool {
    let text = text.trim_start();
    let text = text.strip_prefix("pub").map_or(text, |rest| {
        let rest = rest.trim_start();
        rest.strip_prefix('(').and_then(|rest| rest.split_once(')')).map_or(rest, |(_, r)| r)
    });
    text.trim_start().starts_with("use ")
}

fn is_constant_name(word: &str) -> bool {
    let word = word.trim_start_matches('_');
    word.starts_with(|c: char| c.is_ascii_uppercase())
        && word.len() > 1
        && word.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

/// Rust (the captures of `highlights.scm` of tree-sitter-rust): a bare identifier is
/// a variable, a member after `.`, a macro before `!(`, a function before `(`, a type
/// when it starts upper case, a constant when all of it is, a module after `::`, after
/// `mod` and in a `use` line;
/// `self` is a builtin, a primitive type is a type, a macro that the grammar knows
/// and the name of an attribute are macros, the name of an `impl` is a type, a
/// bracket, comma or semicolon that the grammar left bare is punctuation.
fn rust(text: &str, segment: &Segment<'_>, marks: &mut Vec<(usize, usize, Scope)>) {
    let s = scopes();
    let stack = segment.stack;
    if has(stack, s.string) || has(stack, s.comment) {
        return;
    }
    let bytes = text.as_bytes();
    let top = top(stack);
    let whole = |scope: Scope| (segment.start, segment.end, scope);
    match top {
        // `println!`: the grammar knows the name, and the theme has no rule for it.
        Some(scope) if s.support_macro.is_prefix_of(scope) => {
            return marks.push(whole(s.macro_name));
        }
        // `#[test]`: the name of an attribute is a macro, its `#` a bracket.
        Some(scope) if s.variable_annotation.is_prefix_of(scope) => {
            return marks.push(whole(s.macro_name));
        }
        Some(scope) if s.annotation_mark.is_prefix_of(scope) => {
            return marks.push(whole(s.bracket));
        }
        // `impl Store`, `mod json`: the grammar names them, the theme has no rule.
        Some(scope) if s.impl_name.is_prefix_of(scope) => {
            return marks.push(whole(s.type_name));
        }
        Some(scope) if s.module_name.is_prefix_of(scope) => {
            return marks.push(whole(s.module));
        }
        _ => {}
    }
    if bare(stack) && !has(stack, s.meta_path) {
        // A bracket that the grammar did not match (a hunk that starts inside a
        // block), a comma or a semicolon with no scope.
        for (at, byte) in text.bytes().enumerate().take(segment.end).skip(segment.start) {
            let scope = match byte {
                b'{' | b'}' | b'(' | b')' | b'[' | b']' => s.bracket,
                b',' | b';' => s.separator,
                _ => continue,
            };
            marks.push((at, at + 1, scope));
        }
    }
    for (from, word) in words(text, segment) {
        let to = from + word.len();
        if word.starts_with(|c: char| c.is_ascii_digit()) {
            continue;
        }
        if top.is_some_and(|scope| s.variable_parameter.is_prefix_of(scope)) {
            if word == "self" {
                marks.push((from, to, s.builtin));
            }
            continue;
        }
        if top.is_some_and(|scope| s.storage_type.is_prefix_of(scope)) {
            if RUST_PRIMITIVES.contains(&word) {
                marks.push((from, to, s.primitive));
            }
            continue;
        }
        if !bare(stack)
            || [s.meta_path, s.meta_generic, s.meta_macro].iter().any(|prefix| has(stack, *prefix))
        {
            continue;
        }
        let after = bytes.get(to).copied();
        let member = from > 0 && bytes[from - 1] == b'.' && (from < 2 || bytes[from - 2] != b'.');
        let path = from >= 2 && &bytes[from - 2..from] == b"::";
        let scope = if after == Some(b'!') && matches!(bytes.get(to + 1), Some(b'(' | b'[' | b'{'))
        {
            marks.push((from, to + 1, s.macro_name));
            continue;
        } else if member {
            s.member
        } else if is_constant_name(word) {
            s.constant
        } else if word.starts_with(|c: char| c.is_ascii_uppercase()) {
            s.type_name
        } else if after == Some(b'(') {
            s.function_call
        } else if path || is_use(text) || text[..from].trim_end().ends_with("mod") {
            s.module
        } else {
            s.variable
        };
        marks.push((from, to, scope));
    }
}

/// TypeScript and JavaScript: a name that is all upper case is a constant (the
/// grammar gives `const ID` the scope of `const m`).
fn script(text: &str, segment: &Segment<'_>, marks: &mut Vec<(usize, usize, Scope)>) {
    let s = scopes();
    if !top(segment.stack).is_some_and(|scope| s.variable_other.is_prefix_of(scope)) {
        return;
    }
    for (from, word) in words(text, segment) {
        if is_constant_name(word) {
            marks.push((from, from + word.len(), s.constant));
        }
    }
}

/// Shell: a number as a word of a command is a number, and an option inside `[[ ]]`
/// is a test operator (`-ne`), as tree-sitter-bash captures them.
fn shell(
    text: &str,
    segment: &Segment<'_>,
    in_test: &mut bool,
    marks: &mut Vec<(usize, usize, Scope)>,
) {
    let s = scopes();
    let stack = segment.stack;
    let Some(top) = top(stack) else { return };
    if s.double_brace_begin.is_prefix_of(top) {
        *in_test = true;
        return;
    }
    if s.double_brace_end.is_prefix_of(top) {
        *in_test = false;
        return;
    }
    if *in_test && has(stack, s.option) {
        marks.push((segment.start, segment.end, s.test_operator));
        return;
    }
    if !s.arguments.is_prefix_of(top) {
        return;
    }
    let bytes = text.as_bytes();
    let space = |at: Option<&u8>| at.is_none_or(|b| b.is_ascii_whitespace() || *b == b';');
    for (from, word) in words(text, segment) {
        let to = from + word.len();
        let whole = (from == 0 || space(bytes.get(from - 1))) && space(bytes.get(to));
        if whole && word.bytes().all(|b| b.is_ascii_digit()) {
            marks.push((from, to, s.number));
        }
    }
}

/// The first line `#!…` of a shell block: a directive, not a comment (tree-sitter-bash
/// `@keyword.directive`). The comment scopes would keep the comment's italic under an
/// added scope, so the line gets the stack that the grammar leaves after it and the
/// directive on top, and no comment scope.
fn shebang(
    before: &ScopeStack,
    text: &str,
    ops: &[(usize, ScopeStackOp)],
    content: usize,
) -> Vec<(usize, ScopeStackOp)> {
    let mut after = before.clone();
    for (_, op) in ops {
        if after.apply(op).is_err() {
            return ops.to_vec();
        }
    }
    let (before, after) = (before.as_slice(), after.as_slice());
    let common = before.iter().zip(after).take_while(|(a, b)| a == b).count();
    let mut out = Vec::new();
    if before.len() > common {
        out.push((0, ScopeStackOp::Pop(before.len() - common)));
    }
    out.extend(after[common..].iter().map(|scope| (0, ScopeStackOp::Push(*scope))));
    out.push((0, ScopeStackOp::Push(scopes().directive)));
    out.push((content.min(text.len()), ScopeStackOp::Pop(1)));
    out
}

/// Markdown: a list item has no colour (TM-Diffs 1, the rows of `CHANGELOG.md`). The
/// grammar scopes the whole item as `markup.list`, where the editor's capture is the
/// marker alone, and the board draws neither in a colour. The line gets the
/// operations of the grammar without the scopes of a list: a pop or a clear counts
/// only the scopes that stay. `stack` is the grammar's stack before the line.
fn markdown(stack: &ScopeStack, ops: Vec<(usize, ScopeStackOp)>) -> Vec<(usize, ScopeStackOp)> {
    let s = scopes();
    let list = |scope: Scope| s.markup_list.is_prefix_of(scope) || s.list_mark.is_prefix_of(scope);
    if !ops.iter().any(|(_, op)| matches!(op, ScopeStackOp::Push(scope) if list(*scope)))
        && !stack.as_slice().iter().any(|scope| list(*scope))
    {
        return ops;
    }
    let mut grammar = stack.clone();
    let mut out = Vec::with_capacity(ops.len());
    for (position, op) in ops {
        let kept = |n: usize, grammar: &ScopeStack| {
            let scopes = grammar.as_slice();
            scopes[scopes.len().saturating_sub(n)..].iter().filter(|scope| !list(**scope)).count()
        };
        let filtered = match &op {
            ScopeStackOp::Push(scope) => (!list(*scope)).then_some(op.clone()),
            ScopeStackOp::Pop(n) => {
                let k = kept(*n, &grammar);
                (k > 0).then_some(ScopeStackOp::Pop(k))
            }
            ScopeStackOp::Clear(ClearAmount::TopN(n)) => {
                let k = kept(*n, &grammar);
                (k > 0).then_some(ScopeStackOp::Clear(ClearAmount::TopN(k)))
            }
            ScopeStackOp::Clear(ClearAmount::All) | ScopeStackOp::Restore | ScopeStackOp::Noop => {
                Some(op.clone())
            }
        };
        if grammar.apply(&op).is_err() {
            return out;
        }
        out.extend(filtered.map(|op| (position, op)));
    }
    out
}

/// The operations `ops` with each mark as a push at its start and a pop at its end,
/// split at every operation of the grammar inside it, so that each added scope is the
/// top of the stack while it is open: a pop before the grammar's operations at a
/// position, a push after them.
fn merge(
    ops: Vec<(usize, ScopeStackOp)>,
    marks: &[(usize, usize, Scope)],
) -> Vec<(usize, ScopeStackOp)> {
    // (position, order, sequence, op): order 0 is an added pop, 1 the grammar's, 2
    // an added push.
    let mut events: Vec<(usize, u8, usize, ScopeStackOp)> =
        Vec::with_capacity(ops.len() + marks.len() * 2);
    let positions: Vec<usize> = ops.iter().map(|(position, _)| *position).collect();
    for (sequence, (position, op)) in ops.into_iter().enumerate() {
        events.push((position, 1, sequence, op));
    }
    for &(start, end, scope) in marks {
        let mut from = start;
        for &cut in positions.iter().filter(|&&p| p > start && p < end) {
            if cut > from {
                events.push((from, 2, 0, ScopeStackOp::Push(scope)));
                events.push((cut, 0, 0, ScopeStackOp::Pop(1)));
            }
            from = cut;
        }
        if end > from {
            events.push((from, 2, 0, ScopeStackOp::Push(scope)));
            events.push((end, 0, 0, ScopeStackOp::Pop(1)));
        }
    }
    events.sort_by_key(|(position, order, sequence, _)| (*position, *order, *sequence));
    events.into_iter().map(|(position, _, _, op)| (position, op)).collect()
}

#[cfg(test)]
mod tests;

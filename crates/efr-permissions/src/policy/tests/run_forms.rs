//! The forms that a rule which asks or denies reads behind a wrapper (OQL3): the
//! borrowed forms decide as the copied forms of v0.0.4 did, on random lines with
//! wrapper chains, program paths and git options.

use std::path::{Path, PathBuf};

use proptest::prelude::*;

use super::cx;
use crate::Effect;
use crate::command::{self, git};
use crate::policy::{Action, CommandPattern, Fit, MAX_FORMS, Part, Place, Resource, Rule, Under};

/// The forms as v0.0.4 made them: each one a copy of the rest of the line, each once.
fn copied_run_forms(words: &[String]) -> Vec<Vec<String>> {
    let base_name = |word: &str| word.rsplit('/').next().unwrap_or(word).to_owned();
    let mut forms = vec![words.to_vec()];
    let Some(program) = words.first() else { return forms };
    let named = |rest: &[String]| -> Vec<String> {
        let mut form = rest.to_vec();
        if let Some(first) = form.first_mut() {
            *first = base_name(first);
        }
        form
    };
    let mut add = |form: Vec<String>| {
        if !forms.contains(&form) {
            forms.push(form);
        }
    };
    add(named(words));
    if command::WRAPPERS.contains(&base_name(program).as_str()) {
        for at in (1..words.len()).filter(|at| !words[*at].starts_with('-')) {
            add(named(&words[at..]));
        }
    }
    forms
}

/// How v0.0.4 matched a rule that asks or denies against the command `part`, past
/// the match of its words as they are. v0.0.4 knew the dir only before any change of
/// directory, and a pattern with `under` matched nothing without it.
pub(super) fn copied_fit(rule: &Rule, pattern: &CommandPattern, part: &Part<'_>) -> Fit {
    let cx = cx();
    let dir = part.place.known();
    if part.privileged {
        return Fit::No;
    }
    if pattern.matches_command(part.words, part.pattern) && rule.placed(pattern, dir, &[], &cx) {
        return Fit::Raw;
    }
    let is_git = |words: &[String]| {
        pattern.program == "git" && words.first().is_some_and(|word| word == "git")
    };
    let mut matched = false;
    let mut dirs: Vec<PathBuf> = Vec::new();
    for words in copied_run_forms(part.words) {
        matched |= pattern.matches_command(&words, part.pattern);
        if is_git(&words) {
            let views = git::views(&words);
            matched |= views.unreadable
                || views.words.iter().any(|view| pattern.matches_command(view, part.pattern));
            dirs.extend(views.dirs.iter().flat_map(|dirs| dirs.resolve(dir, cx.home)));
        }
    }
    if matched && rule.placed(pattern, dir, &dirs, &cx) { Fit::Read } else { Fit::No }
}

/// The words of a random line: wrappers with their values, program paths, git's own
/// options and subcommands, and operands.
fn line() -> impl Strategy<Value = Vec<String>> {
    let word = prop::sample::select(vec![
        "env",
        "/usr/bin/env",
        "X=1",
        "nice",
        "-n",
        "5",
        "time",
        "timeout",
        "60",
        "xargs",
        "-0",
        "git",
        "/usr/bin/git",
        "-C",
        "/home/u/p/app",
        "/tmp",
        "--work-tree=/tmp/w",
        "-c",
        "alias.p=push",
        "p",
        "push",
        "pull",
        "status",
        "--force",
        "-f",
        "origin",
        "rg",
        "rm",
        "-rf",
        "--",
        "f1",
        "*.rs",
    ]);
    prop::collection::vec(word.prop_map(str::to_owned), 1..12)
}

/// The words of a random git line: a chain of wrappers with their values, then git by
/// its name or its path with some of its own options, a subcommand and operands, at
/// times twice, as `xargs` or a value of a wrapper may give it.
fn git_line() -> impl Strategy<Value = Vec<String>> {
    let wrapper = prop::sample::select(vec![
        &["env"][..],
        &["env", "X=1"],
        &["/usr/bin/env"],
        &["nice", "-n", "5"],
        &["time"],
        &["timeout", "60"],
        &["xargs", "-0"],
    ]);
    let program = prop::sample::select(vec!["git", "/usr/bin/git"]);
    let unit = prop::sample::select(vec![
        &["-C", "/tmp"][..],
        &["-C", "/home/u/p/app"],
        &["-C", "sub"],
        &["--work-tree=/tmp/w"],
        &["-c", "alias.p=push"],
        &["p"],
        &["push"],
        &["pull"],
        &["status"],
        &["--force"],
        &["origin"],
    ]);
    let git = (program, prop::collection::vec(unit, 0..4));
    (prop::collection::vec(wrapper, 0..3), git.clone(), prop::option::weighted(0.8, git)).prop_map(
        |(wrappers, first, second)| {
            let mut words: Vec<&str> = wrappers.concat();
            for (program, units) in std::iter::once(first).chain(second) {
                words.push(program);
                words.extend(units.concat());
            }
            words.into_iter().map(str::to_owned).collect()
        },
    )
}

/// A random command pattern of a program that the lines name.
fn pattern() -> impl Strategy<Value = CommandPattern> {
    let program =
        prop::sample::select(vec!["git", "git", "git", "rg", "rm", "push", "env", "nice", "p"]);
    let args = prop::sample::select(vec![&[][..], &["push"], &["push|pull"], &["p*"], &["-C"]]);
    let forbid = prop::sample::select(vec![&[][..], &["--force"], &["-f"]]);
    let max_operands = prop::option::of(0_usize..3);
    let under = prop::sample::select(vec![
        None,
        Some(Under::Project),
        Some(Under::Path(PathBuf::from("~/p"))),
        Some(Under::Path(PathBuf::from("/tmp"))),
    ]);
    (program, args, forbid, max_operands, under).prop_map(
        |(program, args, forbid, max_operands, under)| {
            let mut pattern = CommandPattern::new(program)
                .with_args(args.iter().copied())
                .with_forbid(forbid.iter().copied());
            pattern.max_operands = max_operands;
            pattern.under = under;
            pattern
        },
    )
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(16_384))]

    /// A rule that asks or denies fits each command as it did with the copied forms:
    /// the same [`Fit`], so the same effect, the same cause and the same placing.
    #[test]
    fn borrowed_forms_fit_as_copied_forms(
        words in prop_oneof![line(), git_line()],
        pattern in pattern(),
        deny in any::<bool>(),
        globbed in any::<bool>(),
        known_dir in any::<bool>(),
    ) {
        let effect = if deny { Effect::Deny } else { Effect::Ask };
        let rule = Rule::new(Action::Execute, Resource::Command(pattern.clone()), effect);
        let place =
            if known_dir { Place::Start(Path::new("/home/u/p/app")) } else { Place::Unknown };
        let part = Part { words: &words, pattern: globbed, privileged: false, place };
        let new = rule.runs(&pattern, &part, &cx());
        let old = copied_fit(&rule, &pattern, &part);
        // NOTE: where the dir is not known, a pattern with `under` now matches what its
        // words match (OQL9), and only as a read form.
        let unplaced = !known_dir && pattern.under.is_some() && old == Fit::No;
        prop_assert!(
            new == old || (unplaced && new == Fit::Read),
            "{:?} against {:?}: {:?}, was {:?}", pattern, words, new, old
        );
    }
}

/// A rule with `under` reads the dirs of every git form, also after a form that
/// matched in another place: here only the second `git` works in `/tmp`.
#[test]
fn a_later_git_form_places_a_rule_with_under() {
    let pattern = CommandPattern::new("git").with_args(["push"]).with_under("/tmp");
    let rule = Rule::new(Action::Execute, Resource::Command(pattern.clone()), Effect::Ask);
    let words: Vec<String> =
        "env git push git -C /tmp push".split(' ').map(str::to_owned).collect();
    let place = Place::Start(Path::new("/home/u"));
    let part = Part { words: &words, pattern: false, privileged: false, place };
    assert_eq!(copied_fit(&rule, &pattern, &part), Fit::Read);
    assert_eq!(rule.runs(&pattern, &part, &cx()), Fit::Read);
}

/// `env` and then `git` `n` times.
fn gits(n: usize) -> Vec<String> {
    std::iter::once("env").chain(std::iter::repeat_n("git", n)).map(str::to_owned).collect()
}

/// Past [`MAX_FORMS`] forms of its program, a rule that asks or denies matches in any
/// place: `env git git git ...` read the views of each form over the rest of the line,
/// the square of its words. Up to the cap it fits as the copied forms did.
#[test]
fn a_rule_matches_a_command_with_more_forms_of_its_program_than_it_reads() {
    let pattern = CommandPattern::new("git").with_args(["push"]).with_under("/tmp");
    let rule = Rule::new(Action::Execute, Resource::Command(pattern.clone()), Effect::Ask);
    let place = Place::Start(Path::new("/home/u"));
    for n in [1, 2, MAX_FORMS] {
        let words = gits(n);
        let part = Part { words: &words, pattern: false, privileged: false, place };
        assert_eq!(copied_fit(&rule, &pattern, &part), Fit::No, "{n} gits");
        assert_eq!(rule.runs(&pattern, &part, &cx()), Fit::No, "{n} gits");
    }
    for n in [MAX_FORMS + 1, 10_000] {
        let words = gits(n);
        let part = Part { words: &words, pattern: false, privileged: false, place };
        assert_eq!(rule.runs(&pattern, &part, &cx()), Fit::Read, "{n} gits");
    }
    // NOTE: a rule with no `under` still stops at the first form that matches.
    let pattern = CommandPattern::new("git");
    let rule = Rule::new(Action::Execute, Resource::Command(pattern.clone()), Effect::Deny);
    let words = gits(10_000);
    let part = Part { words: &words, pattern: false, privileged: false, place };
    assert_eq!(rule.runs(&pattern, &part, &cx()), Fit::Read);
}

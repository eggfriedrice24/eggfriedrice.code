//! Where the parts of a line run for `under` (OQL9), and the readings past the global
//! options of `systemctl`, `cargo`, `docker` and `kubectl` (OQL4): on random lines and
//! policies, the effect of the rules is never looser than in v0.0.4.

use std::path::Path;

use proptest::prelude::*;
use rstest::rstest;

use super::cx;
use super::run_forms::copied_fit;
use crate::Effect;
use crate::command::{self, SimpleCommand, git};
use crate::policy::place::MAX_DIRS;
use crate::policy::{
    Action, CommandPattern, Fit, Part, Place, Places, Policy, Resource, Rule, Target, Under,
};

const HOME: &str = "/home/u";

/// The dir where each command of `commands` ran for v0.0.4: `start` until a `cd`,
/// `pushd`, `popd` or `chdir` by its first word, then unknown.
fn copied_dirs<'a>(commands: &[SimpleCommand], start: Option<&'a Path>) -> Vec<Option<&'a Path>> {
    let mut dir = start;
    commands
        .iter()
        .map(|command| {
            let now = dir;
            let program = command.words[0].rsplit('/').next().unwrap_or(&command.words[0]);
            if matches!(program, "cd" | "pushd" | "popd" | "chdir") {
                dir = None;
            }
            now
        })
        .collect()
}

/// How v0.0.4 fit `rule` to the command `part`, whose dir is `part.place.known()`.
fn copied_rule_fit(rule: &Rule, part: &Part<'_>) -> Fit {
    let Resource::Command(pattern) = &rule.resource else {
        return Fit::from(rule.resource == Resource::Any);
    };
    if rule.effect >= Effect::Ask {
        return copied_fit(rule, pattern, part);
    }
    if part.privileged {
        return Fit::No;
    }
    let raw = pattern.matches_command(part.words, part.pattern);
    let plain = pattern.program == "git"
        && part.words[0] == "git"
        && git::plain_view(part.words).is_some_and(|view| pattern.matches_command(&view, false));
    ((raw || plain) && rule.placed(pattern, part.place.known(), &[], &cx())).into()
}

/// The effect of v0.0.4's last match of `policy` for the command `part`.
fn copied_last_match(policy: &Policy, part: &Part<'_>) -> Option<Effect> {
    let mut decided: Option<Effect> = None;
    for rule in policy.rules() {
        let counts = match copied_rule_fit(rule, part) {
            Fit::No => false,
            Fit::Raw => true,
            Fit::Read => decided.is_none_or(|effect| rule.effect >= effect),
        };
        if counts {
            decided = Some(rule.effect);
        }
    }
    decided
}

/// A random line: simple commands that change the dir in every way and commands with
/// global options, joined by every operator.
fn line() -> impl Strategy<Value = String> {
    let command = prop::sample::select(vec![
        "git push",
        "git -C /home/u/work push",
        "git -C work push",
        "env git push",
        "cargo publish",
        "cargo +nightly publish",
        "cargo -C /home/u/work publish",
        "cargo --zz publish",
        "systemctl restart x",
        "systemctl --user restart x",
        "docker -H h push i",
        "kubectl -n x delete pod p",
        "ls",
        "cd /tmp",
        "cd /home/u/work",
        "cd ~/work/app",
        "cd",
        "cd ~",
        "cd -",
        "cd sub",
        "cd ..",
        "cd -P /srv",
        "cd a b",
        "pushd /srv",
        "pushd",
        "popd",
        "time cd /tmp",
        "chdir /home/u/work",
    ]);
    let joint = prop::sample::select(vec!["; ", " && ", " || ", " | "]);
    (command.clone(), prop::collection::vec((joint, command), 0..5)).prop_map(|(first, rest)| {
        let mut line = first.to_owned();
        for (joint, command) in rest {
            line.push_str(joint);
            line.push_str(command);
        }
        line
    })
}

fn effect() -> impl Strategy<Value = Effect> {
    prop::sample::select(vec![Effect::Allow, Effect::Ask, Effect::Deny])
}

/// A random policy: a rule for every command line, then rules for the programs of the
/// lines, with and without `under`.
fn policy() -> impl Strategy<Value = Policy> {
    let program =
        prop::sample::select(vec!["git", "cargo", "systemctl", "docker", "kubectl", "env", "cd"]);
    let args = prop::sample::select(vec![
        &[][..],
        &["push"],
        &["publish"],
        &["restart"],
        &["delete"],
        &["-C"],
        &["--user"],
    ]);
    let under = prop::sample::select(vec![
        None,
        Some(Under::Path("~/work".into())),
        Some(Under::Path("/srv".into())),
        Some(Under::Path("/tmp".into())),
        Some(Under::Project),
    ]);
    let rule = (program, args, under, effect()).prop_map(|(program, args, under, effect)| {
        let mut pattern = CommandPattern::new(program).with_args(args.iter().copied());
        pattern.under = under;
        Rule::new(Action::Execute, Resource::Command(pattern), effect)
    });
    (effect(), prop::collection::vec(rule, 1..5)).prop_map(|(base, rules)| {
        let mut all = vec![Rule::new(Action::Execute, Resource::Any, base)];
        all.extend(rules);
        Policy::new(all).unwrap()
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(16_384))]

    /// Each part of a random line gets an effect at least as strict as v0.0.4 gave it.
    #[test]
    fn the_rules_are_never_looser_than_in_v0_0_4(
        line in line(),
        policy in policy(),
        start in prop::sample::select(vec![
            None,
            Some("/home/u/work/app"),
            Some("/srv"),
            Some("/home/u"),
            Some("/tmp"),
        ]),
    ) {
        let commands = command::analyze(&line).unwrap();
        let start = start.map(Path::new);
        let places = Places::walk(&commands, start, Path::new(HOME));
        for (index, (command, dir)) in commands.iter().zip(copied_dirs(&commands, start)).enumerate() {
            let privileged = command::privileged(command).is_some();
            let part = |place| Part { words: &command.words, pattern: command.pattern, privileged, place };
            let new = policy
                .last_match(&Target::Command(part(places.place(index))), &cx())
                .map(|(_, effect)| effect);
            let old = copied_last_match(&policy, &part(dir.map_or(Place::Unknown, Place::Start)));
            prop_assert!(new >= old, "{:?} part {}: {:?}, was {:?}", line, index, new, old);
        }
    }
}

/// The places of the commands of `line` from `/srv`.
fn places_of(line: &str) -> Vec<String> {
    let commands = command::analyze(line).unwrap();
    let places = Places::walk(&commands, Some(Path::new("/srv")), Path::new(HOME));
    (0..commands.len())
        .map(|index| match places.place(index) {
            Place::Start(dir) => format!("start {}", dir.display()),
            Place::Among(dirs) => {
                let dirs: Vec<String> = dirs.iter().map(|dir| dir.display().to_string()).collect();
                format!("among {}", dirs.join(" "))
            }
            Place::Unknown => "unknown".to_owned(),
        })
        .collect()
}

#[rstest]
#[case::no_cd("ls; ls", &["start /srv", "start /srv"])]
#[case::absolute("cd /tmp && ls", &["start /srv", "among /srv /tmp"])]
#[case::home("cd && ls", &["start /srv", "among /srv /home/u"])]
#[case::tilde("cd ~/work/../w && ls", &["start /srv", "among /srv /home/u/w"])]
#[case::options("cd -qP -- /tmp && ls", &["start /srv", "among /srv /tmp"])]
#[case::pushd("pushd /tmp && ls", &["start /srv", "among /srv /tmp"])]
#[case::two("cd /a; cd /b; ls", &["start /srv", "among /srv /a", "among /srv /a /b"])]
#[case::again("cd /a; cd /srv; ls", &["start /srv", "among /srv /a", "among /srv /a"])]
#[case::relative("cd sub && ls", &["start /srv", "unknown"])]
#[case::up("cd .. && ls", &["start /srv", "unknown"])]
#[case::back("cd - && ls", &["start /srv", "unknown"])]
#[case::stack("cd +1 && ls", &["start /srv", "unknown"])]
#[case::stack_minus("cd -1 && ls", &["start /srv", "unknown"])]
#[case::replace("cd a b && ls", &["start /srv", "unknown"])]
#[case::popd("popd && ls", &["start /srv", "unknown"])]
#[case::bare_pushd("pushd && ls", &["start /srv", "unknown"])]
#[case::pattern("cd /tmp/x* && ls", &["start /srv", "unknown"])]
#[case::behind_time("time cd /tmp && ls", &["start /srv", "unknown"])]
#[case::stays_unknown("cd - && cd /tmp && ls", &["start /srv", "unknown", "unknown"])]
#[case::by_path("/usr/bin/cd /tmp && ls", &["start /srv", "among /srv /tmp"])]
fn a_line_moves_where_its_cds_go(#[case] line: &str, #[case] expected: &[&str]) {
    assert_eq!(places_of(line), expected, "{line:?}");
}

#[test]
fn past_the_most_dirs_the_place_is_unknown() {
    let line: Vec<String> = (0..40).map(|at| format!("cd /d{at}")).collect();
    let places = places_of(&line.join("; "));
    assert_eq!(places[MAX_DIRS - 1].split(' ').count(), 33);
    assert_eq!(places[MAX_DIRS], "unknown");
}

#[test]
fn a_line_with_no_known_start_runs_where_efr_cannot_tell() {
    let commands = command::analyze("ls; cd /tmp; ls").unwrap();
    let places = Places::walk(&commands, None, Path::new(HOME));
    assert!((0..3).all(|index| places.place(index) == Place::Unknown));
}

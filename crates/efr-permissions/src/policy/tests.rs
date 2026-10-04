use std::path::{Path, PathBuf};

use pretty_assertions::assert_eq;
use rstest::rstest;
use serde::{Deserialize, Serialize};

use super::{Action, CommandPattern, MatchContext, Policy, Resource, Rule, Target, defaults};
use crate::{Access, Effect, PathClass, PermissionsError};

const HOME: &str = "/home/u";

fn cx() -> MatchContext<'static> {
    MatchContext {
        home: Path::new(HOME),
        home_aliases: &[],
        project_root: Some(Path::new("/home/u/p/app")),
    }
}

fn write(path: &'static str, class: PathClass) -> Target<'static> {
    Target::Path { path: Path::new(path), access: Access::Write, class }
}

fn read(path: &'static str, class: PathClass) -> Target<'static> {
    Target::Path { path: Path::new(path), access: Access::Read, class }
}

fn words(line: &str) -> Vec<String> {
    line.split(' ').map(str::to_owned).collect()
}

#[test]
fn the_defaults_pass_their_own_checks() {
    let defaults = Policy::defaults();
    assert_eq!(Policy::new(defaults.rules().to_vec()), Ok(defaults));
}

#[test]
fn the_defaults_keep_the_path_table_first_and_add_only_command_allows() {
    let defaults = Policy::defaults();
    let rules = defaults.rules();
    assert_eq!(rules[7], Rule::new(Action::Any, Resource::Class(PathClass::Secrets), Effect::Deny));
    for rule in &rules[8..] {
        assert_eq!(rule.action, Action::Execute, "{rule:?}");
        assert_eq!(rule.effect, Effect::Allow, "{rule:?}");
        assert!(matches!(rule.resource, Resource::Command(_)), "{rule:?}");
    }
}

/// The rows of the read-only table, numbered from `first`, as `defaults.md` holds them.
fn table(first: usize) -> String {
    let code = |words: &[String]| {
        let words: Vec<String> =
            words.iter().map(|word| format!("`{}`", word.replace('|', "\\|"))).collect();
        words.join(" ")
    };
    defaults::read_only()
        .iter()
        .enumerate()
        .map(|(offset, pattern)| {
            let max = pattern.max_operands.map(|max| max.to_string()).unwrap_or_default();
            format!(
                "| {} | `{}` | {} | {} | {} |\n",
                first + offset,
                pattern.program,
                code(&pattern.args),
                code(&pattern.forbid),
                max
            )
        })
        .collect()
}

#[test]
fn the_documented_table_is_the_data() {
    let documented = include_str!("defaults.md");
    assert_eq!(documented, table(8), "regenerate src/policy/defaults.md from the rows");
}

#[test]
fn the_permissions_doc_holds_the_same_table() {
    let doc = include_str!("../../../../docs/permissions.md");
    let header = "| # | Program | Args | Forbid | Max |\n|---|---|---|---|---|\n";
    assert!(
        doc.contains(&format!("{header}{}\n", table(8))),
        "copy src/policy/defaults.md under the read-only table of docs/permissions.md"
    );
}

#[test]
fn the_last_matching_rule_wins() {
    let allow_then_deny = Policy::new(vec![
        Rule::new(Action::Write, Resource::Any, Effect::Allow),
        Rule::new(Action::Write, Resource::Under("~/.config".into()), Effect::Deny),
    ])
    .unwrap();
    let target = write("/home/u/.config/a", PathClass::UserConfig);
    assert_eq!(allow_then_deny.last_match(&target, &cx()), Some((1, Effect::Deny)));

    let deny_then_allow =
        Policy::new(allow_then_deny.rules().iter().rev().cloned().collect()).unwrap();
    assert_eq!(deny_then_allow.last_match(&target, &cx()), Some((1, Effect::Allow)));

    let other = write("/home/u/notes", PathClass::UserData);
    assert_eq!(allow_then_deny.last_match(&other, &cx()), Some((0, Effect::Allow)));
}

#[test]
fn an_empty_policy_matches_nothing() {
    assert_eq!(Policy::empty().last_match(&read("/etc/hosts", PathClass::System), &cx()), None);
}

#[test]
fn then_appends_later_rules_that_win() {
    let count = Policy::defaults().rules().len();
    let later = Policy::new(vec![Rule::new(Action::Read, Resource::Any, Effect::Deny)]).unwrap();
    let policy = Policy::defaults().then(later);
    assert_eq!(policy.rules().len(), count + 1);
    assert_eq!(
        policy.last_match(&read("/etc/hosts", PathClass::System), &cx()),
        Some((count, Effect::Deny))
    );
}

#[test]
fn push_checks_the_rule_at_its_position() {
    let mut policy = Policy::defaults();
    let count = policy.rules().len();
    let bad = Rule::new(Action::Write, Resource::Under("relative".into()), Effect::Allow);
    assert_eq!(
        policy.push(bad),
        Err(PermissionsError::RulePathNotAbsolute { index: count, path: "relative".into() })
    );
    policy.push(Rule::new(Action::Network, Resource::Any, Effect::Allow)).unwrap();
    assert_eq!(policy.last_match(&Target::Network, &cx()), Some((count, Effect::Allow)));
}

#[rstest]
#[case::any_any(Action::Any, Resource::Any, true, true, true, true, true)]
#[case::read_any(Action::Read, Resource::Any, true, false, false, false, false)]
#[case::write_any(Action::Write, Resource::Any, false, true, false, false, false)]
#[case::execute_any(Action::Execute, Resource::Any, false, false, true, true, false)]
#[case::network_any(Action::Network, Resource::Any, false, false, false, false, true)]
#[case::any_class(
    Action::Any,
    Resource::Class(PathClass::UserData),
    true,
    true,
    false,
    false,
    false
)]
#[case::any_under(Action::Any, Resource::Under("/home/u/p".into()), true, true, false, false, false)]
#[case::any_project(Action::Any, Resource::Project, true, true, false, false, false)]
#[case::any_command(
    Action::Any,
    Resource::Command(CommandPattern::new("ls")),
    false,
    false,
    true,
    false,
    false
)]
fn actions_and_resources_select_targets(
    #[case] action: Action,
    #[case] resource: Resource,
    #[case] reads: bool,
    #[case] writes: bool,
    #[case] runs: bool,
    #[case] runs_opaque: bool,
    #[case] networks: bool,
) {
    let policy = Policy::new(vec![Rule::new(action, resource, Effect::Allow)]).unwrap();
    let matched = |target: Target<'_>| policy.last_match(&target, &cx()).is_some();
    let ls = words("ls -la");
    assert_eq!(matched(read("/home/u/p/app/a", PathClass::UserData)), reads);
    let tree = Target::Path {
        path: Path::new("/home/u/p/app"),
        access: Access::ReadTree,
        class: PathClass::UserData,
    };
    assert_eq!(matched(tree), reads);
    assert_eq!(matched(write("/home/u/p/app/a", PathClass::UserData)), writes);
    assert_eq!(matched(Target::Command { words: &ls, privileged: false, dir: None }), runs);
    assert_eq!(matched(Target::Opaque), runs_opaque);
    assert_eq!(matched(Target::Network), networks);
}

#[test]
fn a_privileged_command_matches_only_rules_for_every_command() {
    let sudo = words("sudo ls");
    let target = Target::Command { words: &sudo, privileged: true, dir: None };
    let pattern = Policy::new(vec![Rule::new(
        Action::Execute,
        Resource::Command(CommandPattern::new("sudo")),
        Effect::Allow,
    )])
    .unwrap();
    assert_eq!(pattern.last_match(&target, &cx()), None);
    let any = Policy::new(vec![Rule::new(Action::Execute, Resource::Any, Effect::Ask)]).unwrap();
    assert_eq!(any.last_match(&target, &cx()), Some((0, Effect::Ask)));
}

#[rstest]
#[case::inside("/home/u/.config/nvim/init.lua", true)]
#[case::the_root_itself("/home/u/.config/nvim", true)]
#[case::sibling_prefix("/home/u/.config/nvim-old/init.lua", false)]
#[case::parent("/home/u/.config", false)]
fn under_matches_by_component(#[case] path: &'static str, #[case] expected: bool) {
    for root in ["~/.config/nvim", "/home/u/.config/nvim", "/home/u/.config/./nvim/"] {
        let policy = Policy::new(vec![Rule::new(
            Action::Write,
            Resource::Under(root.into()),
            Effect::Allow,
        )])
        .unwrap();
        let matched = policy.last_match(&write(path, PathClass::UserConfig), &cx()).is_some();
        assert_eq!(matched, expected, "{root} against {path}");
    }
}

#[test]
fn a_bare_tilde_is_the_home_directory() {
    let policy =
        Policy::new(vec![Rule::new(Action::Write, Resource::Under("~".into()), Effect::Allow)])
            .unwrap();
    assert!(policy.last_match(&write("/home/u/a", PathClass::UserData), &cx()).is_some());
    assert!(policy.last_match(&write("/etc/a", PathClass::System), &cx()).is_none());
}

#[test]
fn project_matches_only_inside_the_widening_root() {
    let policy =
        Policy::new(vec![Rule::new(Action::Write, Resource::Project, Effect::Allow)]).unwrap();
    let inside = write("/home/u/p/app/src/main.rs", PathClass::UserData);
    let outside = write("/home/u/p/other/a", PathClass::UserData);
    assert!(policy.last_match(&inside, &cx()).is_some());
    assert!(policy.last_match(&outside, &cx()).is_none());
    let no_project = MatchContext { home: Path::new(HOME), home_aliases: &[], project_root: None };
    assert!(policy.last_match(&inside, &no_project).is_none());
}

#[test]
fn under_and_project_rules_match_both_forms_of_a_linked_home() {
    let policy = Policy::new(vec![
        Rule::new(Action::Write, Resource::Under("/var/home/u/.config/nvim".into()), Effect::Allow),
        Rule::new(Action::Write, Resource::Under("~/.config/zsh".into()), Effect::Allow),
        Rule::new(Action::Write, Resource::Project, Effect::Ask),
    ])
    .unwrap();
    let aliases = [PathBuf::from("/var/home/u")];
    let cx = MatchContext {
        home: Path::new(HOME),
        home_aliases: &aliases,
        project_root: Some(Path::new("/home/u/p/app")),
    };
    let rule = |path: &'static str| policy.last_match(&write(path, PathClass::UserConfig), &cx);
    assert_eq!(rule("/home/u/.config/nvim/init.lua"), Some((0, Effect::Allow)));
    assert_eq!(rule("/var/home/u/.config/nvim/init.lua"), Some((0, Effect::Allow)));
    assert_eq!(rule("/var/home/u/.config/zsh/aliases.zsh"), Some((1, Effect::Allow)));
    assert_eq!(rule("/var/home/u/p/app/src/main.rs"), Some((2, Effect::Ask)));
    assert_eq!(rule("/var/home/u/.config/fish/config.fish"), None);
    assert_eq!(rule("/var/home/u2/.config/nvim/init.lua"), None);
}

#[rstest]
#[case::program_alone("ls", "ls", &[], true)]
#[case::extra_args("ls", "ls -la /etc", &[], true)]
#[case::tabs_and_spaces("git", " \tgit\t status ", &["status"], true)]
#[case::args_prefix("git", "git status --short", &["status"], true)]
#[case::quoted_words("git", "'git' \"status\"", &["status"], true)]
#[case::quoted_spaces("ls", "ls 'a b'", &[], true)]
#[case::tilde("ls", "ls ~ ~/p", &[], true)]
#[case::null_redirect("ls", "ls 2>/dev/null", &[], true)]
#[case::harmless_assignment("git", "LC_ALL=C git status", &["status"], true)]
#[case::other_args("git", "git push", &["status"], false)]
#[case::too_short("git", "git", &["status"], false)]
#[case::similar_program("ls", "lsblk", &[], false)]
#[case::path_program("ls", "/bin/ls", &[], false)]
#[case::option_with_value("git", "git log --format=%H -n1", &["log"], true)]
#[case::semicolon("ls", "ls; rm -rf ~", &[], false)]
#[case::and("ls", "ls && rm -rf x", &[], false)]
#[case::pipe("ls", "ls | sh", &[], false)]
#[case::redirect("ls", "ls > /etc/passwd", &[], false)]
#[case::substitution("ls", "ls $(rm -rf x)", &[], false)]
#[case::backticks("ls", "ls `rm -rf x`", &[], false)]
#[case::variable("ls", "ls $HOME", &[], false)]
#[case::leading_glob("ls", "ls *", &[], false)]
#[case::newline("ls", "ls\nrm -rf x", &[], false)]
#[case::background("ls", "ls &", &[], false)]
#[case::assignment_first("git", "GIT_PAGER=x git status", &[], false)]
#[case::unicode("ls", "ls \u{e9}t\u{e9}", &[], false)]
#[case::privileged("sudo", "sudo ls", &[], false)]
#[case::empty("ls", "", &[], false)]
fn command_patterns_match_one_plain_simple_command(
    #[case] program: &str,
    #[case] line: &str,
    #[case] args: &[&str],
    #[case] expected: bool,
) {
    let pattern = CommandPattern::new(program).with_args(args.iter().copied());
    assert_eq!(pattern.matches(line), expected, "{line:?}");
}

#[rstest]
#[case::first_alternative("git status", true)]
#[case::last_alternative("git log -1", true)]
#[case::not_an_alternative("git push", false)]
#[case::alternatives_are_whole_words("git statuses", false)]
fn args_may_list_alternatives(#[case] line: &str, #[case] expected: bool) {
    let pattern = CommandPattern::new("git").with_args(["status|diff|log"]);
    assert_eq!(pattern.matches_words(&words(line)), expected, "{line:?}");
}

#[rstest]
#[case::bare("pacman -Q", true)]
#[case::with_letters("pacman -Qi zsh", true)]
#[case::long_form("pacman --query zsh", true)]
#[case::other_operation("pacman -S zsh", false)]
#[case::not_a_prefix("pacman -q", false)]
fn an_alternative_may_end_in_a_wildcard(#[case] line: &str, #[case] expected: bool) {
    let pattern = CommandPattern::new("pacman").with_args(["-Q*|--query"]);
    assert_eq!(pattern.matches_words(&words(line)), expected, "{line:?}");
}

#[rstest]
// A long option matches itself, its value form and its abbreviations.
#[case::long("--output", "sort --output x", true)]
#[case::long_value("--output", "sort --output=x", true)]
#[case::long_abbreviation("--output", "sort --out=x", true)]
#[case::long_one_letter("--output", "sort --o=x", true)]
#[case::long_other("--output", "git log --oneline", false)]
#[case::long_longer("--output", "sort --output-x", false)]
#[case::end_of_options("--output", "sort -- x", false)]
#[case::long_wildcard("--vacuum*", "journalctl --vacuum-size=1G", true)]
#[case::long_wildcard_abbreviation("--vacuum*", "journalctl --vac", true)]
#[case::long_wildcard_other("--vacuum*", "journalctl --verify", false)]
// One letter after one dash matches inside a cluster of short options.
#[case::short("-o", "sort -o x", true)]
#[case::short_cluster("-o", "sort -uo x", true)]
#[case::short_absent("-o", "sort -nr", false)]
#[case::short_case("-R", "tree -r", false)]
#[case::short_not_long("-o", "sort --zero-terminated", false)]
#[case::short_not_operand("-o", "sort out", false)]
// Several letters after one dash match that word, as find reads its predicates.
#[case::predicate("-delete", "find . -delete", true)]
#[case::predicate_other("-delete", "find . -depth", false)]
#[case::predicate_wildcard("-fprint*", "find . -fprint0 x", true)]
// A word without a dash matches operands that contain it.
#[case::substring("env", "jq -n env", true)]
#[case::substring_inside("ENV", "jq -n $ENV.TOKEN", true)]
#[case::substring_absent("e", "ps aux", false)]
#[case::substring_cluster("e", "ps axe", true)]
#[case::substring_not_options("e", "ps -ef", false)]
fn forbidden_words(#[case] entry: &str, #[case] line: &str, #[case] expected: bool) {
    let words = words(line);
    let pattern = CommandPattern::new(words[0].clone()).with_forbid([entry]);
    assert_eq!(pattern.matches_words(&words), !expected, "{entry} against {line:?}");
}

#[test]
fn forbidden_words_are_checked_in_the_args_too() {
    let pattern = CommandPattern::new("pacman").with_args(["-S*"]).with_forbid(["-y"]);
    assert!(pattern.matches_words(&words("pacman -Ss zsh")));
    assert!(!pattern.matches_words(&words("pacman -Ssy zsh")));
}

#[rstest]
#[case::none("uniq", 0, true)]
#[case::one("uniq -c in.txt", 1, true)]
#[case::two("uniq in.txt out.txt", 2, false)]
#[case::stdin_dash("uniq - out.txt", 2, false)]
#[case::after_double_dash("uniq -- -x out.txt", 2, false)]
#[case::options_do_not_count("uniq -c -d -i", 0, true)]
fn max_operands_counts_operands(#[case] line: &str, #[case] count: usize, #[case] expected: bool) {
    let pattern = CommandPattern::new("uniq").with_max_operands(1);
    assert_eq!(pattern.matches_words(&words(line)), expected, "{line:?} has {count}");
}

#[rstest]
#[case::the_root("/home/u/p/app", true)]
#[case::below("/home/u/p/app/crates/x", true)]
#[case::linked_form("/var/home/u/p/app/src", true)]
#[case::sibling_prefix("/home/u/p/application", false)]
#[case::elsewhere("/home/u/p/other", false)]
fn under_limits_a_command_to_a_directory(#[case] dir: &'static str, #[case] expected: bool) {
    let policy = Policy::new(vec![Rule::new(
        Action::Execute,
        Resource::Command(CommandPattern::new("cargo").with_args(["test"]).with_under("~/p/app")),
        Effect::Allow,
    )])
    .unwrap();
    let aliases = [PathBuf::from("/var/home/u")];
    let cx = MatchContext { home: Path::new(HOME), home_aliases: &aliases, project_root: None };
    let cargo_test = words("cargo test");
    let target =
        Target::Command { words: &cargo_test, privileged: false, dir: Some(Path::new(dir)) };
    assert_eq!(policy.last_match(&target, &cx).is_some(), expected, "{dir}");
    let unknown = Target::Command { words: &cargo_test, privileged: false, dir: None };
    assert_eq!(policy.last_match(&unknown, &cx), None, "an unknown directory matches nothing");
}

#[test]
fn a_pattern_with_a_directory_matches_no_bare_line() {
    let pattern = CommandPattern::new("cargo").with_under("/srv/app");
    assert!(!pattern.matches("cargo test"));
}

#[test]
fn max_operands_counts_after_the_args() {
    let pattern = CommandPattern::new("git").with_args(["remote"]).with_max_operands(0);
    assert!(pattern.matches_words(&words("git remote -v")));
    assert!(!pattern.matches_words(&words("git remote -v add origin x")));
}

#[rstest]
#[case::relative_under(
    Rule::new(Action::Write, Resource::Under("p/app".into()), Effect::Allow),
    PermissionsError::RulePathNotAbsolute { index: 0, path: "p/app".into() }
)]
#[case::other_users_home(
    Rule::new(Action::Write, Resource::Under("~bob/x".into()), Effect::Allow),
    PermissionsError::RulePathNotAbsolute { index: 0, path: "~bob/x".into() }
)]
#[case::empty_program(
    Rule::new(Action::Execute, Resource::Command(CommandPattern::new("")), Effect::Allow),
    PermissionsError::RuleProgramInvalid { index: 0, program: String::new() }
)]
#[case::program_with_space(
    Rule::new(Action::Execute, Resource::Command(CommandPattern::new("git status")), Effect::Allow),
    PermissionsError::RuleProgramInvalid { index: 0, program: "git status".into() }
)]
#[case::program_is_an_option(
    Rule::new(Action::Execute, Resource::Command(CommandPattern::new("-rf")), Effect::Allow),
    PermissionsError::RuleProgramInvalid { index: 0, program: "-rf".into() }
)]
#[case::program_is_an_assignment(
    Rule::new(Action::Execute, Resource::Command(CommandPattern::new("=ls")), Effect::Allow),
    PermissionsError::RuleProgramInvalid { index: 0, program: "=ls".into() }
)]
#[case::program_with_alternatives(
    Rule::new(Action::Execute, Resource::Command(CommandPattern::new("ls|rm")), Effect::Allow),
    PermissionsError::RuleProgramInvalid { index: 0, program: "ls|rm".into() }
)]
#[case::argument_with_glob(
    Rule::new(
        Action::Execute,
        Resource::Command(CommandPattern::new("rm").with_args(["*"])),
        Effect::Allow
    ),
    PermissionsError::RuleArgumentInvalid { index: 0, argument: "*".into() }
)]
#[case::argument_with_an_empty_alternative(
    Rule::new(
        Action::Execute,
        Resource::Command(CommandPattern::new("git").with_args(["status|"])),
        Effect::Allow
    ),
    PermissionsError::RuleArgumentInvalid { index: 0, argument: "status|".into() }
)]
#[case::argument_with_a_wildcard_inside(
    Rule::new(
        Action::Execute,
        Resource::Command(CommandPattern::new("git").with_args(["st*tus"])),
        Effect::Allow
    ),
    PermissionsError::RuleArgumentInvalid { index: 0, argument: "st*tus".into() }
)]
#[case::forbid_a_lone_dash(
    Rule::new(
        Action::Execute,
        Resource::Command(CommandPattern::new("sort").with_forbid(["--"])),
        Effect::Allow
    ),
    PermissionsError::RuleForbidInvalid { index: 0, word: "--".into() }
)]
#[case::forbid_a_wildcard_operand(
    Rule::new(
        Action::Execute,
        Resource::Command(CommandPattern::new("jq").with_forbid(["env*"])),
        Effect::Allow
    ),
    PermissionsError::RuleForbidInvalid { index: 0, word: "env*".into() }
)]
#[case::relative_command_directory(
    Rule::new(
        Action::Execute,
        Resource::Command(CommandPattern::new("cargo").with_under("p/app")),
        Effect::Allow
    ),
    PermissionsError::RulePathNotAbsolute { index: 0, path: "p/app".into() }
)]
#[case::forbid_a_space(
    Rule::new(
        Action::Execute,
        Resource::Command(CommandPattern::new("sort").with_forbid(["-o x"])),
        Effect::Allow
    ),
    PermissionsError::RuleForbidInvalid { index: 0, word: "-o x".into() }
)]
#[case::execute_a_class(
    Rule::new(Action::Execute, Resource::Class(PathClass::System), Effect::Allow),
    PermissionsError::RuleNeverMatches { index: 0, action: Action::Execute }
)]
#[case::network_under(
    Rule::new(Action::Network, Resource::Under("/etc".into()), Effect::Allow),
    PermissionsError::RuleNeverMatches { index: 0, action: Action::Network }
)]
#[case::write_a_command(
    Rule::new(Action::Write, Resource::Command(CommandPattern::new("ls")), Effect::Allow),
    PermissionsError::RuleNeverMatches { index: 0, action: Action::Write }
)]
fn invalid_rules_are_refused(#[case] rule: Rule, #[case] expected: PermissionsError) {
    assert_eq!(Policy::new(vec![rule]), Err(expected));
}

#[test]
fn the_error_names_the_position_of_the_rule() {
    let rules = vec![
        Rule::new(Action::Any, Resource::Any, Effect::Ask),
        Rule::new(Action::Execute, Resource::Project, Effect::Allow),
    ];
    assert_eq!(
        Policy::new(rules).unwrap_err().to_string(),
        "rule 1 pairs the action execute with a resource that it never matches"
    );
}

/// The shape a configuration file gives the rules.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct Config {
    rules: Policy,
}

#[test]
fn rules_read_from_toml() {
    let text = r#"
rules = [
    { action = "write", resource = { under = "~/.config/nvim" }, effect = "allow" },
    { action = "execute", resource = { command = { program = "git", args = ["status"] } }, effect = "allow" },
    { action = "any", resource = { class = "user_data" }, effect = "deny" },
    { action = "write", resource = "project", effect = "ask" },
    { action = "network", resource = "any", effect = "allow" },
    { action = "execute", resource = { command = { program = "find", forbid = ["-delete"], max_operands = 2 } }, effect = "allow" },
    { action = "execute", resource = { command = { program = "cargo", args = ["test"], under = "~/p/app" } }, effect = "allow" },
]
"#;
    let config: Config = toml::from_str(text).unwrap();
    let expected = Policy::new(vec![
        Rule::new(Action::Write, Resource::Under("~/.config/nvim".into()), Effect::Allow),
        Rule::new(
            Action::Execute,
            Resource::Command(CommandPattern::new("git").with_args(["status"])),
            Effect::Allow,
        ),
        Rule::new(Action::Any, Resource::Class(PathClass::UserData), Effect::Deny),
        Rule::new(Action::Write, Resource::Project, Effect::Ask),
        Rule::new(Action::Network, Resource::Any, Effect::Allow),
        Rule::new(
            Action::Execute,
            Resource::Command(
                CommandPattern::new("find").with_forbid(["-delete"]).with_max_operands(2),
            ),
            Effect::Allow,
        ),
        Rule::new(
            Action::Execute,
            Resource::Command(
                CommandPattern::new("cargo").with_args(["test"]).with_under("~/p/app"),
            ),
            Effect::Allow,
        ),
    ])
    .unwrap();
    assert_eq!(config.rules, expected);
}

#[test]
fn rules_round_trip_through_toml() {
    let config = Config { rules: Policy::defaults() };
    let text = toml::to_string(&config).unwrap();
    assert_eq!(toml::from_str::<Config>(&text).unwrap(), config);
}

#[test]
fn reading_checks_the_rules() {
    let text =
        r#"rules = [{ action = "execute", resource = { class = "system" }, effect = "allow" }]"#;
    let error = toml::from_str::<Config>(text).unwrap_err().to_string();
    assert!(error.contains("rule 0 pairs the action execute"), "{error}");
}

#[rstest]
#[case::unknown_rule_key(
    r#"rules = [{ action = "read", resource = "any", effect = "allow", why = "x" }]"#
)]
#[case::unknown_pattern_key(
    r#"rules = [{ action = "execute", resource = { command = { program = "ls", flags = [] } }, effect = "allow" }]"#
)]
#[case::unknown_effect(r#"rules = [{ action = "read", resource = "any", effect = "maybe" }]"#)]
#[case::unknown_class(
    r#"rules = [{ action = "read", resource = { class = "home" }, effect = "allow" }]"#
)]
#[case::negative_operands(
    r#"rules = [{ action = "execute", resource = { command = { program = "ls", max_operands = -1 } }, effect = "allow" }]"#
)]
fn unknown_keys_and_values_are_refused(#[case] text: &str) {
    assert!(toml::from_str::<Config>(text).is_err());
}

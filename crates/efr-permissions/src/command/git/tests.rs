//! The table of git's global options, the exact parse and the views of a rule.

use std::path::{Path, PathBuf};

use pretty_assertions::assert_eq;
use rstest::rstest;

use super::{Class, Form, GitDirs, OPTIONS, exact, lookup, plain_view, views};

/// The words of `line`, split on spaces; a `+` inside a word stands for a space.
fn words(line: &str) -> Vec<String> {
    line.split(' ').map(|word| word.replace('+', " ")).collect()
}

/// The subcommand of `git <line>` by the exact parse.
fn sub(line: &str) -> Option<String> {
    let args = words(line);
    exact(&args).sub.map(|at| args[at].clone())
}

/// The views of `git <line>`, each joined by spaces.
fn view_lines(line: &str) -> Vec<String> {
    let mut lines: Vec<String> =
        views(&words(&format!("git {line}"))).words.iter().map(|view| view.join(" ")).collect();
    lines.sort();
    lines
}

#[rstest]
// Value as the next word.
#[case::dir("-C /x push")]
#[case::config("-c k=v push")]
#[case::git_dir("--git-dir /x/.git push")]
#[case::work_tree("--work-tree /x push")]
#[case::namespace("--namespace n push")]
#[case::config_env("--config-env k=V push")]
#[case::attr_source("--attr-source HEAD push")]
#[case::shallow_file("--shallow-file f push")]
// Value after `=`.
#[case::git_dir_joined("--git-dir=/x/.git push")]
#[case::work_tree_joined("--work-tree=/x push")]
#[case::namespace_joined("--namespace=n push")]
#[case::config_env_joined("--config-env=k=V push")]
#[case::attr_source_joined("--attr-source=HEAD push")]
#[case::exec_path_joined("--exec-path=/x push")]
#[case::list_cmds_joined("--list-cmds=main push")]
// No value.
#[case::paginate_short("-p push")]
#[case::paginate("--paginate push")]
#[case::no_pager_short("-P push")]
#[case::no_pager("--no-pager push")]
#[case::bare("--bare push")]
#[case::no_replace_objects("--no-replace-objects push")]
#[case::no_lazy_fetch("--no-lazy-fetch push")]
#[case::literal_pathspecs("--literal-pathspecs push")]
#[case::no_literal_pathspecs("--no-literal-pathspecs push")]
#[case::glob_pathspecs("--glob-pathspecs push")]
#[case::noglob_pathspecs("--noglob-pathspecs push")]
#[case::icase_pathspecs("--icase-pathspecs push")]
#[case::no_optional_locks("--no-optional-locks push")]
#[case::no_advice("--no-advice push")]
// Prints and exits: read as a flag.
#[case::version_short("-v push")]
#[case::version("--version push")]
#[case::help_short("-h push")]
#[case::help("--help push")]
#[case::exec_path("--exec-path push")]
#[case::html_path("--html-path push")]
#[case::man_path("--man-path push")]
#[case::info_path("--info-path push")]
// Several.
#[case::several("-C a -C b -c x=y --no-pager --git-dir=/g push")]
fn each_option_of_the_table_gives_the_subcommand(#[case] line: &str) {
    let args = words(line);
    let parse = exact(&args);
    assert_eq!(parse.sub, Some(args.len() - 1), "{line}");
    assert!(parse.globals.iter().all(|global| global.option.is_some()), "{line}");
    assert_eq!(view_lines(line), ["git push"], "{line}");
}

#[test]
fn every_option_of_the_table_is_found_by_its_form() {
    for option in OPTIONS {
        let word = match option.form {
            Form::Joined => format!("{}=x", option.name),
            Form::Next | Form::Flag => option.name.to_owned(),
        };
        assert_eq!(lookup(&word), Some(option), "{word}");
    }
}

#[rstest]
#[case::joined_dir("-C.")]
#[case::joined_config("-ca.b=c")]
#[case::shallow_file_joined("--shallow-file=x")]
#[case::list_cmds_separate("--list-cmds")]
#[case::abbreviation("--no-pag")]
#[case::super_prefix("--super-prefix=x")]
#[case::double_dash("--")]
#[case::bare_with_value("--bare=x")]
fn a_form_that_git_refuses_is_unknown(#[case] word: &str) {
    assert_eq!(lookup(word), None, "{word}");
    let args = words(&format!("{word} push"));
    let parse = exact(&args);
    assert_eq!(parse.globals[0].option, None, "{word}");
    // The exits read it as a flag, as before.
    assert_eq!(parse.sub, Some(1), "{word}");
}

#[rstest]
#[case::no_options("push origin", Some("push"))]
#[case::value_that_looks_like_a_subcommand("-C push status", Some("status"))]
#[case::config_value("-c alias.x=push status", Some("status"))]
#[case::attr_source("--attr-source HEAD push", Some("push"))]
#[case::config_env("--config-env a.b=HOME -C ~ clean -fdx", Some("clean"))]
#[case::shallow_file("--shallow-file f -C x fetch", Some("fetch"))]
#[case::no_subcommand("-C x", None)]
#[case::value_missing("-C", None)]
#[case::options_only("--no-pager --bare", None)]
fn exact_parse_finds_the_subcommand(#[case] line: &str, #[case] expected: Option<&str>) {
    assert_eq!(sub(line).as_deref(), expected, "{line}");
}

#[test]
fn exact_parse_names_each_global_and_its_value() {
    let args = words("-C a --work-tree=b -c k=v --no-pager --zz push");
    let parse = exact(&args);
    let named: Vec<(&str, Option<&str>)> = parse
        .globals
        .iter()
        .map(|global| {
            let name = global.option.map_or(args[global.at].as_str(), |option| option.name);
            (name, global.value(&args))
        })
        .collect();
    assert_eq!(
        named,
        [
            ("-C", Some("a")),
            ("--work-tree", Some("b")),
            ("-c", Some("k=v")),
            ("--no-pager", None),
            ("--zz", None),
        ]
    );
    assert_eq!(parse.sub, Some(7));
}

#[test]
fn the_plain_class_is_the_list_of_dl23() {
    let mut plain: Vec<&str> = OPTIONS
        .iter()
        .filter(|option| option.class == Class::Plain)
        .map(|option| option.name)
        .collect();
    plain.sort_unstable();
    let mut expected = [
        "--no-pager",
        "-P",
        "--no-optional-locks",
        "--no-advice",
        "--no-lazy-fetch",
        "--literal-pathspecs",
        "--no-literal-pathspecs",
        "--glob-pathspecs",
        "--noglob-pathspecs",
        "--icase-pathspecs",
    ];
    expected.sort_unstable();
    assert_eq!(plain, expected);
    // Each plain option is a flag.
    assert!(OPTIONS.iter().filter(|o| o.class == Class::Plain).all(|o| o.form == Form::Flag));
}

#[rstest]
#[case::no_pager("git --no-pager log", Some("git log"))]
#[case::no_pager_short("git -P log -1", Some("git log -1"))]
#[case::several_plain("git --no-optional-locks --literal-pathspecs status", Some("git status"))]
#[case::no_options("git log", None)]
#[case::dir("git -C /x log", None)]
#[case::config("git -c core.pager=x log", None)]
#[case::exec_path("git --exec-path=/x log", None)]
#[case::paginate("git -p log", None)]
#[case::version("git --version log", None)]
#[case::plain_then_dir("git --no-pager -C /x log", None)]
#[case::unknown("git --zz log", None)]
#[case::no_subcommand("git --no-pager", None)]
fn plain_view_skips_only_the_plain_options(#[case] line: &str, #[case] expected: Option<&str>) {
    let view = plain_view(&words(line)).map(|view| view.join(" "));
    assert_eq!(view.as_deref(), expected, "{line}");
}

#[test]
fn views_of_unknown_options() {
    // One: read as a flag and as an option with a value.
    assert_eq!(view_lines("--zz push"), ["git push"]);
    assert_eq!(view_lines("--zz x push"), ["git push", "git x push"]);
    // Two: four readings.
    assert_eq!(view_lines("--zz a --yy b push"), ["git a --yy b push", "git b push", "git push"]);
    // Five: after four, every later word that does not start with `-` may be the
    // subcommand.
    let line = "--u1 --u2 --u3 --u4 --u5 a b push";
    let lines = view_lines(line);
    for expected in ["git a b push", "git b push", "git push"] {
        assert!(lines.contains(&expected.to_owned()), "{expected} in {lines:?}");
    }
    let many = "--u1 v1 --u2 v2 --u3 v3 --u4 v4 --u5 v5 --u6 v6 push";
    assert!(view_lines(many).contains(&"git push".to_owned()), "{:?}", view_lines(many));
}

#[rstest]
#[case::alias("-c alias.p=push p", &["git p", "git push"], false)]
#[case::alias_upper("-c alias.P=push p", &["git p", "git push"], false)]
#[case::alias_section_upper("-c ALIAS.p=push p origin", &["git p origin", "git push origin"], false)]
#[case::alias_words("-c alias.p=push+--force p", &["git p", "git push --force"], false)]
#[case::alias_with_options("-c alias.p=-C/x p", &["git p"], false)]
#[case::alias_of_alias(
    "-c alias.a=b -c alias.b=push a",
    &["git a", "git b", "git push"],
    false
)]
#[case::alias_with_global(
    "-c alias.p=--no-pager+push p",
    &["git p", "git push"],
    false
)]
#[case::other_name("-c alias.x=push status", &["git status"], false)]
#[case::other_key("-c core.pager=push p", &["git p"], false)]
#[case::last_one_wins("-c alias.p=log -c alias.p=push p", &["git p", "git push"], false)]
#[case::shell("-c alias.p=!git p", &["git p"], true)]
#[case::from_env("--config-env=alias.p=X p", &["git p"], true)]
#[case::from_env_separate("--config-env alias.p=X p", &["git p"], true)]
#[case::no_value("-c alias.p p", &["git p"], true)]
#[case::empty_value("-c alias.p= p", &["git p"], true)]
fn views_read_an_alias_of_the_line(
    #[case] line: &str,
    #[case] expected: &[&str],
    #[case] unreadable: bool,
) {
    let mut expected: Vec<String> = expected.iter().map(|line| (*line).to_owned()).collect();
    expected.sort();
    assert_eq!(view_lines(line), expected, "{line}");
    assert_eq!(views(&words(&format!("git {line}"))).unreadable, unreadable, "{line}");
}

#[test]
fn an_alias_with_a_quote_or_a_loop_cannot_be_read() {
    let quoted = vec!["git".to_owned(), "-c".to_owned(), "alias.p=push 'x'".to_owned(), "p".into()];
    assert!(views(&quoted).unreadable);
    let shell = vec!["git".to_owned(), "-c".to_owned(), "alias.p=!git push".to_owned(), "p".into()];
    assert!(views(&shell).unreadable);
    // An alias of itself ends at the depth limit and fails closed.
    assert!(views(&words("git -c alias.p=p p")).unreadable);
}

fn resolve(dirs: &GitDirs, start: Option<&str>) -> Vec<PathBuf> {
    dirs.resolve(start.map(Path::new), Path::new("/home/u"))
}

#[test]
fn views_carry_the_dirs_of_each_reading() {
    let found = views(&words("git -C ~/work -C app --work-tree=tree push")).dirs;
    assert_eq!(
        found,
        [GitDirs { chain: vec!["~/work".into(), "app".into()], work_tree: Some("tree".into()) }]
    );
    assert_eq!(
        resolve(&found[0], None),
        [PathBuf::from("/home/u/work/app"), PathBuf::from("/home/u/work/app/tree")]
    );
}

#[rstest]
#[case::absolute(&["/tmp"], None, Some("/srv"), &["/tmp"])]
#[case::relative(&["app"], None, Some("/home/u/work"), &["/home/u/work/app"])]
#[case::relative_unknown(&["app"], None, None, &[])]
#[case::chain(&["/a", "b", "../c"], None, None, &["/a/c"])]
#[case::home(&["~"], None, None, &["/home/u"])]
#[case::other_user(&["~x"], None, Some("/srv"), &[])]
#[case::work_tree_alone(&[], Some("/w"), None, &["/w"])]
#[case::work_tree_after_dir(&["/a"], Some("w"), None, &["/a", "/a/w"])]
#[case::nothing(&[], None, Some("/srv"), &[])]
#[case::empty_dir_is_ignored(&[""], None, Some("/srv"), &["/srv"])]
fn git_dirs_resolve_as_git_moves(
    #[case] chain: &[&str],
    #[case] work_tree: Option<&str>,
    #[case] start: Option<&str>,
    #[case] expected: &[&str],
) {
    let dirs = GitDirs {
        chain: chain.iter().map(|word| (*word).to_owned()).collect(),
        work_tree: work_tree.map(str::to_owned),
    };
    let expected: Vec<PathBuf> = expected.iter().map(PathBuf::from).collect();
    assert_eq!(resolve(&dirs, start), expected);
}

//! The readings of a command of `systemctl`, `cargo`, `docker` and `kubectl` past their
//! own options (OQL4).

use pretty_assertions::assert_eq;
use rstest::rstest;

use super::{Kind, TABLES, readings, table};

fn words(line: &str) -> Vec<String> {
    line.split(' ').map(str::to_owned).collect()
}

/// The words of each reading of `line` (program first), as the rules read them.
fn read(line: &str) -> (Vec<String>, bool) {
    let all = words(line);
    let (program, args) = all.split_first().unwrap();
    let out = readings(table(program).unwrap(), args);
    let mut views: Vec<String> = out.subcommands.iter().map(|at| args[*at..].join(" ")).collect();
    views.sort();
    (views, out.unreadable)
}

#[rstest]
#[case::systemctl_user("systemctl --user restart x", &["restart x"])]
#[case::systemctl_cluster("systemctl -qH host restart x", &["restart x"])]
#[case::systemctl_joined_short("systemctl -Hhost restart x", &["restart x"])]
#[case::systemctl_joined_long("systemctl --host=h restart x", &["restart x"])]
#[case::systemctl_value("systemctl -H h --user stop x", &["stop x"])]
#[case::systemctl_dash_dash("systemctl --user -- restart x", &["restart x"])]
#[case::cargo_toolchain("cargo +nightly publish", &["publish"])]
#[case::cargo_toolchain_and_options("cargo +1.91 -vv --locked publish", &["publish"])]
#[case::cargo_config_setting("cargo --config build.jobs=4 publish", &["publish"])]
#[case::cargo_dir("cargo -C ../x publish", &["publish"])]
#[case::cargo_z("cargo -Zunstable-options publish", &["publish"])]
#[case::docker_host("docker -H tcp://h:2375 push i", &["push i"])]
#[case::docker_context("docker --context prod push i", &["push i"])]
#[case::docker_flags("docker -D --tls push i", &["push i"])]
#[case::kubectl_namespace("kubectl -n prod delete pod x", &["delete pod x"])]
#[case::kubectl_context("kubectl --context=prod delete pod x", &["delete pod x"])]
#[case::kubectl_kubeconfig("kubectl --kubeconfig /k delete pod x", &["delete pod x"])]
#[case::kubectl_level("kubectl -v 6 delete pod x", &["delete pod x"])]
// An option that the table does not know is read as a flag and with a value.
#[case::unknown_flag("systemctl --zz restart x", &["restart x", "x"])]
#[case::unknown_value("docker --zz h push i", &["h push i", "push i"])]
#[case::abbreviation("systemctl --us restart x", &["restart x", "x"])]
#[case::unknown_letter("systemctl -qz restart x", &["restart x", "x"])]
// No subcommand.
#[case::value_at_the_end("kubectl -n", &[])]
#[case::only_options("systemctl --user", &[])]
fn readings_skip_the_programs_own_options(#[case] line: &str, #[case] expected: &[&str]) {
    let (views, unreadable) = read(line);
    assert_eq!(views, expected, "{line:?}");
    assert!(!unreadable, "{line:?}");
}

#[rstest]
#[case::cargo_alias("cargo --config alias.p=publish p")]
#[case::cargo_alias_table("cargo --config=alias={p=\"publish\"} p")]
#[case::cargo_config_file("cargo --config extra.toml p")]
#[case::kubectl_kuberc("kubectl --kuberc /k p")]
fn an_option_that_may_set_an_alias_is_unreadable(#[case] line: &str) {
    assert!(read(line).1, "{line:?}");
}

#[test]
fn past_four_unknown_options_every_later_word_may_be_the_subcommand() {
    let (views, unreadable) = read("docker --a --b --c --d --e x push i");
    assert!(!unreadable);
    assert_eq!(views, ["i", "push i", "x push i"]);
}

#[test]
fn a_long_line_of_unknown_options_is_unreadable() {
    let mut line = String::from("docker --a --b --c --d --e");
    for at in 0..100 {
        line.push_str(&format!(" w{at}"));
    }
    assert!(read(&line).1);
}

#[test]
fn cargo_counts_the_dir_of_dash_c() {
    let all = words("cargo -C /srv/x +nightly publish");
    let out = readings(table("cargo").unwrap(), &all[1..]);
    assert_eq!(out.dirs.len(), 1);
    assert_eq!(out.dirs[0].chain, ["/srv/x"]);
}

#[test]
fn each_table_names_each_option_once_with_its_dashes() {
    for table in TABLES {
        for (at, option) in table.options.iter().enumerate() {
            let short = option.name.len() == 2 && option.name.starts_with('-');
            let long = option.name.len() > 2 && option.name.starts_with("--");
            assert!(short || long, "{} {}", table.program, option.name);
            assert!(!option.name.contains('='), "{} {}", table.program, option.name);
            let again = table.options[at + 1..].iter().any(|other| other.name == option.name);
            assert!(!again, "{} {}", table.program, option.name);
        }
        let dir = table.dir.and_then(|dir| table.option(dir));
        assert!(table.dir.is_none() || dir.is_some_and(|dir| dir.kind == Kind::Value));
    }
}

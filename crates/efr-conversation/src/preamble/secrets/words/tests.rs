use pretty_assertions::assert_eq;
use rstest::rstest;

use super::commands;

/// The text of each word of each command of `line`.
fn texts(line: &str) -> Vec<Vec<String>> {
    commands(line)
        .into_iter()
        .map(|command| command.into_iter().map(|word| word.text).collect())
        .collect()
}

#[rstest]
#[case::plain("mysql -u root -pabc", &[&["mysql", "-u", "root", "-pabc"][..]])]
#[case::single_quotes("echo 'a b' c", &[&["echo", "a b", "c"][..]])]
#[case::double_quotes(r#"echo "a \"b\" \$c \x""#, &[&["echo", r#"a "b" $c \x"#][..]])]
#[case::ansi_c_quotes(r"echo $'a\tb'", &[&["echo", r"a\tb"][..]])]
#[case::quotes_inside_a_word(r#"-p'a b'"c""#, &[&["-pa bc"][..]])]
#[case::backslash(r"echo a\ b", &[&["echo", "a b"][..]])]
#[case::operators(
    "a 1; b 2 && c 3 | d 4 || e (f) & g\nh",
    &[&["a", "1"][..], &["b", "2"], &["c", "3"], &["d", "4"], &["e"], &["f"], &["g"], &["h"]]
)]
#[case::redirections(
    "mysql -pabc < dump.sql 2>&1 >>log &>all",
    &[&["mysql", "-pabc", "dump.sql", "2", "1", "log", "all"][..]]
)]
#[case::operators_in_quotes("echo 'a; b' \"c | d\"", &[&["echo", "a; b", "c | d"][..]])]
#[case::unclosed_quote("echo 'a b", &[&["echo", "a b"][..]])]
#[case::empty_quotes("echo ''", &[&["echo", ""][..]])]
#[case::empty("  ; ", &[])]
fn a_line_splits_into_commands_and_words(#[case] line: &str, #[case] expected: &[&[&str]]) {
    assert_eq!(texts(line), expected);
}

#[test]
fn a_part_of_a_word_maps_back_to_the_line_without_its_quotes() {
    let line = r#"curl -H 'Authorization: Bearer abc' -u "me:pä\"ss" x"#;
    let commands = commands(line);
    let header = &commands[0][2];
    let at = header.text.find("abc").unwrap();
    assert_eq!(&line[header.line_range(at).unwrap()], "abc");
    let user = &commands[0][4];
    assert_eq!(user.text, r#"me:pä"ss"#);
    assert_eq!(&line[user.line_range(3).unwrap()], r#"pä\"ss"#);
    assert_eq!(&line[user.line_range_to(0, 2).unwrap()], "me");
    assert_eq!(user.line_range(user.text.len()), None, "nothing after the end");
}

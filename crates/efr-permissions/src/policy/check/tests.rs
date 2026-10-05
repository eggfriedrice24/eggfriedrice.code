use pretty_assertions::assert_eq;
use rstest::rstest;

use super::Check;

/// The words after the program of `line`, split on spaces; `_` stands for a space
/// inside one word and `|` for a newline.
fn after(line: &str) -> Vec<String> {
    line.split(' ').skip(1).map(|word| word.replace('_', " ").replace('|', "\n")).collect()
}

#[rstest]
#[case::print_a_line("sed -n 5p f")]
#[case::print_a_range("sed -n 1,20p f")]
#[case::print_to_the_end("sed -n 10,$p f")]
#[case::last_line("sed -n $p f")]
#[case::quiet("sed --quiet 5p f")]
#[case::silent("sed --silent 5p f")]
#[case::regex("sed -n /fn_main/p f")]
#[case::regex_range("sed -n /start/,/end/p f")]
#[case::regex_flags("sed -n /x/Ip f")]
#[case::escaped_slash("sed -n /a\\/b/p f")]
#[case::step("sed -n 0~4p f")]
#[case::relative_range("sed -n /x/,+3p f")]
#[case::multiple_range("sed -n 2,~4p f")]
#[case::negated("sed -n 1!p f")]
#[case::list_and_number("sed -n l;=;l_70 f")]
#[case::quit("sed -n 1,5p;5q f")]
#[case::quit_silently("sed -n 3Q f")]
#[case::newlines("sed -n 1p|2p f")]
#[case::expression("sed -n -e 1p -e 3p f")]
#[case::expression_attached("sed -n -e1p f")]
#[case::expression_in_a_cluster("sed -ne 1p f")]
#[case::long_expression("sed -n --expression=1p f")]
#[case::long_expression_apart("sed -n --expression 1p f")]
#[case::extended("sed -nE /a+/p f")]
#[case::sandbox("sed -n --sandbox --posix -r -u 1p f")]
#[case::options_after_operands("sed 1p f -n")]
#[case::stdin("sed -n 1p")]
#[case::spaces("sed -n _1_,_3_!_p_;_ f")]
fn sed_scripts_that_only_print_pass(#[case] line: &str) {
    assert!(Check::SedPrintOnly.accepts(&after(line)), "{line:?}");
}

#[rstest]
#[case::without_quiet("sed 5p f")]
#[case::in_place("sed -n -i 5p f")]
#[case::in_place_long("sed -n --in-place 5p f")]
#[case::in_place_abbreviated("sed -n --in 5p f")]
#[case::quiet_abbreviated("sed --qui 5p f")]
#[case::in_place_in_a_cluster("sed -ni 5p f")]
#[case::script_file("sed -n -f script.sed f")]
#[case::script_file_long("sed -n --file=script.sed f")]
#[case::separate("sed -n -s 5p f")]
#[case::null_data("sed -n -z 5p f")]
#[case::line_length("sed -n -l 5 5p f")]
#[case::debug("sed -n --debug 5p f")]
#[case::write("sed -n w_out f")]
#[case::write_after_an_address("sed -n 1w_out f")]
#[case::write_first_line("sed -n W_out f")]
#[case::read("sed -n r_other f")]
#[case::read_line("sed -n R_other f")]
#[case::execute("sed -n 1e_date f")]
#[case::execute_bare("sed -n e f")]
#[case::substitute("sed -n s/a/b/p f")]
#[case::substitute_and_write("sed -n s/a/b/w_out f")]
#[case::substitute_and_execute("sed -n s/a/date/e f")]
#[case::append("sed -n a_text f")]
#[case::block("sed -n 1{p} f")]
#[case::comment("sed -n #n f")]
#[case::bracket("sed -n /[/]/p f")]
#[case::bracket_hiding_a_write("sed -n /[/p;/]w_out/p f")]
#[case::open_regex("sed -n /abc f")]
#[case::regex_across_lines("sed -n /a|b/p f")]
#[case::custom_delimiter("sed -n \\,x,p f")]
#[case::step_without_a_step("sed -n 1~p f")]
#[case::two_negations("sed -n 1!!p f")]
#[case::trailing_garbage("sed -n 1px f")]
#[case::no_script("sed -n")]
#[case::empty_expression_value("sed -n -e")]
#[case::second_expression_writes("sed -n -e 1p -e w_out f")]
#[case::unknown_long_value("sed -n --line-length=5 1p f")]
fn sed_scripts_that_could_do_more_fail(#[case] line: &str) {
    assert!(!Check::SedPrintOnly.accepts(&after(line)), "{line:?}");
}

#[rstest]
#[case::branch("main", true)]
#[case::remote_branch("origin/main", true)]
#[case::tag("v1.2.3", true)]
#[case::nested("feature/x-y_z", true)]
#[case::ancestry("HEAD~2", true)]
#[case::parent("HEAD^", true)]
#[case::dot(".", false)]
#[case::dot_slash("./src", false)]
#[case::dotdot("../x", false)]
#[case::range("a..b", false)]
#[case::pathspec_magic(":/", false)]
#[case::glob("*", false)]
#[case::url("https://example.com/x", false)]
#[case::scp_form("git@example.com:x", false)]
#[case::absolute("/srv/repo", false)]
#[case::trailing_slash("src/", false)]
#[case::double_slash("a//b", false)]
#[case::trailing_dot("v1.", false)]
#[case::reflog("main@{1}", false)]
fn ref_names_are_words_git_reads_as_refs(#[case] word: &str, #[case] expected: bool) {
    assert_eq!(Check::RefNames.accepts(&[word.to_owned()]), expected, "{word:?}");
}

#[test]
fn ref_names_allow_options_but_not_the_end_of_options() {
    let words = |line: &str| line.split(' ').map(str::to_owned).collect::<Vec<_>>();
    assert!(Check::RefNames.accepts(&words("-b feature origin/main")));
    assert!(Check::RefNames.accepts(&words("-")));
    assert!(!Check::RefNames.accepts(&words("-- main")));
}

#[test]
fn a_check_has_a_name_and_a_program() {
    assert_eq!(Check::SedPrintOnly.to_string(), "sed_print_only");
    assert_eq!(Check::RefNames.to_string(), "ref_names");
    assert_eq!(Check::SedPrintOnly.program(), Some("sed"));
    assert_eq!(Check::RefNames.program(), None);
}

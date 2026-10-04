//! The command lexer: lines in, simple commands or the construct that stops it out.

use pretty_assertions::assert_eq;
use proptest::prelude::*;
use rstest::rstest;

use super::{Construct, SimpleCommand, analyze, privileged, privileged_anywhere};

fn words(line: &str) -> Vec<Vec<String>> {
    match analyze(line) {
        Ok(commands) => commands.into_iter().map(|command| command.words).collect(),
        Err(construct) => panic!("{line:?} did not split: {construct:?}"),
    }
}

fn construct(line: &str) -> Construct {
    match analyze(line) {
        Ok(commands) => panic!("{line:?} split into {commands:?}"),
        Err(construct) => construct,
    }
}

#[rstest]
#[case::one_word("ls", vec![vec!["ls"]])]
#[case::spaces_and_tabs(" \tls\t -la  ", vec![vec!["ls", "-la"]])]
#[case::semicolon("ls; pwd", vec![vec!["ls"], vec!["pwd"]])]
#[case::trailing_semicolon("ls;", vec![vec!["ls"]])]
#[case::and("ls && pwd", vec![vec!["ls"], vec!["pwd"]])]
#[case::or("ls || pwd", vec![vec!["ls"], vec!["pwd"]])]
#[case::pipe("ls | wc -l", vec![vec!["ls"], vec!["wc", "-l"]])]
#[case::pipe_both("ls |& wc -l", vec![vec!["ls"], vec!["wc", "-l"]])]
#[case::no_spaces("ls&&pwd|wc;id", vec![vec!["ls"], vec!["pwd"], vec!["wc"], vec!["id"]])]
#[case::newlines("ls\n\npwd\n", vec![vec!["ls"], vec!["pwd"]])]
#[case::single_quotes("echo 'a b; $(x) `y` \"z\" !'", vec![vec!["echo", "a b; $(x) `y` \"z\" !"]])]
#[case::double_quotes("echo \"a b; 'c' | d\"", vec![vec!["echo", "a b; 'c' | d"]])]
#[case::escaped_in_double_quotes("echo \"\\$HOME \\\" \\\\ \\a\"", vec![vec!["echo", "$HOME \" \\ \\a"]])]
#[case::backslash("echo a\\;b \\$x \\|", vec![vec!["echo", "a;b", "$x", "|"]])]
#[case::quotes_join_words("git 'st'\"at\"us", vec![vec!["git", "status"]])]
#[case::quoted_program("'ls' -la", vec![vec!["ls", "-la"]])]
#[case::quoted_operators("rg 'a|b' && rg \"c;d\"", vec![vec!["rg", "a|b"], vec!["rg", "c;d"]])]
#[case::non_ascii_in_quotes("cat '\u{e9}t\u{e9}.txt'", vec![vec!["cat", "\u{e9}t\u{e9}.txt"]])]
#[case::quoted_newline("echo 'a\nb'", vec![vec!["echo", "a\nb"]])]
#[case::tilde("ls ~ ~/p", vec![vec!["ls", "~", "~/p"]])]
#[case::quoted_tilde("ls '~user'", vec![vec!["ls", "~user"]])]
#[case::git_revisions("git diff HEAD~1 HEAD^", vec![vec!["git", "diff", "HEAD~1", "HEAD^"]])]
#[case::glob_after_a_plain_start("wc -l src/*.rs ./x?", vec![vec!["wc", "-l", "src/*.rs", "./x?"]])]
#[case::quoted_glob("ls '*' \"-*\"", vec![vec!["ls", "*", "-*"]])]
#[case::option_values("git log --format=%H -n1", vec![vec!["git", "log", "--format=%H", "-n1"]])]
#[case::null_redirect("ls >/dev/null 2>/dev/null", vec![vec!["ls"]])]
#[case::spaced_null_redirect("ls > /dev/null 2> /dev/null", vec![vec!["ls"]])]
#[case::both_to_null("ls &>/dev/null; ls &>>/dev/null; ls >&/dev/null", vec![vec!["ls"], vec!["ls"], vec!["ls"]])]
#[case::append_to_null("ls >>/dev/null", vec![vec!["ls"]])]
#[case::clobber_null("ls >|/dev/null", vec![vec!["ls"]])]
#[case::duplicate("ls 2>&1 >&2 1>&2", vec![vec!["ls"]])]
#[case::redirect_first(">/dev/null ls", vec![vec!["ls"]])]
#[case::input("wc -l < notes.txt", vec![vec!["wc", "-l"]])]
#[case::quoted_null(r#"ls > "/dev/null""#, vec![vec!["ls"]])]
#[case::harmless_assignment("LC_ALL=C TZ=UTC sort", vec![vec!["sort"]])]
#[case::assignment_as_argument("dd if=a", vec![vec!["dd", "if=a"]])]
#[case::quoted_assignment("'PATH=x' ls", vec![vec!["PATH=x", "ls"]])]
#[case::digits_word("head -n 5 2", vec![vec!["head", "-n", "5", "2"]])]
fn lines_that_split(#[case] line: &str, #[case] expected: Vec<Vec<&str>>) {
    let expected: Vec<Vec<String>> = expected
        .iter()
        .map(|command| command.iter().map(|word| (*word).to_owned()).collect())
        .collect();
    assert_eq!(words(line), expected, "{line:?}");
}

#[rstest]
#[case::substitution("ls $(rm -rf x)", Construct::CommandSubstitution)]
#[case::substitution_in_double_quotes("echo \"$(id)\"", Construct::CommandSubstitution)]
#[case::backticks("ls `rm -rf x`", Construct::CommandSubstitution)]
#[case::backticks_in_double_quotes("echo \"`id`\"", Construct::CommandSubstitution)]
#[case::input_process("diff <(ls) x", Construct::ProcessSubstitution)]
#[case::output_process("ls >(cat)", Construct::ProcessSubstitution)]
#[case::zsh_process("cat =(ls)", Construct::ProcessSubstitution)]
#[case::variable("ls $HOME", Construct::Expansion)]
#[case::variable_in_double_quotes("ls \"$HOME\"", Construct::Expansion)]
#[case::braced_variable("ls ${HOME}", Construct::Expansion)]
#[case::arithmetic("echo $((1+1))", Construct::CommandSubstitution)]
#[case::ansi_quote("echo $'a'", Construct::Expansion)]
#[case::history("ls; !rm", Construct::HistoryExpansion)]
#[case::history_in_double_quotes("echo \"!!\"", Construct::HistoryExpansion)]
#[case::escaped_history_in_double_quotes("echo \"\\!\"", Construct::HistoryExpansion)]
#[case::quick_history("^foo^bar", Construct::Character { character: '^' })]
#[case::subshell("(ls)", Construct::Grouping)]
#[case::group("{ ls; }", Construct::Grouping)]
#[case::brace_expansion("ls {a,b}", Construct::Grouping)]
#[case::function("f() ls", Construct::Grouping)]
#[case::glob_qualifier("ls a*(e:x:)", Construct::Grouping)]
#[case::leading_glob("ls *", Construct::Glob)]
#[case::leading_question("ls ?x", Construct::Glob)]
#[case::leading_bracket("ls [ab]x", Construct::Glob)]
#[case::option_glob("ls -*", Construct::Glob)]
#[case::empty_quote_glob("ls ''*", Construct::Glob)]
#[case::here_document("cat <<EOF", Construct::HereDocument)]
#[case::here_string("cat <<< x", Construct::HereDocument)]
#[case::output_file("ls > out.txt", Construct::Redirection)]
#[case::output_file_no_space("ls >out.txt", Construct::Redirection)]
#[case::append_file("ls >> out.txt", Construct::Redirection)]
#[case::error_file("ls 2> err.txt", Construct::Redirection)]
#[case::both_file("ls &> out.txt", Construct::Redirection)]
#[case::both_file_zsh("ls >& out.txt", Construct::Redirection)]
#[case::read_write("cat <> x", Construct::Redirection)]
#[case::close("ls >&-", Construct::Redirection)]
#[case::force_clobber("ls >! out", Construct::Redirection)]
#[case::null_lookalike("ls > /dev/null2", Construct::Redirection)]
#[case::background("ls &", Construct::Background)]
#[case::disown("ls &!", Construct::Background)]
#[case::path_prefix("PATH=/tmp ls", Construct::Assignment)]
#[case::preload("LD_PRELOAD=/tmp/x.so ls", Construct::Assignment)]
#[case::library_path("LD_LIBRARY_PATH=/tmp ls", Construct::Assignment)]
#[case::pager("GIT_PAGER=x git log", Construct::Assignment)]
#[case::append_assignment("PATH+=:/tmp ls", Construct::Assignment)]
#[case::only_assignment("PATH=/tmp", Construct::Assignment)]
#[case::only_harmless_assignment("LC_ALL=C", Construct::Assignment)]
#[case::eval("eval ls", Construct::Builtin { program: "eval".to_owned() })]
#[case::exec("exec ls", Construct::Builtin { program: "exec".to_owned() })]
#[case::source("source x.sh", Construct::Builtin { program: "source".to_owned() })]
#[case::dot(". x.sh", Construct::Builtin { program: ".".to_owned() })]
#[case::alias("alias ls=rm", Construct::Builtin { program: "alias".to_owned() })]
#[case::export("export PATH=/tmp", Construct::Builtin { program: "export".to_owned() })]
#[case::builtin_after_an_operator("ls && eval x", Construct::Builtin { program: "eval".to_owned() })]
#[case::unclosed_single("echo 'a", Construct::Unfinished)]
#[case::unclosed_double("echo \"a", Construct::Unfinished)]
#[case::trailing_backslash("echo a\\", Construct::Unfinished)]
#[case::continuation("echo a\\\nb", Construct::Unfinished)]
#[case::continuation_in_double_quotes("echo \"a\\\nb\"", Construct::Unfinished)]
#[case::comment("ls # x", Construct::Character { character: '#' })]
#[case::other_users_home("ls ~root", Construct::Character { character: '~' })]
#[case::directory_stack("ls ~+", Construct::Character { character: '~' })]
#[case::equals_expansion("cat =ls", Construct::Character { character: '=' })]
#[case::unicode("ls \u{e9}t\u{e9}", Construct::Character { character: '\u{e9}' })]
#[case::escape_character("ls \u{1b}[2J", Construct::Character { character: '\u{1b}' })]
#[case::carriage_return("ls\rrm", Construct::Character { character: '\r' })]
#[case::control_in_quotes("echo '\u{3}'", Construct::Character { character: '\u{3}' })]
#[case::escaped_control("echo \\\u{15}", Construct::Character { character: '\u{15}' })]
#[case::leading_and("&& ls", Construct::Syntax)]
#[case::trailing_pipe("ls |", Construct::Syntax)]
#[case::double_and("ls && && pwd", Construct::Syntax)]
#[case::case_terminator("ls;; pwd", Construct::Syntax)]
#[case::join_then_newline("ls &&\npwd", Construct::Syntax)]
#[case::redirect_without_target("ls >", Construct::Syntax)]
#[case::empty("", Construct::Empty)]
#[case::blank(" \n\t;", Construct::Empty)]
fn lines_that_do_not_split(#[case] line: &str, #[case] expected: Construct) {
    assert_eq!(construct(line), expected, "{line:?}");
}

fn command(words: &[&str]) -> SimpleCommand {
    SimpleCommand { words: words.iter().map(|word| (*word).to_owned()).collect() }
}

#[rstest]
#[case::sudo(&["sudo", "ls"], Some("sudo"))]
#[case::full_path(&["/usr/bin/sudo", "ls"], Some("sudo"))]
#[case::doas(&["doas", "ls"], Some("doas"))]
#[case::su(&["su", "-c", "ls"], Some("su"))]
#[case::pkexec(&["pkexec", "ls"], Some("pkexec"))]
#[case::run0(&["run0", "ls"], Some("run0"))]
#[case::behind_env(&["env", "FOO=1", "sudo", "ls"], Some("sudo"))]
#[case::behind_nice(&["nice", "-n", "5", "doas", "ls"], Some("doas"))]
#[case::named_as_an_argument(&["pacman", "-Qi", "sudo"], None)]
#[case::plain(&["ls", "-la"], None)]
fn privileged_programs(#[case] words: &[&str], #[case] expected: Option<&str>) {
    assert_eq!(privileged(&command(words)), expected);
}

#[test]
fn a_privileged_program_is_found_anywhere_in_a_line_that_did_not_split() {
    assert_eq!(privileged_anywhere("echo $(sudo rm -rf /)"), Some("sudo"));
    assert_eq!(privileged_anywhere("x=`/usr/bin/doas id`"), Some("doas"));
    assert_eq!(privileged_anywhere("echo $(pseudo x)"), None);
}

#[test]
fn constructs_read_as_one_line_each() {
    assert_eq!(Construct::CommandSubstitution.to_string(), "a command substitution");
    assert_eq!(
        Construct::Builtin { program: "eval".to_owned() }.to_string(),
        "the builtin \"eval\""
    );
    assert_eq!(Construct::Character { character: '#' }.to_string(), "the character '#'");
}

/// Splits a line without quotes at `;`, `&&`, `||`, `|` and newlines, knowing nothing
/// else about the shell.
fn naive_parts(line: &str) -> Vec<String> {
    let line = line.replace("&&", "\n").replace("||", "\n").replace([';', '|'], "\n");
    line.lines().filter(|part| !part.trim().is_empty()).map(str::to_owned).collect()
}

/// The quote that is still open at the end of `text`, which has no backslashes.
fn open_quote(text: &str) -> Option<char> {
    let mut open: Option<char> = None;
    for c in text.chars() {
        open = match (open, c) {
            (None, '\'' | '"') => Some(c),
            (Some(quote), c) if c == quote => None,
            (open, _) => open,
        };
    }
    open
}

proptest! {
    /// Text that stays inside single quotes never changes the split.
    #[test]
    fn single_quoted_text_is_one_word(text in "[^'\\x00-\\x1f\\x7f-\\x9f]*") {
        let line = format!("echo '{text}'");
        prop_assert_eq!(words(&line), vec![vec!["echo".to_owned(), text]]);
    }

    /// Plain words joined by operators split into exactly those words.
    #[test]
    fn plain_commands_split_at_every_operator(
        commands in prop::collection::vec(prop::collection::vec("x[a-z0-9./-]{0,6}", 1..4), 1..5),
        operators in prop::collection::vec(prop::sample::select(vec![";", "&&", "||", "|", "\n", " ; ", " && "]), 4),
    ) {
        let mut line = String::new();
        for (index, command) in commands.iter().enumerate() {
            if index > 0 {
                line.push_str(operators[index - 1]);
            }
            line.push_str(&command.join(" "));
        }
        prop_assert_eq!(words(&line), commands.clone());
        prop_assert_eq!(naive_parts(&line).len(), commands.len());
    }

    /// No line with a `$(` or backquote outside single quotes splits.
    #[test]
    fn substitutions_never_split(
        before in "[a-z ;|&'\"]{0,8}",
        after in "[a-z )`'\"]{0,8}",
        opener in prop::sample::select(vec!["$(", "`"]),
    ) {
        let line = format!("{before}{opener}{after}");
        if open_quote(&before) != Some('\'') {
            prop_assert!(analyze(&line).is_err(), "{:?} split", line);
        }
    }
}

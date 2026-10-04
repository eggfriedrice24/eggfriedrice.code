use pretty_assertions::assert_eq;
use proptest::prelude::*;
use rstest::rstest;

use super::{Line, Word, is_assignment, split};

/// The words of each command, inputs after `<` and outputs after `>`.
fn shape(line: &Line) -> Vec<String> {
    line.commands
        .iter()
        .map(|command| {
            let mut parts: Vec<String> =
                command.words.iter().map(|word| word.text.clone()).collect();
            parts.extend(command.inputs.iter().map(|word| format!("<{}", word.text)));
            parts.extend(command.outputs.iter().map(|word| format!(">{}", word.text)));
            parts.join(" ")
        })
        .collect()
}

#[rstest]
#[case::plain("ls -la src", &["ls -la src"], false)]
#[case::operators("ls; pwd && id || wc | sort & echo", &["ls", "pwd", "id", "wc", "sort", "echo"], false)]
#[case::quotes("cat 'a b' \"c d\" e\\ f", &["cat a b c d e f"], false)]
#[case::quoted_operators("rg 'a|b; c' x", &["rg a|b; c x"], false)]
#[case::empty_quotes("echo ''", &["echo "], false)]
#[case::input("wc -l < ~/.ssh/id_rsa", &["wc -l <~/.ssh/id_rsa"], false)]
#[case::output("echo x > out.txt", &["echo x >out.txt"], false)]
#[case::append("echo x >> ~/.zshrc", &["echo x >~/.zshrc"], false)]
#[case::both("make &> log.txt", &["make >log.txt"], false)]
#[case::descriptor_output("make 2> err.txt", &["make >err.txt"], false)]
#[case::descriptor_copy("make 2>&1 >&2", &["make"], false)]
#[case::to_null("ls > /dev/null 2>/dev/null", &["ls"], false)]
#[case::zsh_both_to_file("make >& log.txt", &["make >log.txt"], false)]
#[case::substitution("echo $(cat ~/.ssh/id_rsa)", &["echo", "cat ~/.ssh/id_rsa"], true)]
#[case::quoted_substitution("echo \"$(cat key)\"", &["echo", "cat key"], true)]
#[case::backticks("echo `cat key`", &["echo", "cat key"], true)]
#[case::subshell("(cd /tmp && ls)", &["cd /tmp", "ls"], true)]
#[case::process_substitution("diff <(ls a) b", &["diff", "ls a", "b"], true)]
#[case::here_document("cat <<EOF", &["cat"], true)]
#[case::variable("cat $DIR/.netrc", &["cat $DIR/.netrc"], true)]
#[case::braced_variable("cat ${DIR}/.netrc", &["cat ${DIR}/.netrc"], true)]
#[case::history("ls; !!", &["ls", "!!"], true)]
#[case::unclosed("echo 'a", &["echo a"], true)]
fn splits(#[case] line: &str, #[case] expected: &[&str], #[case] opaque: bool) {
    let split = split(line);
    assert_eq!(shape(&split), expected, "{line:?}");
    assert_eq!(split.opaque, opaque, "{line:?}");
}

fn first_word(line: &str, index: usize) -> Word {
    split(line).commands[0].words[index].clone()
}

#[test]
fn a_leading_tilde_is_the_home_directory_only_outside_quotes() {
    assert!(first_word("ls ~", 1).tilde);
    assert!(first_word("ls ~/p", 1).tilde);
    assert!(!first_word("ls '~/p'", 1).tilde);
    assert!(!first_word("ls ~root", 1).tilde);
    assert!(!first_word("ls a~", 1).tilde);
}

#[test]
fn home_at_the_start_of_a_word_is_the_home_directory() {
    let home = first_word("cat $HOME/.ssh/id_rsa", 1);
    assert_eq!((home.text.as_str(), home.tilde, home.expansion), ("~/.ssh/id_rsa", true, false));
    let braced = first_word("cat \"${HOME}/.netrc\"", 1);
    assert_eq!((braced.text.as_str(), braced.tilde), ("~/.netrc", true));
    assert!(first_word("cat $HOMEDIR/x", 1).expansion);
    assert!(first_word("cat a$HOME", 1).expansion);
    assert!(split("cat $HOME/x").opaque);
}

#[test]
fn patterns_are_found_outside_quotes_only() {
    assert_eq!(first_word("ls src/*.rs", 1).pattern, Some(4));
    assert_eq!(first_word("ls ~/.ss*", 1).pattern, Some(5));
    assert_eq!(first_word("ls '*.rs'", 1).pattern, None);
    assert_eq!(first_word("git diff HEAD~1", 2).pattern, Some(4));
}

#[test]
fn an_expansion_marks_its_word() {
    assert!(first_word("cat $FILE", 1).expansion);
    assert!(first_word("cat \"$FILE\"", 1).expansion);
    assert!(!first_word("cat '$FILE'", 1).expansion);
}

#[test]
fn assignments_come_before_the_program() {
    let line = split("LC_ALL=C TZ=UTC sort -u x");
    let words: Vec<&str> =
        line.commands[0].program_and_args().iter().map(|word| word.text.as_str()).collect();
    assert_eq!(words, ["sort", "-u", "x"]);
    assert!(is_assignment("PATH+=/x"));
    assert!(!is_assignment("--format=x"));
    assert!(!is_assignment("=x"));
}

proptest! {
    /// Any text splits without a panic.
    #[test]
    fn any_text_splits(line in "(?s).{0,40}") {
        let _ = split(&line);
    }
}

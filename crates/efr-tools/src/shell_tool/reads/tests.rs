use pretty_assertions::assert_eq;
use rstest::rstest;

use super::{Depth, Reads, reads};
use crate::shell_tool::words::split;

/// The reads of the first simple command of `line`, as `path:depth` texts and the
/// working directory's depth.
fn of(line: &str) -> (Vec<String>, Option<Depth>) {
    let line = split(line);
    let Reads { named, cwd } = reads(line.commands[0].program_and_args());
    let named = named
        .iter()
        .map(|named| {
            let depth = match named.depth {
                Depth::One => "one",
                Depth::Tree => "tree",
            };
            format!("{}:{depth}", named.text)
        })
        .collect();
    (named, cwd)
}

#[rstest]
// Operands and option values that look like paths, read alone.
#[case::cat("cat a.txt ~/.ssh/id_ed25519", &["a.txt:one", "~/.ssh/id_ed25519:one"], None)]
#[case::option_value("date -f ~/.ssh/id_rsa", &["~/.ssh/id_rsa:one"], None)]
#[case::long_value("wc --files0-from=/tmp/list", &["/tmp/list:one"], None)]
#[case::attached_path("tail -n1 -f/var/log/x", &["/var/log/x:one"], None)]
#[case::after_double_dash("cat -- -n", &["-n:one"], None)]
#[case::stdin("cat -", &[], None)]
#[case::expansion("cat $FILE", &[], None)]
#[case::unknown_program("cargo test --manifest-path=x/Cargo.toml", &["test:one", "x/Cargo.toml:one"], None)]
// Programs whose arguments are not paths.
#[case::echo("echo ~/.ssh/id_rsa", &[], None)]
#[case::printf("printf '%s' ~/.netrc", &[], None)]
#[case::basename("basename ~/.ssh/id_rsa", &[], None)]
// A wrapper reads its words and what its program reads.
#[case::sudo("sudo cat /etc/shadow", &["cat:one", "/etc/shadow:one", "/etc/shadow:one"], None)]
#[case::timeout("timeout 5 rg x src", &["5:one", "rg:one", "x:one", "src:one", "src:tree"], None)]
// ls lists its operands, or the working directory, and recurses with -R.
#[case::ls("ls -la src", &["src:one"], None)]
#[case::ls_cwd("ls -la", &[], Some(Depth::One))]
#[case::ls_recursive("ls -laR ~", &["~:tree"], None)]
#[case::ls_recursive_cwd("ls --recursive", &[], Some(Depth::Tree))]
#[case::ls_value("ls -I '*.o' src", &["src:one"], None)]
#[case::ls_unknown_option("ls --frobnicate src", &["src:one"], Some(Depth::One))]
// rg searches below its paths; its first operand is the pattern.
#[case::rg("rg -n TOKEN ~/.aws", &["~/.aws:tree"], None)]
#[case::rg_cwd("rg -n TOKEN", &[], Some(Depth::Tree))]
#[case::rg_regexp("rg -e TOKEN src", &["src:tree"], None)]
#[case::rg_regexp_cwd("rg -e TOKEN", &[], Some(Depth::Tree))]
#[case::rg_files("rg --files ~/.ssh", &["~/.ssh:tree"], None)]
#[case::rg_value_option("rg -g '*.rs' x src", &["src:tree"], None)]
#[case::rg_pattern_file("rg -f ~/.netrc src", &["src:tree", "~/.netrc:one"], None)]
#[case::rg_unknown_option("rg --frobnicate x src", &["x:tree", "src:tree"], Some(Depth::Tree))]
#[case::rg_unknown_short("rg -W x", &["x:tree"], Some(Depth::Tree))]
// grep reads its files alone, or below them with -r, and the working directory then.
#[case::grep("grep -n x a.txt", &["a.txt:one"], None)]
#[case::grep_stdin("grep x", &[], None)]
#[case::grep_recursive("grep -rn x ~", &["~:tree"], None)]
#[case::grep_recursive_cwd("grep -r x", &[], Some(Depth::Tree))]
#[case::grep_recursive_regexp_cwd("grep -r -e x", &[], Some(Depth::Tree))]
#[case::grep_recursive_long("grep --recursive x ~/.config", &["~/.config:tree"], None)]
#[case::grep_recursive_abbreviated("grep --rec x ~/.config", &["~/.config:tree"], None)]
#[case::grep_directories("grep -d recurse x .", &[".:tree"], None)]
#[case::grep_directories_long("grep --directories=recurse x .", &[".:tree"], None)]
#[case::grep_exclude("grep -r --exclude-dir node_modules TOKEN", &[], Some(Depth::Tree))]
#[case::grep_context("grep -5 x a.txt", &["a.txt:one"], None)]
// find reads below its start points, or the working directory.
#[case::find("find /etc ~/p -name '*.conf'", &["/etc:tree", "~/p:tree"], None)]
#[case::find_cwd("find -name x", &[], Some(Depth::Tree))]
#[case::find_follow("find -L . -type f", &[".:tree"], None)]
// du and tree read below their paths, or the working directory.
#[case::du("du -sh ~/Downloads", &["~/Downloads:tree"], None)]
#[case::du_cwd("du -sh", &[], Some(Depth::Tree))]
#[case::du_depth("du -d 1 /var", &["/var:tree"], None)]
#[case::tree("tree -L 2 src", &["src:tree"], None)]
#[case::tree_cwd("tree -a", &[], Some(Depth::Tree))]
// diff reads below its operands, which may be directories.
#[case::diff("diff -u ~ /tmp/x", &["~:tree", "/tmp/x:tree"], None)]
fn reads_by_program(#[case] line: &str, #[case] expected: &[&str], #[case] cwd: Option<Depth>) {
    let (named, read_cwd) = of(line);
    assert_eq!(named, expected, "{line:?}");
    assert_eq!(read_cwd, cwd, "{line:?}");
}

#[test]
fn a_tilde_option_value_counts_as_the_home_directory() {
    let line = split("date --file=~/.ssh/id_rsa");
    let reads = reads(line.commands[0].program_and_args());
    assert!(reads.named[0].tilde);
}

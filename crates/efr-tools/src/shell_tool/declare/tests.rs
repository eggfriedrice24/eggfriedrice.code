use std::path::{Path, PathBuf};

use pretty_assertions::assert_eq;
use rstest::rstest;

use super::{Declared, declared};
use crate::shell_tool::words::split;

const HOME: &str = "/home/u";
const CWD: &str = "/home/u/p/app";

fn paths(list: &[&str]) -> Vec<PathBuf> {
    list.iter().map(PathBuf::from).collect()
}

fn declare(line: &str) -> Declared {
    declared(&split(line), Path::new(CWD), Path::new(HOME))
}

#[rstest]
#[case::relative("cat src/main.rs ../x", &["/home/u/p/app/src/main.rs", "/home/u/p/x"], &[], &[])]
#[case::tilde("cat ~/.ssh/id_ed25519 ~", &["/home/u/.ssh/id_ed25519", "/home/u"], &[], &[])]
#[case::absolute("cat /etc/hosts", &["/etc/hosts"], &[], &[])]
#[case::quoted_tilde("cat '~/x'", &["/home/u/p/app/~/x"], &[], &[])]
#[case::once("cat a a", &["/home/u/p/app/a"], &[], &[])]
#[case::implicit_cwd("ls", &["/home/u/p/app"], &[], &[])]
#[case::search_cwd("rg TOKEN", &[], &["/home/u/p/app"], &[])]
#[case::search_home("rg TOKEN ~/.aws", &[], &["/home/u/.aws"], &[])]
#[case::glob("cat ~/.ss*/id*", &[], &["/home/u"], &[])]
#[case::relative_glob("wc -l src/*.rs", &[], &["/home/u/p/app/src"], &[])]
#[case::bare_glob("cat x*", &[], &["/home/u/p/app"], &[])]
#[case::root_glob("ls /*", &[], &["/"], &[])]
#[case::input("wc -c < ~/.netrc", &["/home/u/.netrc"], &[], &[])]
#[case::output("echo x >> ~/.zshrc", &[], &[], &["/home/u/.zshrc"])]
#[case::null("ls > /dev/null", &["/home/u/p/app"], &[], &[])]
#[case::inside_substitution("echo $(cat ~/.ssh/id_rsa)", &["/home/u/.ssh/id_rsa"], &[], &[])]
#[case::every_program("cp ~/.ssh/id_rsa /tmp/k", &["/home/u/.ssh/id_rsa", "/tmp/k"], &[], &[])]
#[case::echo_names_nothing("echo ~/.ssh/id_rsa", &[], &[], &[])]
#[case::home_variable("cat $HOME/.ssh/id_rsa", &["/home/u/.ssh/id_rsa"], &[], &[])]
fn declares(
    #[case] line: &str,
    #[case] reads: &[&str],
    #[case] trees: &[&str],
    #[case] writes: &[&str],
) {
    let expected = Declared { reads: paths(reads), trees: paths(trees), writes: paths(writes) };
    assert_eq!(declare(line), expected, "{line:?}");
}

#[rstest]
// A cd adds its target as a directory the rest of the line may run in.
#[case::cd_relative(
    "cd .. && cat id_ed25519",
    &["/home/u/p", "/home/u/p/app/id_ed25519", "/home/u/p/id_ed25519"],
    &[]
)]
#[case::cd_into_secrets("cd ~/.ssh && cat id_ed25519", &["/home/u/.ssh", "/home/u/p/app/id_ed25519", "/home/u/.ssh/id_ed25519"], &[])]
#[case::cd_home("cd && ls", &["/home/u/p/app", "/home/u"], &[])]
#[case::two_steps(
    "cd ~ && cd .ssh && cat x",
    &[
        "/home/u",
        "/home/u/p/app/.ssh",
        "/home/u/.ssh",
        "/home/u/p/app/x",
        "/home/u/x",
        "/home/u/p/app/.ssh/x",
        "/home/u/.ssh/x",
    ],
    &[]
)]
// A cd the text cannot follow makes every relative path anything below /.
#[case::cd_back("cd - && cat id_ed25519", &[], &["/"])]
#[case::cd_variable("cd $DIR && ls", &[], &["/"])]
#[case::popd("popd; cat ~/x", &["/home/u/x"], &[])]
fn follows_cd(#[case] line: &str, #[case] reads: &[&str], #[case] trees: &[&str]) {
    let declared = declare(line);
    assert_eq!(declared.reads, paths(reads), "{line:?}");
    assert_eq!(declared.trees, paths(trees), "{line:?}");
}

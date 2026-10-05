use pretty_assertions::assert_eq;
use rstest::rstest;

use crate::shell_tool::reads::{Depth, Named, Reads, reads};
use crate::shell_tool::words::split;

/// The reads and the writes of the first simple command of `line`.
fn of(line: &str) -> (Vec<String>, Vec<String>) {
    let line = split(line);
    let Reads { named, writes, cwd, .. } = reads(line.commands[0].program_and_args());
    assert_eq!(cwd, None, "a writer reads no working directory it does not name");
    let texts = |list: Vec<Named>| -> Vec<String> {
        list.into_iter()
            .map(|named| match named.depth {
                Depth::One => named.text,
                Depth::Tree => format!("{}/**", named.text),
            })
            .collect()
    };
    (texts(named), texts(writes))
}

#[rstest]
// Every operand is written.
#[case::rm("rm -rf build ~/x", &[], &["build", "~/x"])]
#[case::rmdir("rmdir a b", &[], &["a", "b"])]
#[case::mkdir("mkdir -p src/x", &[], &["src/x"])]
#[case::touch("touch a.txt", &[], &["a.txt"])]
#[case::mv("mv src ~/x", &[], &["src", "~/x"])]
#[case::mv_target("mv -t ~/x a", &[], &["~/x", "a"])]
#[case::chmod("chmod +x run.sh", &[], &["+x", "run.sh"])]
#[case::truncate("truncate -s 0 log", &[], &["0", "log"])]
#[case::tee("tee -a out.log", &[], &["out.log"])]
#[case::after_double_dash("rm -- -f", &[], &["-f"])]
#[case::stdin_and_expansion("tee - $OUT", &[], &[])]
#[case::reference_is_read("chmod --reference=/etc/hosts f", &["/etc/hosts"], &["f"])]
// cp writes its last operand and reads the others.
#[case::cp("cp a b ~/.config/efr/config.toml", &["a/**", "b/**"], &["~/.config/efr/config.toml"])]
#[case::cp_recursive_home("cp -r ~ backup", &["~/**"], &["backup"])]
#[case::cp_one_operand("cp a", &[], &["a"])]
#[case::cp_target_attached("cp -t/srv/x a b", &[], &["/srv/x", "a", "b"])]
#[case::cp_target_apart("cp -t dst a b", &[], &["dst", "a", "b"])]
#[case::cp_target_in_a_cluster("cp -at dst a", &[], &["dst", "a"])]
#[case::cp_target_long("cp --target-directory=/srv/x a", &[], &["/srv/x", "a"])]
#[case::cp_target_abbreviated("cp --targ dst a", &[], &["dst", "a"])]
#[case::cp_no_target("cp --no-target-directory a b", &["a/**"], &["b"])]
// The value of `-S` or `--suffix` is never the destination, wherever it stands.
#[case::cp_suffix_after("cp evil ~/.config/efr/config.toml -S x", &[], &["evil", "~/.config/efr/config.toml"])]
#[case::cp_suffix_long_after("cp evil ~/.bashrc --suffix x", &[], &["evil", "~/.bashrc"])]
#[case::cp_suffix_abbreviated_after("cp evil ~/.bashrc --suf x", &[], &["evil", "~/.bashrc"])]
#[case::cp_suffix_before("cp -S x evil ~/.bashrc", &["evil/**"], &["~/.bashrc"])]
#[case::cp_suffix_attached("cp -S.bak evil ~/.bashrc", &["evil/**"], &["~/.bashrc"])]
#[case::cp_sparse_value("cp --sparse always evil ~/.bashrc", &["evil/**"], &["~/.bashrc"])]
#[case::cp_flag_after("cp -r src dst -v", &[], &["src", "dst"])]
#[case::cp_unknown_option("cp --frobnicate a b", &[], &["a", "b"])]
#[case::cp_ambiguous_abbreviation("cp --s x a b", &[], &["x", "a", "b"])]
#[case::cp_unknown_short_option("cp -rq a b", &[], &["a", "b"])]
#[case::cp_target_with_a_suffix("cp -tS a b", &[], &["S", "a", "b"])]
// ln writes the link and reads its target.
#[case::ln("ln -s ~/.ssh/id_ed25519 key", &["~/.ssh/id_ed25519"], &["key"])]
#[case::ln_into_config("ln -sf x ~/.config/efr/config.toml", &["x"], &["~/.config/efr/config.toml"])]
#[case::ln_one_operand("ln -s ~/.config/efr", &["~/.config/efr"], &["efr"])]
#[case::ln_one_operand_slash("ln -s /srv/x/", &["/srv/x/"], &["x"])]
#[case::ln_one_operand_home("ln -s ~", &["~"], &["."])]
#[case::ln_target("ln -s -t ~/bin a b", &[], &["~/bin", "a", "b"])]
#[case::ln_suffix_after("ln -sf x ~/.zshrc -S y", &[], &["x", "~/.zshrc"])]
#[case::ln_suffix_long_after("ln -sf x ~/.zshrc --suffix y", &[], &["x", "~/.zshrc"])]
#[case::ln_suffix_before("ln -sf -S y x ~/.zshrc", &["x"], &["~/.zshrc"])]
// A hard link writes its source too: the new name writes the same file.
#[case::ln_hard("ln ~/.config/efr/config.toml x", &[], &["~/.config/efr/config.toml", "x"])]
#[case::ln_hard_one_operand("ln ~/.config/efr/config.toml", &[], &["~/.config/efr/config.toml", "config.toml"])]
#[case::ln_symbolic_long("ln --symbolic a b", &["a"], &["b"])]
#[case::ln_symbolic_abbreviated("ln --sy a b", &["a"], &["b"])]
#[case::ln_suffix_is_not_symbolic("ln -Ss a b", &[], &["a", "b"])]
#[case::cp_hard("cp -l a b", &[], &["a/**", "b"])]
#[case::cp_hard_in_a_cluster("cp -al a b", &[], &["a/**", "b"])]
#[case::cp_hard_abbreviated("cp --l a b", &[], &["a/**", "b"])]
// A wrapper writes what its program writes.
#[case::nice("nice rm -rf ~/x", &["rm", "~/x"], &["~/x"])]
// git deletes, moves and creates paths in the work tree.
#[case::git_rm("git rm -r src", &[], &["src"])]
#[case::git_mv("git mv a b", &[], &["a", "b"])]
#[case::git_worktree_add("git worktree add ../wt main", &[], &["../wt", "main"])]
#[case::git_status_reads("git status src", &["status", "src"], &[])]
fn writer_programs_write_their_operands(
    #[case] line: &str,
    #[case] reads: &[&str],
    #[case] writes: &[&str],
) {
    assert_eq!(of(line), (owned(reads), owned(writes)), "{line:?}");
}

fn owned(texts: &[&str]) -> Vec<String> {
    texts.iter().map(|text| (*text).to_owned()).collect()
}

#[test]
fn programs_that_are_not_writers_write_nothing() {
    for line in ["cat a b", "cargo test", "sed -n 1p f", "git checkout main"] {
        assert_eq!(of(line).1, Vec::<String>::new(), "{line:?}");
    }
}

#[test]
fn cp_ln_and_mv_may_relink_and_the_other_writers_may_not() {
    let links = |line: &str| reads(split(line).commands[0].program_and_args()).links;
    for line in ["cp a b", "ln -s a b", "mv a b", "nice mv a b", "git mv a b", "git worktree add w"]
    {
        assert!(links(line), "{line:?}");
    }
    for line in ["rm a", "mkdir a", "touch a", "chmod +x a", "tee a", "cat a"] {
        assert!(!links(line), "{line:?}");
    }
}

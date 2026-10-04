//! Decision tables for command lines: every built-in read-only rule, compound lines,
//! substitutions, privileged programs, secrets that a read-only command would read,
//! and the user's rules over the defaults.

use efr_protocol::{Origin, Scope};
use pretty_assertions::assert_eq;
use proptest::prelude::*;
use rstest::rstest;

use super::{app, engine, input, locations};
use crate::{
    Action, Cause, CommandPattern, Construct, Effect, Engine, Layer, Policy, Requirements,
    Resource, Rule,
};

fn run(line: &str) -> Requirements {
    Requirements::none().with_command(line)
}

fn effect_of(line: &str) -> Effect {
    engine().decide(&input(run(line), Scope::Machine, Origin::Shell)).effect()
}

/// The engine with the defaults and then `rules`, as the daemon builds it from the
/// configuration.
fn configured(rules: Vec<Rule>) -> Engine {
    Engine::new(locations(), Policy::defaults().then(Policy::new(rules).unwrap()))
}

fn allow_command(pattern: CommandPattern) -> Rule {
    Rule::new(Action::Execute, Resource::Command(pattern), Effect::Allow)
}

#[rstest]
#[case::ls("ls -la src")]
#[case::pwd("pwd")]
#[case::cat("cat README.md")]
#[case::head("head -n 20 Cargo.toml")]
#[case::tail("tail -n 50 build.log")]
#[case::wc("wc -l src/main.rs")]
#[case::file("file target/debug/efrd")]
#[case::stat("stat -c %s Cargo.lock")]
#[case::du("du -sh target")]
#[case::df("df -h /")]
#[case::lsblk("lsblk -f")]
#[case::blkid("blkid /dev/nvme0n1p2")]
#[case::findmnt("findmnt /home")]
#[case::free("free -h")]
#[case::uptime("uptime")]
#[case::uname("uname -a")]
#[case::whoami("whoami")]
#[case::id("id -u")]
#[case::groups("groups")]
#[case::hostname("hostname -f")]
#[case::date("date +%F")]
#[case::which("which zsh")]
#[case::type_builtin("type ls")]
#[case::command_v("command -v rg")]
#[case::command_upper_v("command -V rg")]
#[case::echo("echo 'hello world'")]
#[case::printf("printf '%s\\n' a b")]
#[case::realpath("realpath ../x")]
#[case::readlink("readlink -f /usr/bin/sh")]
#[case::basename("basename /a/b.txt .txt")]
#[case::dirname("dirname /a/b.txt")]
#[case::tree("tree -L 2 src")]
#[case::rg("rg -n 'fn main' src")]
#[case::grep("grep -rn TODO src")]
#[case::egrep("egrep 'a|b' notes.txt")]
#[case::fgrep("fgrep x notes.txt")]
#[case::diff("diff -u a.txt b.txt")]
#[case::cmp("cmp a.bin b.bin")]
#[case::sort("sort -u names.txt")]
#[case::uniq("uniq -c names.txt")]
#[case::cut("cut -d: -f1 /etc/passwd")]
#[case::tr("tr a-z A-Z")]
#[case::column("column -t data.tsv")]
#[case::jq("jq '.name' package.json")]
#[case::ps("ps aux")]
#[case::ps_standard("ps -ef")]
#[case::pgrep("pgrep -a nginx")]
#[case::ss("ss -tulpn")]
#[case::ip_addr("ip addr")]
#[case::ip_a("ip a")]
#[case::ip_addr_show("ip address show dev eth0")]
#[case::ip_route("ip route")]
#[case::ip_route_show("ip r show table main")]
#[case::ip_link("ip link")]
#[case::ip_link_list("ip l list")]
#[case::journalctl("journalctl -u nginx -n 50 --no-pager")]
#[case::journalctl_boot("journalctl -b -1 -p err")]
#[case::systemctl_status("systemctl status nginx --no-pager")]
#[case::systemctl_units("systemctl list-units --failed")]
#[case::systemctl_files("systemctl list-unit-files")]
#[case::systemctl_active("systemctl is-active sshd")]
#[case::systemctl_enabled("systemctl is-enabled sshd")]
#[case::systemctl_failed("systemctl is-failed sshd")]
#[case::systemctl_show("systemctl show -p MainPID nginx")]
#[case::systemctl_cat("systemctl cat nginx")]
#[case::systemctl_user("systemctl --user status efrd")]
#[case::git_status("git status --short")]
#[case::git_diff("git diff HEAD~1")]
#[case::git_log("git log --oneline -n 5")]
#[case::git_show("git show HEAD:src/main.rs")]
#[case::git_rev_parse("git rev-parse --show-toplevel")]
#[case::git_ls_files("git ls-files")]
#[case::git_blame("git blame -L 1,20 src/main.rs")]
#[case::git_no_pager("git --no-pager log -1")]
#[case::git_branch("git branch -vv")]
#[case::git_branch_current("git branch --show-current")]
#[case::git_remote("git remote")]
#[case::git_remote_v("git remote -v")]
#[case::pacman_query("pacman -Qi zsh")]
#[case::pacman_query_long("pacman --query --info zsh")]
#[case::pacman_owner("pacman -Qo /usr/bin/zsh")]
#[case::pacman_search("pacman -Ss ripgrep")]
#[case::pacman_info("pacman -Si ripgrep")]
#[case::lspci("lspci -k")]
#[case::lsusb("lsusb")]
#[case::sensors("sensors")]
#[case::nproc("nproc")]
#[case::find("find . -name '*.rs' -type f")]
fn every_read_only_default_runs_without_approval(#[case] line: &str) {
    assert_eq!(effect_of(line), Effect::Allow, "{line:?}");
}

#[rstest]
#[case::tail_follow("tail -f build.log")]
#[case::tail_follow_cluster("tail -fn 20 build.log")]
#[case::tail_follow_name("tail --follow=name build.log")]
#[case::file_compile("file -C -m magic")]
#[case::blkid_cache("blkid -g")]
#[case::findmnt_poll("findmnt --poll")]
#[case::free_repeat("free -s 1")]
#[case::hostname_set("hostname newname")]
#[case::hostname_file("hostname -F /etc/hostname")]
#[case::date_set("date -s 2026-01-01")]
#[case::command_runs("command rm -rf x")]
#[case::printf_assigns("printf -v PATH /tmp")]
#[case::tree_output("tree -o out.txt")]
#[case::tree_recursive_files("tree -R -H .")]
#[case::rg_preprocessor("rg --pre sh x")]
#[case::rg_preprocessor_value("rg --pre=sh x")]
#[case::rg_hostname_bin("rg --hostname-bin=x y")]
#[case::sort_output("sort -o out.txt in.txt")]
#[case::sort_output_cluster("sort -uo out.txt in.txt")]
#[case::sort_output_long("sort --output=out.txt in.txt")]
#[case::sort_output_abbreviated("sort --out=out.txt in.txt")]
#[case::sort_compress("sort --compress-program=sh in.txt")]
#[case::uniq_output("uniq in.txt out.txt")]
#[case::jq_in_place("jq -i '.a = 1' x.json")]
#[case::jq_env("jq -n env")]
#[case::jq_env_variable("jq -n '$ENV.TOKEN'")]
#[case::ps_environment("ps axe")]
#[case::ps_environment_separate("ps aux e")]
#[case::ss_kill("ss -K dst 1.2.3.4")]
#[case::ss_diag("ss -D dump.raw")]
#[case::ip_addr_add("ip addr add 10.0.0.1/24 dev eth0")]
#[case::ip_addr_flush("ip addr flush dev eth0")]
#[case::ip_route_del("ip route del default")]
#[case::ip_link_set("ip link set eth0 down")]
#[case::ip_batch("ip -batch cmds")]
#[case::journalctl_vacuum("journalctl --vacuum-size=100M")]
#[case::journalctl_vacuum_abbreviated("journalctl --vac=1d")]
#[case::journalctl_rotate("journalctl --rotate")]
#[case::journalctl_flush("journalctl --flush")]
#[case::journalctl_follow("journalctl -fu nginx")]
#[case::systemctl_restart("systemctl restart nginx")]
#[case::systemctl_stop("systemctl stop sshd")]
#[case::systemctl_enable("systemctl enable --now x")]
#[case::systemctl_edit("systemctl edit nginx")]
#[case::systemctl_remote("systemctl status -H server nginx")]
#[case::systemctl_user_restart("systemctl --user restart efrd")]
#[case::git_push("git push")]
#[case::git_commit("git commit -m x")]
#[case::git_checkout("git checkout main")]
#[case::git_config_before("git -c core.pager=sh log")]
#[case::git_dir_before("git -C /tmp status")]
#[case::git_diff_output("git diff --output=patch.txt")]
#[case::git_log_output_abbreviated("git log --out=x")]
#[case::git_branch_create("git branch feature")]
#[case::git_branch_delete("git branch -d feature")]
#[case::git_branch_delete_cluster("git branch -vD feature")]
#[case::git_branch_move("git branch --move a b")]
#[case::git_remote_add("git remote add origin x")]
#[case::git_remote_v_add("git remote -v add origin x")]
#[case::git_remote_show("git remote show origin")]
#[case::pacman_install("pacman -S zsh")]
#[case::pacman_upgrade("pacman -Syu")]
#[case::pacman_search_refresh("pacman -Ss -y zsh")]
#[case::pacman_refresh_search("pacman -Ssy zsh")]
#[case::pacman_remove("pacman -R zsh")]
#[case::pacman_file("pacman -U x.pkg.tar.zst")]
#[case::sensors_set("sensors -s")]
#[case::find_exec("find . -exec rm {} +")]
#[case::find_exec_quoted("find . -exec rm '{}' ';'")]
#[case::find_execdir("find . -execdir rm x ';'")]
#[case::find_delete("find . -name '*.o' -delete")]
#[case::find_ok("find . -ok rm x ';'")]
#[case::find_fprint("find . -fprint out.txt")]
#[case::find_fprintf("find . -fprintf out.txt %p")]
#[case::find_fls("find . -fls out.txt")]
#[case::env("env")]
#[case::printenv("printenv")]
#[case::other_program("cargo build")]
#[case::full_path("/bin/ls")]
fn writes_and_runs_need_approval(#[case] line: &str) {
    assert_eq!(effect_of(line), Effect::Ask, "{line:?}");
}

#[rstest]
// A compound line runs only when every simple command in it is allowed.
#[case::all_allowed("ls && git status | head -n 5; pwd", Effect::Allow)]
#[case::newline_separated("git status\ngit diff --stat", Effect::Allow)]
#[case::null_redirections("ls 2>/dev/null || echo none", Effect::Allow)]
#[case::quoted_operators("rg 'a; b && c' src", Effect::Allow)]
#[case::one_part_asks("ls && rm -rf build", Effect::Ask)]
#[case::last_part_asks("git status; git push", Effect::Ask)]
#[case::pipe_into_shell("cat script.sh | sh", Effect::Ask)]
#[case::pipe_into_xargs("find . -name '*.o' | xargs rm", Effect::Ask)]
// Constructs the rules cannot see through fall back to approval.
#[case::substitution("ls $(cat dirs.txt)", Effect::Ask)]
#[case::backticks("echo `id`", Effect::Ask)]
#[case::process_substitution("diff <(ls a) <(ls b)", Effect::Ask)]
#[case::variable("ls $HOME", Effect::Ask)]
#[case::here_document("cat <<EOF\nx\nEOF", Effect::Ask)]
#[case::output_file("ls > files.txt", Effect::Ask)]
#[case::append_file("echo x >> ~/.zshrc", Effect::Ask)]
#[case::eval("eval 'ls'", Effect::Ask)]
#[case::exec("exec ls", Effect::Ask)]
#[case::source("source ~/.zshrc", Effect::Ask)]
#[case::dot(". ./env.sh", Effect::Ask)]
#[case::alias("alias ls='rm -rf'", Effect::Ask)]
#[case::path_assignment("PATH=/tmp/evil ls", Effect::Ask)]
#[case::preload_assignment("LD_PRELOAD=/tmp/x.so cat a", Effect::Ask)]
#[case::bare_assignment("PATH=/tmp/evil", Effect::Ask)]
#[case::harmless_assignment("LC_ALL=C sort names.txt", Effect::Allow)]
#[case::leading_glob("cat *.rs", Effect::Ask)]
#[case::plain_glob("cat ./*.rs", Effect::Allow)]
#[case::history("ls; !!", Effect::Ask)]
#[case::background("ls &", Effect::Ask)]
#[case::subshell("(ls)", Effect::Ask)]
fn compound_lines_and_constructs(#[case] line: &str, #[case] expected: Effect) {
    assert_eq!(effect_of(line), expected, "{line:?}");
}

#[rstest]
#[case::sudo("sudo ls")]
#[case::sudo_path("/usr/bin/sudo ls")]
#[case::doas("doas ls")]
#[case::su("su -c ls")]
#[case::pkexec("pkexec ls")]
#[case::run0("run0 ls")]
#[case::behind_env("env sudo ls")]
#[case::in_a_compound("ls && sudo ls")]
#[case::in_a_substitution("echo $(sudo cat /etc/shadow)")]
fn privileged_programs_always_ask(#[case] line: &str) {
    assert_eq!(effect_of(line), Effect::Ask, "{line:?} by default");
    for rule in [
        Rule::new(Action::Execute, Resource::Any, Effect::Allow),
        Rule::new(Action::Any, Resource::Any, Effect::Allow),
        allow_command(CommandPattern::new("sudo")),
        allow_command(CommandPattern::new("ls")),
    ] {
        let engine = configured(vec![rule.clone()]);
        let decision = engine.decide(&input(run(line), Scope::Machine, Origin::Shell));
        assert_eq!(decision.effect(), Effect::Ask, "{line:?} under {rule:?}");
    }
}

#[test]
fn a_privileged_program_says_why_it_asks() {
    let engine = configured(vec![Rule::new(Action::Execute, Resource::Any, Effect::Allow)]);
    let decision = engine.decide(&input(run("sudo pacman -Syu"), Scope::Machine, Origin::Shell));
    assert_eq!(decision.reasons()[0].cause, Cause::Privileged { program: "sudo".to_owned() });
    assert_eq!(
        decision.reasons()[0].to_string(),
        "run \"sudo pacman -Syu\": ask, because sudo runs commands as another user"
    );
}

#[test]
fn a_privileged_program_that_a_rule_denies_stays_denied() {
    let engine = configured(vec![Rule::new(Action::Execute, Resource::Any, Effect::Deny)]);
    let decision = engine.decide(&input(run("sudo ls"), Scope::Machine, Origin::Shell));
    assert_eq!(decision.effect(), Effect::Deny);
}

#[test]
fn the_part_that_asks_is_named() {
    let decision =
        engine().decide(&input(run("ls && rm -rf build"), Scope::Machine, Origin::Shell));
    assert_eq!(
        decision.reasons()[0].to_string(),
        "run \"ls && rm -rf build\": ask, by rule 0 of the machine policy for \"rm -rf build\""
    );
}

#[test]
fn a_line_the_rules_cannot_see_through_says_what_it_holds() {
    let decision = engine().decide(&input(run("ls $(cat x)"), Scope::Machine, Origin::Shell));
    assert_eq!(
        decision.reasons()[0].cause,
        Cause::Opaque {
            construct: Construct::CommandSubstitution,
            layer: Layer::Machine,
            index: 0
        }
    );
    assert_eq!(
        decision.reasons()[0].to_string(),
        "run \"ls $(cat x)\": ask, by rule 0 of the machine policy, because the line holds a \
         command substitution, which no command rule can judge"
    );
}

#[test]
fn an_allowed_command_names_its_rule() {
    let decision = engine().decide(&input(run("git status"), Scope::Machine, Origin::Shell));
    let Cause::Rule { layer: Layer::Machine, index } = decision.reasons()[0].cause else {
        panic!("{:?}", decision.reasons()[0]);
    };
    let defaults = Policy::defaults();
    let rule = &defaults.rules()[index];
    assert!(
        matches!(&rule.resource, Resource::Command(pattern) if pattern.program == "git"),
        "{rule:?}"
    );
}

/// A read-only command with the paths that the shell tool declares for it.
fn reading(line: &str, reads: &[&str], trees: &[&str]) -> Requirements {
    let mut requirements = run(line);
    for path in reads {
        requirements = requirements.with_read(*path);
    }
    for path in trees {
        requirements = requirements.with_read_tree(*path);
    }
    requirements
}

#[rstest]
#[case::cat_key(reading("cat ~/.ssh/id_ed25519", &["/home/u/.ssh/id_ed25519"], &[]), Effect::Deny)]
#[case::list_ssh(reading("ls ~/.ssh", &["/home/u/.ssh"], &[]), Effect::Deny)]
#[case::head_credentials(
    reading("head ~/.aws/credentials", &["/home/u/.aws/credentials"], &[]),
    Effect::Deny
)]
#[case::grep_netrc(reading("grep pass ~/.netrc", &["/home/u/pass", "/home/u/.netrc"], &[]), Effect::Deny)]
#[case::input_redirect(reading("wc -c < ~/.ssh/id_rsa", &["/home/u/.ssh/id_rsa"], &[]), Effect::Deny)]
#[case::daemon_secrets(
    reading(
        "cat ~/.local/share/efr/secrets/openai-subscription.json",
        &["/home/u/.local/share/efr/secrets/openai-subscription.json"],
        &[]
    ),
    Effect::Deny
)]
#[case::shadow(reading("cat /etc/shadow", &["/etc/shadow"], &[]), Effect::Deny)]
#[case::process_environment(reading("cat /proc/self/environ", &["/proc/self/environ"], &[]), Effect::Deny)]
#[case::other_process_environment(reading("cat /proc/1234/environ", &["/proc/1234/environ"], &[]), Effect::Deny)]
#[case::through_proc_root(
    reading(
        "cat /proc/self/root/home/u/.ssh/id_ed25519",
        &["/proc/self/root/home/u/.ssh/id_ed25519"],
        &[]
    ),
    Effect::Deny
)]
#[case::through_proc_fd(reading("cat /proc/4321/fd/7", &["/proc/4321/fd/7"], &[]), Effect::Deny)]
#[case::thread_environment(
    reading("cat /proc/self/task/12/environ", &["/proc/self/task/12/environ"], &[]),
    Effect::Deny
)]
// A recursive read of a directory that holds a secret asks.
#[case::rg_aws(reading("rg TOKEN ~/.aws", &["/home/u/TOKEN"], &["/home/u/.aws"]), Effect::Ask)]
#[case::rg_config(reading("rg token ~/.config", &["/home/u/token"], &["/home/u/.config"]), Effect::Ask)]
#[case::grep_home(reading("grep -r x ~", &["/home/u/x"], &["/home/u"]), Effect::Ask)]
#[case::du_home(reading("du -sh ~", &[], &["/home/u"]), Effect::Ask)]
#[case::find_root(reading("find / -name x", &["/x"], &["/"]), Effect::Ask)]
#[case::grep_etc(reading("grep -r x /etc", &[], &["/etc"]), Effect::Ask)]
#[case::grep_proc(reading("grep -r x /proc/self", &[], &["/proc/self"]), Effect::Ask)]
#[case::glob_in_home(reading("cat ~/.ss*/id*", &[], &["/home/u"]), Effect::Ask)]
#[case::recursive_secret(reading("rg x ~/.ssh", &[], &["/home/u/.ssh"]), Effect::Deny)]
// Everything else a read-only command reads is free, as reading is.
#[case::cat_project(reading("cat src/main.rs", &["/home/u/p/app/src/main.rs"], &[]), Effect::Allow)]
#[case::list_home(reading("ls ~", &["/home/u"], &[]), Effect::Allow)]
#[case::list_config(reading("ls ~/.config", &["/home/u/.config"], &[]), Effect::Allow)]
#[case::cat_os_release(reading("cat /etc/os-release", &["/etc/os-release"], &[]), Effect::Allow)]
#[case::rg_project(reading("rg TODO", &["/home/u/p/app/TODO"], &["/home/u/p/app"]), Effect::Allow)]
#[case::rg_aws_config_file(reading("rg region ~/.aws/config", &[], &["/home/u/.aws/config"]), Effect::Allow)]
#[case::proc_status(reading("cat /proc/self/status", &["/proc/self/status"], &[]), Effect::Allow)]
#[case::proc_cpuinfo(reading("grep -r x /proc/cpuinfo", &[], &["/proc/cpuinfo"]), Effect::Allow)]
fn secrets_in_read_only_commands(#[case] requirements: Requirements, #[case] expected: Effect) {
    let decision = engine().decide(&input(requirements, Scope::Project(app()), Origin::Shell));
    assert_eq!(decision.effect(), expected, "{decision:?}");
}

#[test]
fn a_recursive_read_names_the_secret_below_it() {
    let decision = engine().decide(&input(
        reading("rg TOKEN ~/.aws", &[], &["/home/u/.aws"]),
        Scope::Machine,
        Origin::Shell,
    ));
    let deciding: Vec<_> = decision.deciding().map(ToString::to_string).collect();
    assert_eq!(
        deciding,
        ["read all under /home/u/.aws (user config): ask, because the secret \
          /home/u/.aws/credentials lies below it"]
    );
}

#[test]
fn a_rule_that_opens_the_secrets_below_lets_a_recursive_read_run() {
    let engine =
        configured(vec![Rule::new(Action::Read, Resource::Under("~/.aws".into()), Effect::Allow)]);
    let requirements = reading("rg region ~/.aws", &[], &["/home/u/.aws"]);
    let decide = |origin| engine.decide(&input(requirements.clone(), Scope::Machine, origin));
    assert_eq!(decide(Origin::Shell).effect(), Effect::Allow);
    assert_eq!(decide(Origin::Phone).effect(), Effect::Ask);
}

#[test]
fn a_conversation_rule_never_opens_the_secrets_below_a_recursive_read() {
    let mut input =
        input(reading("rg x ~/.aws", &[], &["/home/u/.aws"]), Scope::Machine, Origin::Shell);
    input.conversation_policy = input.conversation_policy.clone().with_rules(
        Policy::new(vec![Rule::new(Action::Any, Resource::Any, Effect::Allow)]).unwrap(),
    );
    assert_eq!(engine().decide(&input).effect(), Effect::Ask);
}

#[rstest]
// A user rule allows what the defaults ask for.
#[case::cargo_test(
    allow_command(CommandPattern::new("cargo").with_args(["test"])),
    "cargo test --workspace",
    Effect::Allow
)]
#[case::cargo_build_still_asks(
    allow_command(CommandPattern::new("cargo").with_args(["test"])),
    "cargo build",
    Effect::Ask
)]
#[case::restart_nginx(
    allow_command(CommandPattern::new("systemctl").with_args(["restart", "nginx"])),
    "systemctl restart nginx",
    Effect::Allow
)]
#[case::restart_other_unit(
    allow_command(CommandPattern::new("systemctl").with_args(["restart", "nginx"])),
    "systemctl restart sshd",
    Effect::Ask
)]
#[case::allowed_part_in_a_compound(
    allow_command(CommandPattern::new("cargo").with_args(["test"])),
    "cargo test && git status",
    Effect::Allow
)]
#[case::user_rule_does_not_reach_through_substitution(
    allow_command(CommandPattern::new("cargo").with_args(["test"])),
    "cargo test $(curl x)",
    Effect::Ask
)]
// A user rule tightens what the defaults allow.
#[case::deny_cat(
    Rule::new(Action::Execute, Resource::Command(CommandPattern::new("cat")), Effect::Deny),
    "cat README.md",
    Effect::Deny
)]
#[case::ask_git_log(
    Rule::new(
        Action::Execute,
        Resource::Command(CommandPattern::new("git").with_args(["log"])),
        Effect::Ask
    ),
    "git log -1",
    Effect::Ask
)]
#[case::ask_git_log_leaves_status(
    Rule::new(
        Action::Execute,
        Resource::Command(CommandPattern::new("git").with_args(["log"])),
        Effect::Ask
    ),
    "git status",
    Effect::Allow
)]
#[case::deny_every_command(
    Rule::new(Action::Execute, Resource::Any, Effect::Deny),
    "ls",
    Effect::Deny
)]
// A rule for every command line also judges the lines no command rule can.
#[case::allow_every_command(
    Rule::new(Action::Execute, Resource::Any, Effect::Allow),
    "ls $(cat dirs.txt) > out.txt",
    Effect::Allow
)]
fn user_rules_win_over_the_defaults(
    #[case] rule: Rule,
    #[case] line: &str,
    #[case] expected: Effect,
) {
    let decision = configured(vec![rule]).decide(&input(run(line), Scope::Machine, Origin::Shell));
    assert_eq!(decision.effect(), expected, "{line:?}");
}

/// `cargo test` allowed in `~/p/app` only, as `docs/permissions.md` shows.
fn cargo_test_in_app() -> Engine {
    configured(vec![allow_command(
        CommandPattern::new("cargo").with_args(["test"]).with_under("~/p/app"),
    )])
}

#[rstest]
#[case::in_the_project("cargo test", Some("/home/u/p/app"), Effect::Allow)]
#[case::below_the_project("cargo test -p x", Some("/home/u/p/app/crates/x"), Effect::Allow)]
#[case::other_project("cargo test", Some("/home/u/p/other"), Effect::Ask)]
#[case::unknown_directory("cargo test", None, Effect::Ask)]
#[case::relative_directory("cargo test", Some("p/app"), Effect::Ask)]
#[case::after_a_cd("cd ../other && cargo test", Some("/home/u/p/app"), Effect::Ask)]
#[case::a_later_cd_asks_for_itself("cargo test; cd ..", Some("/home/u/p/app"), Effect::Ask)]
#[case::with_a_read_only_part("cargo test && git status", Some("/home/u/p/app"), Effect::Allow)]
fn a_command_rule_may_name_the_directory_it_runs_in(
    #[case] line: &str,
    #[case] dir: Option<&str>,
    #[case] expected: Effect,
) {
    let mut requirements = run(line);
    requirements.command_dir = dir.map(Into::into);
    let decision = cargo_test_in_app().decide(&input(requirements, Scope::Machine, Origin::Shell));
    assert_eq!(decision.effect(), expected, "{line:?} in {dir:?}");
}

#[test]
fn a_directory_rule_sees_through_no_cd_even_when_cd_is_allowed() {
    let engine = configured(vec![
        allow_command(CommandPattern::new("cd")),
        allow_command(CommandPattern::new("cargo").with_args(["test"]).with_under("~/p/app")),
    ]);
    let requirements = run("cd /srv/elsewhere && cargo test").with_command_dir("/home/u/p/app");
    let decision = engine.decide(&input(requirements, Scope::Machine, Origin::Shell));
    assert_eq!(decision.effect(), Effect::Ask);
}

#[test]
fn a_user_rule_is_named_by_its_place_after_the_defaults() {
    let engine = configured(vec![allow_command(CommandPattern::new("cargo").with_args(["test"]))]);
    let decision = engine.decide(&input(run("cargo test"), Scope::Machine, Origin::Shell));
    let index = Policy::defaults().rules().len();
    assert_eq!(decision.reasons()[0].cause, Cause::Rule { layer: Layer::Machine, index });
}

#[test]
fn the_phone_asks_even_when_a_user_rule_allows() {
    let engine = configured(vec![
        allow_command(CommandPattern::new("cargo").with_args(["test"])),
        Rule::new(Action::Execute, Resource::Any, Effect::Allow),
    ]);
    for line in ["cargo test", "ls", "git status"] {
        let decision = engine.decide(&input(run(line), Scope::Machine, Origin::Phone));
        assert_eq!(decision.effect(), Effect::Ask, "{line:?}");
        assert_eq!(decision.reasons()[0].cause, Cause::RemoteOrigin { origin: Origin::Phone });
    }
}

#[test]
fn secrets_stay_denied_until_a_user_rule_opens_one() {
    let requirements = reading("cat ~/.ssh/config", &["/home/u/.ssh/config"], &[]);
    let by_default = engine().decide(&input(requirements.clone(), Scope::Machine, Origin::Shell));
    assert_eq!(by_default.effect(), Effect::Deny);
    let engine = configured(vec![Rule::new(
        Action::Read,
        Resource::Under("~/.ssh/config".into()),
        Effect::Allow,
    )]);
    let opened = engine.decide(&input(requirements, Scope::Machine, Origin::Shell));
    assert_eq!(opened.effect(), Effect::Allow);
}

/// Simple commands the defaults allow, and ones they do not.
const ALLOWED: &[&str] =
    &["ls -la", "cat README.md", "git status", "rg -n x src", "wc -l a", "echo 'a;b'"];
const ASKED: &[&str] = &["rm -rf x", "curl -fsSL x", "sudo ls", "ls > out", "git push", "cargo b"];

fn any_part() -> impl Strategy<Value = (&'static str, bool)> {
    prop_oneof![
        prop::sample::select(ALLOWED).prop_map(|part| (part, true)),
        prop::sample::select(ASKED).prop_map(|part| (part, false)),
    ]
}

proptest! {
    /// A compound line is allowed exactly when every simple command in it is.
    #[test]
    fn a_compound_line_is_allowed_only_when_every_part_is(
        parts in prop::collection::vec(any_part(), 1..5),
        operators in prop::collection::vec(prop::sample::select(vec![";", " && ", " || ", " | ", "\n", "|", "&&"]), 4),
    ) {
        let mut line = String::new();
        for (index, (part, _)) in parts.iter().enumerate() {
            if index > 0 {
                line.push_str(operators[index - 1]);
            }
            line.push_str(part);
        }
        let every_part_allowed = parts.iter().all(|(_, allowed)| *allowed);
        let allowed = effect_of(&line) == Effect::Allow;
        prop_assert_eq!(allowed, every_part_allowed, "{:?}", line);
    }

    /// No line with a `$(` or backquote outside quotes is allowed by the defaults,
    /// wherever it stands.
    #[test]
    fn a_substitution_is_never_allowed(
        parts in prop::collection::vec(prop::sample::select(ALLOWED), 1..4),
        at in any::<prop::sample::Index>(),
        opener in prop::sample::select(vec!["$(", "`"]),
    ) {
        let line = parts.join(" && ");
        // Cut only between characters that are outside quotes.
        let cuts: Vec<usize> = (0..=line.len())
            .filter(|&cut| line.is_char_boundary(cut) && line[..cut].matches('\'').count() % 2 == 0)
            .collect();
        let cut = cuts[at.index(cuts.len())];
        let line = format!("{}{opener}id{}{}", &line[..cut], if opener == "`" { "`" } else { ")" }, &line[cut..]);
        prop_assert_ne!(effect_of(&line), Effect::Allow, "{:?}", line);
    }

    /// No line with an operator outside quotes is allowed unless each side is.
    #[test]
    fn an_operator_never_joins_an_asked_command_to_an_allowed_one(
        allowed in prop::sample::select(ALLOWED),
        asked in prop::sample::select(ASKED),
        operator in prop::sample::select(vec![";", "&&", "||", "|", "\n", "|&"]),
        asked_first in any::<bool>(),
    ) {
        let line = if asked_first {
            format!("{asked}{operator}{allowed}")
        } else {
            format!("{allowed}{operator}{asked}")
        };
        prop_assert_ne!(effect_of(&line), Effect::Allow, "{:?}", line);
    }
}

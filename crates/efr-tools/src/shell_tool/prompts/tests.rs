//! The soft flag of each form of DS2, the lines that it leaves out, and the twin rows of
//! `is_remote`.

use efr_shell::CommandResult;
use rstest::rstest;
use serde_json::{Value, json};

use super::super::words;
use super::{is_remote, may_prompt};
use crate::testing::{FakeRunner, Fixture};
use crate::{ShellTool, Tool as _, ToolRequirements, job_request};

fn flagged(line: &str) -> bool {
    may_prompt(&words::split(line), line)
}

fn requirements(input: Value) -> ToolRequirements {
    let tool = ShellTool::new(FakeRunner::answering(Ok(CommandResult::finished(Some(0), "", "/"))));
    tool.requirements(&Fixture::new().context(), &input).unwrap()
}

#[rstest]
// rsync to or from another host, in every form of the operand.
#[case::rsync_host_path("rsync -a src/ host:backup/")]
#[case::rsync_user_host("rsync -a user@host:src/ dst/")]
#[case::rsync_module("rsync -a host::module/ dst/")]
#[case::rsync_url("rsync -a rsync://host/module/ dst/")]
// rsync with a remote shell, in every form of the option and the variable.
#[case::rsync_e("rsync -a -e ssh src/ dst/")]
#[case::rsync_e_attached("rsync -a -essh src/ dst/")]
#[case::rsync_e_in_group("rsync -avze 'ssh -p 2222' src/ dst/")]
#[case::rsync_rsh_equals("rsync -a --rsh=ssh src/ dst/")]
#[case::rsync_rsh_word("rsync -a --rsh 'ssh -p 2222' src/ dst/")]
#[case::rsync_rsh_variable("RSYNC_RSH=ssh rsync -a src/ dst/")]
#[case::rsync_after_env("env RSYNC_RSH=ssh rsync -a src/ dst/")]
#[case::rsync_after_nice("nice rsync -a src/ host:dst/")]
// The line of the incident.
#[case::evidence(
    "rsync -a --info=progress2 -e 'ssh -o PreferredAuthentications=password,keyboard-interactive \
     -o PubkeyAuthentication=no' eggfriedrice@t:.config/Slack \"$SCRATCH/slack\""
)]
// The programs that always may ask.
#[case::scp("scp build.tar user@host:/tmp")]
#[case::sftp("sftp host")]
#[case::sshfs("sshfs host:/srv mnt")]
#[case::ssh_copy_id("ssh-copy-id user@host")]
#[case::mosh("mosh host")]
#[case::autossh("autossh -M 0 host")]
#[case::ssh_add("ssh-add ~/.ssh/id_ed25519")]
#[case::ssh_keygen("ssh-keygen -t ed25519 -f key")]
// git over the network, past every option of git that takes a value.
#[case::git_push("git push origin main")]
#[case::git_c_dir_push("git -C dir push")]
#[case::git_dir_fetch("git --git-dir x fetch")]
#[case::git_work_tree_pull("git --work-tree w --git-dir w/.git pull")]
#[case::git_config_env("git --config-env core.x=X ls-remote origin")]
#[case::git_namespace("git --namespace n fetch")]
#[case::git_remote_update("git remote update")]
#[case::git_archive_remote("git archive --remote=host:repo HEAD")]
#[case::git_archive_remote_word("git archive --remote host:repo HEAD")]
#[case::git_send_email("git send-email 0001.patch")]
// git with a program of its own for ssh.
#[case::git_ssh_command_variable("GIT_SSH_COMMAND='ssh -i key' git status")]
#[case::git_ssh_variable("GIT_SSH=/usr/bin/ssh git status")]
#[case::git_core_ssh_command("git -c core.sshCommand='ssh -i key' status")]
#[case::git_core_ssh_command_case("git -c Core.SSHCommand=ssh status")]
// A line of several commands, and one in a substitution.
#[case::after_cd("cd ~/p/app && rsync -a ./ host:app/")]
#[case::in_pipe("tar c . | ssh-add -")]
fn lines_that_may_ask_get_the_soft_flag(#[case] line: &str) {
    assert!(flagged(line), "{line}");
}

#[rstest]
#[case::rsync_local("rsync -a a/ b/")]
#[case::rsync_path_with_colon("rsync ./a:b c")]
#[case::rsync_absolute_with_colon("rsync -a /a:b c/")]
#[case::rsync_e_not_a_shell("rsync -a -e cat a/ b/")]
#[case::git_status("git status")]
#[case::git_log_grep_push("git log --grep push")]
#[case::git_remote_v("git remote -v")]
#[case::git_archive_local("git archive HEAD")]
#[case::git_c_other("git -c user.name=x commit -m y")]
#[case::git_ssh_variable_not_git("GIT_SSH_COMMAND=ssh make")]
#[case::ssh_keygen_n("ssh-keygen -t ed25519 -N '' -f key")]
#[case::ssh_keygen_n_group("ssh-keygen -qN '' -f key")]
#[case::scpx("scpx host:x")]
#[case::echo("echo rsync host:x")]
#[case::quoted("echo 'git push'")]
#[case::plain("ls -la")]
fn other_lines_get_no_soft_flag(#[case] line: &str) {
    assert!(!flagged(line), "{line}");
}

#[test]
fn an_opaque_line_is_flagged_by_the_program_name() {
    let line = "x=$(date); rsync \"$x\" \"${DEST}\"";
    assert!(words::split(line).opaque);
    assert!(flagged(line));
    assert!(!flagged("x=$(date); cp \"$x\" \"${DEST}\""));
}

#[test]
fn the_shell_tool_declares_the_soft_flag_and_not_the_hard_one() {
    let declared = requirements(json!({"command": "rsync -e 'sudo ssh' src/ host:dst/"}));
    assert!(declared.may_prompt, "a remote shell gives the soft flag");
    assert!(!declared.interactive, "the hard reasons come from the line itself");
    let declared = requirements(json!({"command": "ls"}));
    assert!(!declared.may_prompt);
    let declared = requirements(json!({"command": "sudo rsync -a src/ host:dst/"}));
    assert!(declared.interactive && declared.may_prompt);
}

#[test]
fn a_job_of_git_clone_still_starts() {
    let input = json!({"command": "git clone git@github.com:x/y.git", "background": true});
    assert!(job_request(&input).unwrap().is_some());
    assert!(requirements(input).may_prompt);
}

/// The operands of `rsync` that are on another host, and those that are not.
// NOTE: the same rows as in `efr-permissions/src/exits/tests.rs`, whose `is_remote` this
// one copies; a change here changes both.
const REMOTE_ROWS: &[(&str, bool)] = &[
    ("host:path", true),
    ("user@host:path", true),
    ("u@h:", true),
    ("host::module", true),
    ("rsync://host/module", true),
    ("eggfriedrice@t:.config/Slack", true),
    ("./a:b", false),
    ("/abs/a:b", false),
    ("dir/a:b", false),
    (":path", false),
    ("plain", false),
    ("-e", false),
    ("--rsh=ssh:x", false),
    ("", false),
];

#[test]
fn remote_rows_match_the_classifier() {
    for (operand, remote) in REMOTE_ROWS {
        assert_eq!(is_remote(operand), *remote, "{operand:?}");
    }
}

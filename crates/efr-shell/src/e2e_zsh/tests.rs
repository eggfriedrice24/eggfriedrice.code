use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use bytes::Bytes;
use efr_holder::ChildStatus;
use efr_protocol::{CallId, InputWait, SecretText};
use efr_test_support::Wait;
use pretty_assertions::assert_eq;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use super::Zsh;
use crate::testing::{Heard, listener};
use crate::{
    CommandResult, Completion, Delimiter, NoProgress, OutputUpdate, Phase, RunMode, RunRequest,
    ShellError, ShellNotice,
};

impl Zsh {
    fn request(&self, command: &str) -> RunRequest {
        RunRequest::new(command, self.start_dir())
    }

    async fn run(&self, command: &str) -> CommandResult {
        self.sessions
            .run_command(self.conversation, self.request(command), &mut NoProgress)
            .await
            .unwrap()
    }

    /// Runs `request` until the screen shows `text`, then lets its timeout pass.
    async fn run_until_shown(&self, request: RunRequest, text: &str) -> CommandResult {
        let (seen, mut updates) = watch::channel(OutputUpdate::default());
        let sessions = self.sessions.clone();
        let conversation = self.conversation;
        let timeout = request.timeout;
        let run = tokio::spawn(async move {
            let mut progress = move |update: &OutputUpdate| {
                seen.send_replace(update.clone());
            };
            sessions.run_command(conversation, request, &mut progress).await
        });
        updates.wait_for(|update| update.tail.contains(text)).await.unwrap();
        self.screen_shows(text).await;
        self.clock.advance(timeout);
        run.await.unwrap().unwrap()
    }

    /// Yields until the screen shows `text`. The screen is fed right after the session,
    /// so this rarely loops.
    async fn screen_shows(&self, text: &str) {
        self.screen_has(|row| row.contains(text)).await;
    }

    /// Yields until a row of the screen starts with `text`.
    async fn screen_row_starts(&self, text: &str) {
        self.screen_has(|row| row.starts_with(text)).await;
    }

    async fn screen_has(&self, row_matches: impl Fn(&str) -> bool) {
        let screen = self.sessions.screen(self.conversation).unwrap();
        Wait::new("a row on the screen")
            .until_some_async(async || {
                let capture = screen.snapshot(0).await.unwrap();
                let rows = &capture.snapshot.rows;
                rows.iter().any(|row| row_matches(&efr_screen::row_text(row))).then_some(())
            })
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn e2e_a_command_with_output_and_exit_code() {
    let Some(zsh) = Zsh::start("e2e_a_command_with_output_and_exit_code") else {
        return;
    };
    let result = zsh.run("printf 'hello\\nworld\\n'").await;
    assert_eq!(result.completion, Completion::Finished);
    assert_eq!(result.exit_code, Some(0));
    assert_eq!(result.output, "hello\nworld\n");
    assert_eq!(result.delimiter, Delimiter::Marks);
    let state = zsh.sessions.state(zsh.conversation).await.unwrap();
    assert!(state.integration);
    assert_eq!(state.last_exit, Some(0));
}

#[tokio::test]
async fn e2e_a_redrawn_progress_display_leaves_its_last_frame() {
    let Some(zsh) = Zsh::start("e2e_a_redrawn_progress_display_leaves_its_last_frame") else {
        return;
    };
    let result = zsh
        .run(
            "printf 'one 1\\ntwo 1\\n'; for i in 2 3; do printf '\\e[2Aone %s\\ntwo %s\\n' $i $i; done",
        )
        .await;
    assert_eq!(result.completion, Completion::Finished);
    assert_eq!(result.exit_code, Some(0));
    assert_eq!(result.output, "one 3\ntwo 3");
}

#[tokio::test]
async fn e2e_a_failing_command() {
    let Some(zsh) = Zsh::start("e2e_a_failing_command") else {
        return;
    };
    let result = zsh.run("sh -c 'echo oops >&2; exit 3'").await;
    assert_eq!(result.completion, Completion::Finished);
    assert_eq!(result.exit_code, Some(3));
    assert_eq!(result.output, "oops\n");
}

#[tokio::test]
async fn e2e_cd_changes_cwd_after() {
    let Some(zsh) = Zsh::start("e2e_cd_changes_cwd_after") else {
        return;
    };
    let target = zsh.dir("work/sub");
    let result = zsh.run(&format!("cd '{}'", target.display())).await;
    assert_eq!(result.exit_code, Some(0));
    assert_eq!(result.cwd_after, target);
    assert_eq!(zsh.sessions.state(zsh.conversation).await.unwrap().cwd, target);
    let pwd = zsh.run("pwd").await;
    assert_eq!(pwd.output, format!("{}\n", target.display()));
    zsh.notices
        .wait_for(|notice| matches!(notice, ShellNotice::CwdChanged { cwd, .. } if *cwd == target))
        .await;
}

#[tokio::test]
async fn e2e_an_interactive_read_hits_the_timeout() {
    let Some(zsh) = Zsh::start("e2e_an_interactive_read_hits_the_timeout") else {
        return;
    };
    let result = zsh.run_until_shown(zsh.request("read -r 'answer?name? '"), "name? ").await;
    assert_eq!(result.completion, Completion::Interactive);
    assert_eq!(result.exit_code, None);
    assert!(
        result.screen_tail.as_deref().is_some_and(|tail| tail.ends_with("name?")),
        "{result:?}"
    );

    // The user answers on the attached screen; the command ends and the shell is free.
    zsh.sessions.write(zsh.conversation, Bytes::from_static(b"bob\r")).await.unwrap();
    let answer = zsh.run("print -r -- $answer").await;
    assert_eq!(answer.output, "bob\n");
}

#[tokio::test]
async fn e2e_a_multiline_command_is_one_run() {
    let Some(zsh) = Zsh::start("e2e_a_multiline_command_is_one_run") else {
        return;
    };
    let result = zsh.run("for i in 1 2 3; do\n\techo \"n=$i\"\ndone").await;
    assert_eq!(result.output, "n=1\nn=2\nn=3\n");
    assert_eq!(result.exit_code, Some(0));
}

#[tokio::test]
async fn e2e_an_unfinished_line_is_cancelled() {
    let Some(zsh) = Zsh::start("e2e_an_unfinished_line_is_cancelled") else {
        return;
    };
    let result = zsh.run("echo \"unclosed").await;
    assert_eq!(result.completion, Completion::NotStarted);
    let next = zsh.run("echo fine").await;
    assert_eq!(next.output, "fine\n");
}

#[tokio::test]
async fn e2e_history_expansion_and_the_history_file_are_off() {
    let Some(zsh) = Zsh::start("e2e_history_expansion_and_the_history_file_are_off") else {
        return;
    };
    let result = zsh.run("echo hi!; print -r -- ${HISTFILE-unset}").await;
    assert_eq!(result.output, "hi!\nunset\n");
}

#[tokio::test]
async fn e2e_the_users_startup_files_run_and_zdotdir_is_restored() {
    let Some(zsh) = Zsh::start("e2e_the_users_startup_files_run_and_zdotdir_is_restored") else {
        return;
    };
    std::fs::write(zsh.home().join(".zshrc"), "alias greet='echo hello from zshrc'\n").unwrap();
    let result = zsh.run("greet; print -r -- ${ZDOTDIR-unset} $EFR_HIDDEN_SHELL").await;
    assert_eq!(result.output, "hello from zshrc\nunset 1\n");
}

#[tokio::test]
async fn e2e_a_pager_from_the_users_zshrc_is_turned_off() {
    let Some(zsh) = Zsh::start("e2e_a_pager_from_the_users_zshrc_is_turned_off") else {
        return;
    };
    std::fs::write(
        zsh.home().join(".zshrc"),
        "export PAGER=less GIT_PAGER=less AWS_PAGER=less GH_PAGER=less BAT_PAGER=less\n",
    )
    .unwrap();
    let result = zsh
        .run("print -r -- $PAGER $GIT_PAGER $SYSTEMD_PAGER $MANPAGER \"[$AWS_PAGER]\" $GH_PAGER $BAT_PAGER")
        .await;
    assert_eq!(result.output, "cat cat cat cat [] cat cat\n");
}

#[tokio::test]
async fn e2e_an_editor_from_the_users_zshrc_is_replaced_by_the_stub() {
    let Some(zsh) = Zsh::start("e2e_an_editor_from_the_users_zshrc_is_replaced_by_the_stub") else {
        return;
    };
    std::fs::write(
        zsh.home().join(".zshrc"),
        "export EDITOR=vim VISUAL=vim GIT_EDITOR=vim GIT_SEQUENCE_EDITOR=vim SUDO_EDITOR=vim \
         SYSTEMD_EDITOR=vim\n",
    )
    .unwrap();
    let result = zsh
        .run(
            "print -r -- $EDITOR $VISUAL $GIT_EDITOR $GIT_SEQUENCE_EDITOR $SUDO_EDITOR \
             $SYSTEMD_EDITOR",
        )
        .await;
    let stub = std::fs::canonicalize(zsh.start_dir().join("zsh")).unwrap().join("efr-editor");
    let stub = stub.display();
    assert_eq!(result.output, format!("{stub} {stub} {stub} {stub} {stub} {stub}\n"));
}

/// Without the stub, git would open vi on the hidden screen and wait for the run's
/// timeout (or fail to find it); with it, the commit fails at once with a message
/// that says how to go on.
#[tokio::test]
async fn e2e_git_commit_without_a_message_fails_at_once_and_says_why() {
    let Some(zsh) = Zsh::start("e2e_git_commit_without_a_message_fails_at_once_and_says_why")
    else {
        return;
    };
    let repo = zsh.dir("repo");
    let result = zsh
        .run(&format!(
            "cd '{}' && git init -q && git -c user.name=efr -c user.email=efr@example.invalid \
             commit --allow-empty",
            repo.display()
        ))
        .await;
    assert_eq!(result.completion, Completion::Finished);
    assert_eq!(result.exit_code, Some(1), "{}", result.output);
    assert!(
        result.output.contains(
            "efr: there is no editor in the hidden shell; pass the text another way, such as \
             git commit -m, a file, or ask the user to edit it\n"
        ),
        "{}",
        result.output
    );
}

#[tokio::test]
async fn e2e_a_pattern_that_matches_nothing_never_vanishes() {
    let Some(zsh) = Zsh::start("e2e_a_pattern_that_matches_nothing_never_vanishes") else {
        return;
    };
    std::fs::write(zsh.home().join(".zshrc"), "setopt null_glob csh_null_glob\n").unwrap();
    let result = zsh.run("[[ -o null_glob || -o csh_null_glob ]] && echo on || echo off").await;
    assert_eq!(result.output, "off\n");
}

#[tokio::test]
async fn e2e_global_and_suffix_aliases_from_the_users_zshrc_are_dropped() {
    let Some(zsh) = Zsh::start("e2e_global_and_suffix_aliases_from_the_users_zshrc_are_dropped")
    else {
        return;
    };
    let zshrc = "alias -g L='| tr a-z A-Z'\nalias -s txt=cat\nalias greet='echo hi'\n";
    std::fs::write(zsh.home().join(".zshrc"), zshrc).unwrap();
    let result = zsh.run("echo hello L; print -r -- ${#galiases} ${#saliases}; greet").await;
    assert_eq!(result.output, "hello L\n0 0\nhi\n");
}

#[tokio::test]
async fn e2e_an_alias_or_function_named_like_a_trusted_program_is_dropped() {
    let test = "e2e_an_alias_or_function_named_like_a_trusted_program_is_dropped";
    let Some(zsh) = Zsh::start_with(test, |config| {
        config.trusted_programs = vec!["ls".to_owned(), "cat".to_owned(), "nproc".to_owned()];
    }) else {
        return;
    };
    let zshrc = "alias ls='echo aliased'\ncat() { echo function; }\nalias greet='echo hi'\n";
    std::fs::write(zsh.home().join(".zshrc"), zshrc).unwrap();
    let result = zsh
        .run("ls -d /; cat /dev/null; greet; print -r -- ${_EFR_HS_TRUSTED_PROGRAMS-unset}")
        .await;
    assert_eq!(result.output, "/\nhi\nunset\n");
}

#[tokio::test]
async fn e2e_an_alias_named_like_a_newly_trusted_program_does_not_run() {
    let test = "e2e_an_alias_named_like_a_newly_trusted_program_does_not_run";
    let Some(zsh) = Zsh::start(test) else {
        return;
    };
    std::fs::write(zsh.home().join(".zshrc"), "alias ls='echo aliased'\n").unwrap();
    let work = zsh.dir("work");
    let before = zsh.run(&format!("cd {}; ls -d /", work.display())).await;
    assert_eq!(before.output, "aliased -d /\n");
    let first = zsh.sessions.state(zsh.conversation).await.unwrap().pty_id;

    // A rules change trusts ls from now on, as a config reload does.
    zsh.sessions.set_trusted_programs(vec!["ls".to_owned()]);
    let after = zsh.run("ls -d /; pwd").await;

    assert_eq!(after.output, format!("/\n{}\n", work.display()));
    let state = zsh.sessions.state(zsh.conversation).await.unwrap();
    assert_ne!(state.pty_id, first, "the shell restarted");
    assert_eq!(state.cwd, work, "in the directory the old shell was in");
}

#[tokio::test]
async fn e2e_the_integration_loads_when_the_users_zshenv_sets_no_unset() {
    let Some(zsh) = Zsh::start("e2e_the_integration_loads_when_the_users_zshenv_sets_no_unset")
    else {
        return;
    };
    std::fs::write(zsh.home().join(".zshenv"), "setopt no_unset\n").unwrap();
    let result = zsh.run("echo marked").await;
    assert_eq!(result.delimiter, Delimiter::Marks);
    assert_eq!(result.output, "marked\n");
}

/// compinit asks before it loads completions from a directory that other users can
/// write to, and waits for a key that nobody types. Ubuntu's /etc/zsh/zshrc calls it in
/// every interactive shell, and GitHub's Ubuntu runner makes /usr/share writable by
/// everyone. The .zshrc here calls it twice, as Ubuntu's file and then the user's own
/// call would.
#[tokio::test]
async fn e2e_compinit_never_waits_for_an_answer_about_an_insecure_directory() {
    let test = "e2e_compinit_never_waits_for_an_answer_about_an_insecure_directory";
    let Some(zsh) = Zsh::start(test) else {
        return;
    };
    let open = zsh.dir("open");
    std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o777)).unwrap();
    let zshrc =
        format!("fpath=('{}' $fpath)\nautoload -Uz compinit\ncompinit\ncompinit\n", open.display());
    std::fs::write(zsh.home().join(".zshrc"), zshrc).unwrap();
    let info = zsh.sessions.open(zsh.conversation, zsh.start_dir()).await.unwrap();

    // Without a run, nothing waits for the first prompt, so a question shows up here
    // instead of as a run that never ends.
    let mut shown = String::new();
    let came = Wait::new("the first prompt or a question")
        .until(|| {
            shown = String::from_utf8_lossy(&zsh.recorded.stream(info.pty_id)).into_owned();
            shown.contains("\x1b]133;A") || shown.contains("insecure")
        })
        .await;
    assert!(came.is_ok(), "no prompt came: {shown:?}");
    assert!(!shown.contains("insecure"), "compinit asked: {shown:?}");

    // The completion system is loaded, without the directory, as after the answer y.
    let check = format!("print -r -- ${{+functions[compdef]}} ${{fpath[(Ie){}]}}", open.display());
    let result = zsh.run(&check).await;
    assert_eq!(result.delimiter, Delimiter::Marks);
    assert_eq!(result.output, "1 0\n");
}

/// The zsh plugin of the user's terminals, which a real .zshrc sources.
fn plugin() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../shell/zsh/efr.plugin.zsh")
}

/// Runs a command with `zshrc` as the user's .zshrc and checks that the marks still
/// delimit it.
async fn marks_survive(test: &str, zshrc: impl FnOnce(&Zsh) -> String) {
    let Some(zsh) = Zsh::start(test) else {
        return;
    };
    std::fs::write(zsh.home().join(".zshrc"), zshrc(&zsh)).unwrap();
    let result = zsh.run("echo marked").await;
    assert_eq!(result.delimiter, Delimiter::Marks);
    assert_eq!(result.output, "marked\n");
    let next = zsh.run("echo again").await;
    assert_eq!(next.delimiter, Delimiter::Marks);
    assert_eq!(next.exit_code, Some(0));
    let state = zsh.sessions.state(zsh.conversation).await.unwrap();
    assert!(state.integration);
    assert_eq!(state.last_exit, Some(0));
}

#[tokio::test]
async fn e2e_the_efr_plugin_in_the_users_zshrc_keeps_the_marks() {
    marks_survive("e2e_the_efr_plugin_in_the_users_zshrc_keeps_the_marks", |_| {
        format!("source '{}'\n", plugin().display())
    })
    .await;
}

/// Without its guard the plugin defines its own hooks in the hidden shell; the
/// integration's names are its own, so its hooks survive anyway.
#[tokio::test]
async fn e2e_the_efr_plugin_loaded_anyway_cannot_replace_the_hooks() {
    marks_survive("e2e_the_efr_plugin_loaded_anyway_cannot_replace_the_hooks", |zsh| {
        let runtime = zsh.dir("runtime");
        format!(
            "unset EFR_HIDDEN_SHELL\nexport XDG_RUNTIME_DIR='{}'\nsource '{}'\n",
            runtime.display(),
            plugin().display()
        )
    })
    .await;
}

#[tokio::test]
async fn e2e_unsent_text_at_the_prompt_does_not_join_the_command() {
    let Some(zsh) = Zsh::start("e2e_unsent_text_at_the_prompt_does_not_join_the_command") else {
        return;
    };
    zsh.run("true").await;
    // Someone typed at the attached screen and did not press Enter.
    zsh.sessions.write(zsh.conversation, Bytes::from_static(b"echo leftover ")).await.unwrap();
    let result = zsh.run("printf ok").await;
    assert_eq!(result.output, "ok");
    assert_eq!(result.exit_code, Some(0));
}

#[tokio::test]
async fn e2e_a_nested_shell_is_driven_by_sentinels() {
    let Some(zsh) = Zsh::start("e2e_a_nested_shell_is_driven_by_sentinels") else {
        return;
    };
    let bash = zsh.run_until_shown(zsh.request("bash --norc --noprofile"), "bash-").await;
    assert!(bash.interactive(), "{bash:?}");

    let nested = zsh
        .sessions
        .run_command(
            zsh.conversation,
            zsh.request("echo nested; (exit 4)").with_mode(RunMode::Sentinel),
            &mut NoProgress,
        )
        .await
        .unwrap();
    assert_eq!(nested.delimiter, Delimiter::Sentinel);
    assert_eq!(nested.output, "nested\n");
    assert_eq!(nested.exit_code, Some(4));
    assert_eq!(zsh.sessions.state(zsh.conversation).await.unwrap().phase, Phase::Running);

    zsh.sessions.write(zsh.conversation, Bytes::from_static(b"exit\r")).await.unwrap();
    let back = zsh.run("echo back").await;
    assert_eq!(back.delimiter, Delimiter::Marks);
    assert_eq!(back.output, "back\n");
}

#[tokio::test]
async fn e2e_exit_ends_the_shell() {
    let Some(zsh) = Zsh::start("e2e_exit_ends_the_shell") else {
        return;
    };
    let info = zsh.sessions.open(zsh.conversation, Path::new(zsh.start_dir())).await.unwrap();
    zsh.run("true").await;
    zsh.sessions.write(zsh.conversation, Bytes::from_static(b"exit 7\r")).await.unwrap();
    zsh.notices
        .wait_for(|notice| {
            matches!(notice, ShellNotice::Exited { pty_id, status: Some(ChildStatus::Exited { code: 7 }), .. }
                if *pty_id == info.pty_id)
        })
        .await;
    let again = zsh.run("echo again").await;
    assert_eq!(again.output, "again\n");
    let state = zsh.sessions.state(zsh.conversation).await.unwrap();
    assert_ne!(state.pty_id, info.pty_id);
    assert_eq!(state.cwd, PathBuf::from(zsh.start_dir()));
}

/// A program that reads a password as getpass does: echo off, one line, echo on. It
/// prints how long the line was, never the line.
const GETPASS: &str =
    r#"sh -c 'stty -echo; printf "pw: "; IFS= read -r p; stty echo; printf "\nlen=%s\n" "${#p}"'"#;

const CALL: &str = "01920000-0000-7000-8000-0000000ce2e0";

fn call() -> CallId {
    CALL.parse().unwrap()
}

impl Zsh {
    /// Starts `command` for [`CALL`] with a listener that says `can_answer`, and waits
    /// until its output shows `text`.
    async fn run_waiting(
        &self,
        command: &str,
        can_answer: bool,
        text: &str,
    ) -> (JoinHandle<Result<CommandResult, ShellError>>, Heard) {
        let (run, mut heard) = self.start_for_call(self.request(command), can_answer);
        heard.output(text).await;
        (run, heard)
    }

    /// Starts `request` for [`CALL`] with a listener that says `can_answer`.
    fn start_for_call(
        &self,
        request: RunRequest,
        can_answer: bool,
    ) -> (JoinHandle<Result<CommandResult, ShellError>>, Heard) {
        let (mut listener, heard) = listener(can_answer);
        let sessions = self.sessions.clone();
        let conversation = self.conversation;
        let request = request.with_call(call()).with_timeout(Duration::from_secs(600));
        let run = tokio::spawn(async move {
            sessions.run_command(conversation, request, &mut listener).await
        });
        (run, heard)
    }

    /// Moves the clock one quiet period, then waits until the run sleeps until its next
    /// look, which it does only after it told the listener what it saw.
    async fn one_look(&self) {
        self.clock.advance(Duration::from_secs(1));
        let after = self.clock.pending_sleeps();
        self.clock.wait_for_sleeps(after + 1).await;
    }

    /// Lets looks pass until the listener has heard `expected`; returns how many.
    async fn look_until(&self, heard: &mut Heard, expected: &[InputWait]) -> usize {
        for looks in 0..30 {
            if heard.inputs.borrow().as_slice() == expected {
                return looks;
            }
            self.one_look().await;
        }
        panic!("the run never reported {expected:?}: {:?}", heard.inputs.borrow());
    }

    /// Every byte the shell printed so far, as text.
    async fn recording(&self) -> String {
        let pty_id = self.sessions.state(self.conversation).await.unwrap().pty_id;
        String::from_utf8_lossy(&self.recorded.stream(pty_id)).into_owned()
    }
}

#[tokio::test]
async fn e2e_input_left_over_when_a_command_ends_never_runs() {
    let Some(zsh) = Zsh::start("e2e_input_left_over_when_a_command_ends_never_runs") else {
        return;
    };
    let dir = zsh.dir("work");
    zsh.sessions.open(zsh.conversation, zsh.start_dir()).await.unwrap();
    let sessions = zsh.sessions.clone();
    let conversation = zsh.conversation;
    let request = zsh.request(&format!("cd '{}' && sleep 1", dir.display()));
    let run =
        tokio::spawn(
            async move { sessions.run_command(conversation, request, &mut NoProgress).await },
        );
    Wait::new("the running command")
        .until_some_async(async || {
            let state = zsh.sessions.state(zsh.conversation).await.unwrap();
            (state.phase == Phase::Running).then_some(())
        })
        .await
        .unwrap();
    // What an answer written just after a password prompt gave up would leave behind.
    zsh.sessions.write(zsh.conversation, Bytes::from_static(b"touch leak\r")).await.unwrap();
    let slept = run.await.unwrap().unwrap();
    assert_eq!(slept.exit_code, Some(0));

    let listed = zsh.run("ls -A").await;
    assert_eq!(listed.output, "", "the leftover line never ran");
    assert!(!dir.join("leak").exists());
}

#[tokio::test]
async fn e2e_a_getpass_read_waits_for_hidden_input_and_its_answer_never_reaches_the_output() {
    let Some(zsh) = Zsh::start(
        "e2e_a_getpass_read_waits_for_hidden_input_and_its_answer_never_reaches_the_output",
    ) else {
        return;
    };
    let (run, mut heard) = zsh.run_waiting(GETPASS, true, "pw: ").await;
    zsh.look_until(&mut heard, &[InputWait::Hidden]).await;

    let secret = SecretText::new("hunter2");
    zsh.sessions.answer(zsh.conversation, call(), &secret, true).await.unwrap();
    let result = run.await.unwrap().unwrap();
    assert_eq!(result.completion, Completion::Finished);
    assert_eq!(result.exit_code, Some(0));
    assert!(result.output.contains("len=7"), "{:?}", result.output);
    assert!(!result.output.contains("hunter2"), "{:?}", result.output);
    assert_eq!(*heard.inputs.borrow(), [InputWait::Hidden, InputWait::None]);
    let after = zsh.run("echo after").await;
    assert_eq!(after.output, "after\n");
    assert!(!zsh.recording().await.contains("hunter2"), "the answer never reached the screen");
}

#[tokio::test]
async fn e2e_a_hidden_answer_while_echo_is_on_is_refused_and_writes_nothing() {
    let Some(zsh) =
        Zsh::start("e2e_a_hidden_answer_while_echo_is_on_is_refused_and_writes_nothing")
    else {
        return;
    };
    let command = r#"sh -c 'printf "name? "; IFS= read -r line; printf "got=%s\n" "$line"'"#;
    let (run, mut heard) = zsh.run_waiting(command, true, "name? ").await;
    zsh.look_until(&mut heard, &[InputWait::Visible]).await;
    let secret = SecretText::new("secret");
    let refused = zsh.sessions.answer(zsh.conversation, call(), &secret, true).await;
    assert!(matches!(refused, Err(ShellError::NotWaiting { .. })), "{refused:?}");

    zsh.sessions.answer(zsh.conversation, call(), &SecretText::new("ok"), false).await.unwrap();
    let result = run.await.unwrap().unwrap();
    assert!(result.output.ends_with("got=ok\n"), "{:?}", result.output);
    assert!(!result.output.contains("secret"), "{:?}", result.output);
}

#[tokio::test]
async fn e2e_a_visible_prompt_waits_for_visible_input_and_its_answer_is_output() {
    let Some(zsh) =
        Zsh::start("e2e_a_visible_prompt_waits_for_visible_input_and_its_answer_is_output")
    else {
        return;
    };
    let command = r#"sh -c 'printf "name> "; IFS= read -r n; printf "hi %s\n" "$n"'"#;
    let (run, mut heard) = zsh.run_waiting(command, true, "name> ").await;
    let looks = zsh.look_until(&mut heard, &[InputWait::Visible]).await;
    assert!(looks >= 3, "visible input waits for three quiet seconds, not {looks}");

    zsh.sessions.answer(zsh.conversation, call(), &SecretText::new("bob"), false).await.unwrap();
    let result = run.await.unwrap().unwrap();
    assert_eq!(result.output, "name> bob\nhi bob\n");
    assert_eq!(*heard.inputs.borrow(), [InputWait::Visible, InputWait::None]);
}

#[tokio::test]
async fn e2e_a_question_waits_for_visible_input_at_the_first_look() {
    let Some(zsh) = Zsh::start("e2e_a_question_waits_for_visible_input_at_the_first_look") else {
        return;
    };
    let command = r#"sh -c 'printf "name? "; IFS= read -r n; printf "hi %s\n" "$n"'"#;
    let (run, mut heard) = zsh.run_waiting(command, true, "name? ").await;
    let looks = zsh.look_until(&mut heard, &[InputWait::Visible]).await;
    assert_eq!(looks, 1, "a question needs half a second of quiet, so the first look");

    zsh.sessions.answer(zsh.conversation, call(), &SecretText::new("bob"), false).await.unwrap();
    let result = run.await.unwrap().unwrap();
    assert_eq!(result.output, "name? bob\nhi bob\n");
    assert_eq!(*heard.inputs.borrow(), [InputWait::Visible, InputWait::None]);
}

/// A question asked behind a relay. util-linux `script` puts the hidden shell's terminal
/// in raw mode and runs the program on a terminal of its own, as `sudo` does with
/// `use_pty` (its default), so the modes that the look reads are the relay's, not the
/// program's.
const RELAYED_QUESTION: &str = r#"script -q -c "sh -c 'printf \"ok? [Y/n] \"; IFS= read -r a; printf \"got=%s\n\" \"\$a\"'" /dev/null"#;

#[tokio::test]
async fn e2e_a_question_behind_a_relay_in_raw_mode_waits_for_visible_input_and_takes_an_answer() {
    let Some(zsh) = Zsh::start(
        "e2e_a_question_behind_a_relay_in_raw_mode_waits_for_visible_input_and_takes_an_answer",
    ) else {
        return;
    };
    let (run, mut heard) = zsh.run_waiting(RELAYED_QUESTION, true, "ok? [Y/n] ").await;
    zsh.look_until(&mut heard, &[InputWait::Visible]).await;

    zsh.sessions.answer(zsh.conversation, call(), &SecretText::new("y"), false).await.unwrap();
    let result = run.await.unwrap().unwrap();
    assert_eq!(result.completion, Completion::Finished);
    assert_eq!(result.exit_code, Some(0));
    assert!(result.output.contains("got=y"), "{:?}", result.output);
    assert_eq!(*heard.inputs.borrow(), [InputWait::Visible, InputWait::None]);
}

impl Zsh {
    /// Lets `looks` looks pass and checks that the run reported no wait, so that an
    /// answer for it is refused; then ends the shell at the prompt with `exit`.
    async fn no_wait_until_exit(
        &self,
        run: JoinHandle<Result<CommandResult, ShellError>>,
        heard: &Heard,
        looks: usize,
    ) -> CommandResult {
        for _ in 0..looks {
            self.one_look().await;
        }
        assert!(heard.inputs.borrow().is_empty(), "{:?}", heard.inputs.borrow());
        let answer = SecretText::new("echo answered");
        let refused = self.sessions.answer(self.conversation, call(), &answer, false).await;
        assert!(matches!(refused, Err(ShellError::NotWaiting { .. })), "{refused:?}");
        self.sessions.write(self.conversation, Bytes::from_static(b"exit\r")).await.unwrap();
        let result = run.await.unwrap().unwrap();
        assert!(heard.inputs.borrow().is_empty(), "{:?}", heard.inputs.borrow());
        assert!(!result.output.contains("answered"), "{:?}", result.output);
        result
    }
}

#[tokio::test]
async fn e2e_a_shell_behind_a_relay_reports_no_visible_wait_at_its_prompt() {
    let Some(zsh) = Zsh::start("e2e_a_shell_behind_a_relay_reports_no_visible_wait_at_its_prompt")
    else {
        return;
    };
    // The relayed shell's prompt sits after text on a quiet raw terminal, as a question
    // behind a relay does.
    let (run, heard) = zsh.run_waiting("script -q -c sh /dev/null", true, "$ ").await;
    let result = zsh.no_wait_until_exit(run, &heard, 6).await;
    assert_eq!(result.completion, Completion::Finished);
    assert_eq!(result.exit_code, Some(0));
}

#[tokio::test]
async fn e2e_a_sentinel_run_reports_a_hidden_wait_but_not_a_nested_shell_prompt() {
    let Some(zsh) =
        Zsh::start("e2e_a_sentinel_run_reports_a_hidden_wait_but_not_a_nested_shell_prompt")
    else {
        return;
    };
    let bash = zsh.run_until_shown(zsh.request("bash --norc --noprofile"), "bash-").await;
    assert!(bash.interactive(), "{bash:?}");

    // A password prompt in the nested shell is still answered.
    let (run, mut heard) =
        zsh.start_for_call(zsh.request(GETPASS).with_mode(RunMode::Sentinel), true);
    // A sentinel run holds back the end of its output until it cannot be the start of
    // its end marker, so a short prompt shows on the screen first.
    zsh.screen_row_starts("pw:").await;
    zsh.look_until(&mut heard, &[InputWait::Hidden]).await;
    zsh.sessions.answer(zsh.conversation, call(), &SecretText::new("hunter2"), true).await.unwrap();
    let read = run.await.unwrap().unwrap();
    assert_eq!(read.delimiter, Delimiter::Sentinel);
    assert!(read.output.contains("len=7"), "{:?}", read.output);
    assert!(!read.output.contains("hunter2"), "{:?}", read.output);
    assert_eq!(*heard.inputs.borrow(), [InputWait::Hidden, InputWait::None]);

    // A shell named through a variable, which the reading of the command line does not
    // follow: only the sentinel run's own rule keeps its prompt from being offered. Its
    // prompt comes from the environment, because `sh` is bash on some systems and dash,
    // whose prompt is a bare `$ `, on Ubuntu.
    let request = zsh.request("shell=sh; PS1='sh-nested$ ' $shell").with_mode(RunMode::Sentinel);
    let (run, heard) = zsh.start_for_call(request, true);
    zsh.screen_row_starts("sh-nested").await;
    let result = zsh.no_wait_until_exit(run, &heard, 6).await;
    assert_eq!(result.delimiter, Delimiter::Sentinel);
    assert_eq!(result.exit_code, Some(0));

    zsh.sessions.write(zsh.conversation, Bytes::from_static(b"exit\r")).await.unwrap();
    let back = zsh.run("echo back").await;
    assert_eq!(back.output, "back\n");
}

#[tokio::test]
async fn e2e_a_read_that_the_shell_runs_itself_is_neither_offered_nor_answered() {
    let Some(zsh) =
        Zsh::start("e2e_a_read_that_the_shell_runs_itself_is_neither_offered_nor_answered")
    else {
        return;
    };
    // zsh's own `read` keeps the terminal in the shell's process group, as its precmd
    // hooks do after a command: input typed then reaches the shell.
    let command = r#"printf 'name? '; IFS= read -r n; printf 'hi %s\n' "$n""#;
    let (run, heard) = zsh.run_waiting(command, true, "name? ").await;
    for _ in 0..5 {
        zsh.one_look().await;
    }
    assert!(heard.inputs.borrow().is_empty(), "{:?}", heard.inputs.borrow());
    let refused =
        zsh.sessions.answer(zsh.conversation, call(), &SecretText::new("bob"), false).await;
    assert!(matches!(refused, Err(ShellError::NotWaiting { .. })), "{refused:?}");

    zsh.sessions.write(zsh.conversation, Bytes::from_static(b"eve\r")).await.unwrap();
    let result = run.await.unwrap().unwrap();
    assert!(result.output.ends_with("hi eve\n"), "{:?}", result.output);
}

#[tokio::test]
async fn e2e_hidden_input_that_nobody_can_answer_is_interrupted() {
    let Some(zsh) = Zsh::start("e2e_hidden_input_that_nobody_can_answer_is_interrupted") else {
        return;
    };
    let (run, heard) = zsh.run_waiting(GETPASS, false, "pw: ").await;
    // The run stops at the first look that sees the hidden read, so it never sleeps
    // until another one. A look is a second on the clock, moved each time the run
    // asks for another sleep.
    let mut asked = zsh.clock.requested_sleeps().len();
    zsh.clock.advance(Duration::from_secs(1));
    Wait::new("the end of the run")
        .until(|| {
            if run.is_finished() {
                return true;
            }
            let now = zsh.clock.requested_sleeps().len();
            if now > asked {
                asked = now;
                zsh.clock.advance(Duration::from_secs(1));
            }
            false
        })
        .await
        .unwrap();
    let result = run.await.unwrap().unwrap();
    assert_eq!(*heard.inputs.borrow(), [InputWait::Hidden, InputWait::None]);
    assert_eq!(result.completion, Completion::Unanswered);
    assert_eq!(result.output, "pw: ");

    // The interrupted program ends and the shell takes the next command.
    let after = zsh.run("echo after").await;
    assert_eq!(after.output, "after\n");
}

/// Waits until `path` exists.
async fn exists(path: &Path) {
    Wait::new(&path.display().to_string()).until(|| path.exists()).await.unwrap();
}

#[tokio::test]
async fn e2e_an_answer_while_a_later_precmd_hook_holds_the_terminal_never_runs() {
    let Some(zsh) =
        Zsh::start("e2e_an_answer_while_a_later_precmd_hook_holds_the_terminal_never_runs")
    else {
        return;
    };
    let dir = zsh.dir("hook");
    let (holding, release, leak) = (dir.join("holding"), dir.join("release"), dir.join("leak"));
    // A hook that the user's .zshrc adds after efr's, as a prompt plugin does. Once the
    // command line arms it, it runs an external command, which takes the terminal in a
    // process group of its own and in cooked mode, until the test lets it go.
    let zshrc = format!(
        "efr_test_hook() {{\n  (( ${{+efr_test_armed}} )) || return 0\n  unset efr_test_armed\n  \
         sh -c ': > \"{}\"; until [ -e \"{}\" ]; do sleep 0.01; done'\n}}\n\
         precmd_functions+=(efr_test_hook)\n",
        holding.display(),
        release.display()
    );
    std::fs::write(zsh.home().join(".zshrc"), zshrc).unwrap();
    let command =
        r#"sh -c 'printf "name? "; IFS= read -r n; printf "hi %s\n" "$n"'; efr_test_armed=1"#;
    let (run, mut heard) = zsh.run_waiting(command, true, "name? ").await;
    zsh.look_until(&mut heard, &[InputWait::Visible]).await;

    // The user answers at the attached screen and the program ends. The chunk with `D`
    // is held on its way to the session, as a slow store holds the reader, while the
    // hook's command holds the terminal.
    zsh.recorded.hold_at(b"\x1b]133;D");
    zsh.sessions.write(zsh.conversation, Bytes::from_static(b"bob\r")).await.unwrap();
    zsh.recorded.holding().await;
    exists(&holding).await;
    let answer = SecretText::new(format!("touch '{}'", leak.display()));
    let refused = zsh.sessions.answer(zsh.conversation, call(), &answer, false).await;

    std::fs::write(&release, "").unwrap();
    zsh.recorded.release();
    let result = run.await.unwrap().unwrap();
    assert_eq!(result.exit_code, Some(0));
    // The line editor has read whatever reached the terminal once a later run ends.
    zsh.run("true").await;
    assert!(!leak.exists(), "the answer ran as a command line");
    assert!(matches!(refused, Err(ShellError::NotWaiting { .. })), "{refused:?}");
    assert_eq!(*heard.inputs.borrow(), [InputWait::Visible, InputWait::None]);
}

/// A directory with a fake `sudo` that appends its arguments to `sudo.log` next to it
/// and prints `sudo ran` unless it is asked to forget (`-k`).
fn fake_sudo() -> (tempfile::TempDir, PathBuf) {
    use std::os::unix::fs::PermissionsExt as _;
    let bin = tempfile::tempdir().unwrap();
    let log = bin.path().join("sudo.log");
    let script = format!(
        "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\n[ \"$1\" = -k ] || echo 'sudo ran'\n",
        log.display()
    );
    let sudo = bin.path().join("sudo");
    std::fs::write(&sudo, script).unwrap();
    std::fs::set_permissions(&sudo, std::fs::Permissions::from_mode(0o755)).unwrap();
    (bin, log)
}

/// The shell with `bin` first on its `PATH`.
fn with_bin(test: &str, bin: &Path) -> Option<Zsh> {
    let path = format!("{}:/usr/bin:/bin", bin.display());
    Zsh::start_with(test, |config| {
        config.base_env.insert("PATH".to_owned(), path);
    })
}

#[tokio::test]
async fn e2e_per_call_makes_the_shell_forget_sudos_credentials_after_each_call() {
    let test = "e2e_per_call_makes_the_shell_forget_sudos_credentials_after_each_call";
    let (bin, log) = fake_sudo();
    let Some(zsh) = with_bin(test, bin.path()) else {
        return;
    };
    let forget = |command: &str| zsh.request(command).with_forget_credentials(true);

    let first = zsh
        .sessions
        .run_command(zsh.conversation, forget("sudo true"), &mut NoProgress)
        .await
        .unwrap();
    // The forget key goes out right after the first call's end, so the shell runs
    // `sudo -k` before it reads this line.
    let read_log = forget(&format!("cat '{}'", log.display()));
    let second =
        zsh.sessions.run_command(zsh.conversation, read_log, &mut NoProgress).await.unwrap();

    assert_eq!(first.output, "sudo ran\n");
    assert_eq!(second.output, "true\n-k\n", "sudo -k ran between the two calls");
    let recording = zsh.recording().await;
    assert!(!recording.contains("efr-forget"), "{recording:?}");
    assert_eq!(recording.matches("-k").count(), 1, "only cat shows it: {recording:?}");
}

#[tokio::test]
async fn e2e_keep_leaves_sudos_credentials_to_sudo() {
    let test = "e2e_keep_leaves_sudos_credentials_to_sudo";
    let (bin, log) = fake_sudo();
    let Some(zsh) = with_bin(test, bin.path()) else {
        return;
    };

    let first = zsh.run("sudo true").await;
    let second = zsh.run(&format!("cat '{}'", log.display())).await;

    assert_eq!(first.output, "sudo ran\n");
    assert_eq!(second.output, "true\n", "nothing asked sudo to forget");
}

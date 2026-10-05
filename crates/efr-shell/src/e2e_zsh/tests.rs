use std::path::{Path, PathBuf};
use std::time::Duration;

use bytes::Bytes;
use efr_holder::ChildStatus;
use efr_protocol::{CallId, InputWait, SecretText};
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
        let screen = self.sessions.screen(self.conversation).unwrap();
        loop {
            let capture = screen.snapshot(0).await.unwrap();
            if capture.snapshot.rows.iter().any(|row| efr_screen::row_text(row).contains(text)) {
                return;
            }
            tokio::task::yield_now().await;
        }
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
    std::fs::write(zsh.home().join(".zshrc"), "export PAGER=less GIT_PAGER=less\n").unwrap();
    let result = zsh.run("print -r -- $PAGER $GIT_PAGER $SYSTEMD_PAGER $MANPAGER").await;
    assert_eq!(result.output, "cat cat cat cat\n");
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
        let (mut listener, mut heard) = listener(can_answer);
        let sessions = self.sessions.clone();
        let conversation = self.conversation;
        let request =
            self.request(command).with_call(call()).with_timeout(Duration::from_secs(600));
        let run = tokio::spawn(async move {
            sessions.run_command(conversation, request, &mut listener).await
        });
        heard.output(text).await;
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
    while zsh.sessions.state(zsh.conversation).await.unwrap().phase != Phase::Running {
        tokio::task::yield_now().await;
    }
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
    let command = r#"sh -c 'printf "name? "; IFS= read -r n; printf "hi %s\n" "$n"'"#;
    let (run, mut heard) = zsh.run_waiting(command, true, "name? ").await;
    let looks = zsh.look_until(&mut heard, &[InputWait::Visible]).await;
    assert!(looks >= 3, "visible input waits for three quiet seconds, not {looks}");

    zsh.sessions.answer(zsh.conversation, call(), &SecretText::new("bob"), false).await.unwrap();
    let result = run.await.unwrap().unwrap();
    assert_eq!(result.output, "name? bob\nhi bob\n");
    assert_eq!(*heard.inputs.borrow(), [InputWait::Visible, InputWait::None]);
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
    // until another one.
    for _ in 0..30 {
        if run.is_finished() {
            break;
        }
        zsh.clock.advance(Duration::from_secs(1));
        for _ in 0..100 {
            tokio::task::yield_now().await;
        }
    }
    let result = run.await.unwrap().unwrap();
    assert_eq!(*heard.inputs.borrow(), [InputWait::Hidden, InputWait::None]);
    assert_eq!(result.completion, Completion::Unanswered);
    assert_eq!(result.output, "pw: ");

    // The interrupted program ends and the shell takes the next command.
    let after = zsh.run("echo after").await;
    assert_eq!(after.output, "after\n");
}

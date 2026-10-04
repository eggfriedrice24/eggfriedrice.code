use std::path::{Path, PathBuf};

use bytes::Bytes;
use efr_holder::ChildStatus;
use pretty_assertions::assert_eq;
use tokio::sync::watch;

use super::Zsh;
use crate::{
    CommandResult, Completion, Delimiter, NoProgress, OutputUpdate, Phase, RunMode, RunRequest,
    ShellNotice,
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

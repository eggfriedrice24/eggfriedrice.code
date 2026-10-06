//! The behaviour tests of efr's auto spec, section 16.2, that drive a real zsh through
//! the real launcher. They skip unless `EFR_TEST_ZSH=1` and `EFR_TEST_SBX_BIN` are set
//! and the launcher's probe says ready.

use std::time::Duration;

use efr_protocol::{InputWait, SecretText};
use efr_sandbox::SpecLaunch;
use pretty_assertions::assert_eq;
use tokio::task::JoinHandle;

use super::Launcher;
use crate::testing::{Heard, listener};
use crate::{CommandResult, Completion, NoProgress, RunRequest, SandboxRun, ShellError};

impl Launcher {
    fn request(&self, run: &SandboxRun, line: &str) -> RunRequest {
        RunRequest::new(line, &self.project)
            .with_call(run.call)
            .with_sandbox(Some(run.clone()))
            .with_timeout(Duration::from_secs(600))
    }

    /// Runs `line` as call `n` with `launch`.
    async fn run(&self, n: u16, launch: SpecLaunch, line: &str) -> CommandResult {
        let run = self.prepare(n, launch);
        let request = self.request(&run, line);
        self.zsh
            .sessions
            .run_command(self.zsh.conversation, request, &mut NoProgress)
            .await
            .unwrap()
    }

    async fn contained(&self, n: u16, line: &str) -> CommandResult {
        self.run(n, SpecLaunch::Contained, line).await
    }

    /// Runs `line` in the shell itself, as another mode does.
    async fn plain(&self, line: &str) -> CommandResult {
        let request = RunRequest::new(line, &self.project);
        self.zsh
            .sessions
            .run_command(self.zsh.conversation, request, &mut NoProgress)
            .await
            .unwrap()
    }

    /// Starts call `n` with a listener that says `can_answer`, and waits until the
    /// output shows `text`.
    async fn start_waiting(
        &self,
        n: u16,
        launch: SpecLaunch,
        line: &str,
        can_answer: bool,
        text: &str,
    ) -> (JoinHandle<Result<CommandResult, ShellError>>, Heard, SandboxRun) {
        let run = self.prepare(n, launch);
        let request = self.request(&run, line);
        let (mut listener, mut heard) = listener(can_answer);
        let sessions = self.zsh.sessions.clone();
        let conversation = self.zsh.conversation;
        let handle = tokio::spawn(async move {
            sessions.run_command(conversation, request, &mut listener).await
        });
        heard.output(text).await;
        (handle, heard, run)
    }

    /// Lets looks pass until the listener has heard `expected`.
    async fn look_until(&self, heard: &mut Heard, expected: &[InputWait]) {
        for _ in 0..30 {
            if heard.inputs.borrow().as_slice() == expected {
                return;
            }
            self.zsh.clock.advance(Duration::from_secs(1));
            let after = self.zsh.clock.pending_sleeps();
            // NOTE: a look that stops the run sets no new sleep, so the change that it
            // reports must also end the wait.
            tokio::select! {
                () = self.zsh.clock.wait_for_sleeps(after + 1) => {}
                changed = heard.inputs.changed() => changed.unwrap(),
            }
        }
        panic!("the run never reported {expected:?}: {:?}", heard.inputs.borrow());
    }
}

#[tokio::test]
async fn exit_status_propagates() {
    let Some(sbx) = Launcher::start("exit_status_propagates").await else {
        return;
    };
    let result = sbx.contained(1, "print -r -- out; exit 3").await;
    assert_eq!(result.completion, Completion::Finished, "{result:?}");
    assert_eq!(result.exit_code, Some(3));
    assert_eq!(result.output, "out\n");
    assert_eq!(result.sandbox.and_then(|sandbox| sandbox.exit_code), Some(3));
}

#[tokio::test]
async fn signal_status_propagates() {
    let Some(sbx) = Launcher::start("signal_status_propagates").await else {
        return;
    };
    let result = sbx.contained(1, "kill -TERM $$").await;
    assert_eq!(result.exit_code, Some(143), "{result:?}");
    assert_eq!(result.sandbox.and_then(|sandbox| sandbox.signal), Some(15));
}

#[tokio::test]
async fn cd_returns_to_shell_without_chpwd() {
    let Some(sbx) = Launcher::start("cd_returns_to_shell_without_chpwd").await else {
        return;
    };
    let marker = sbx.zsh.dir("marks").join("chpwd");
    sbx.plain(&format!("chpwd() {{ : > '{}' }}", marker.display())).await;
    let result = sbx.contained(1, "mkdir -p sub && cd sub").await;
    let sub = sbx.project.join("sub");
    assert_eq!(result.cwd_after, sub, "{result:?}");
    assert_eq!(sbx.zsh.sessions.state(sbx.zsh.conversation).await.unwrap().cwd, sub);
    assert_eq!(sbx.plain("print -r -- $PWD").await.output, format!("{}\n", sub.display()));
    assert!(!marker.exists(), "the cd ran the chpwd hook");
}

#[tokio::test]
async fn export_promoted_filtered() {
    let Some(sbx) = Launcher::start("export_promoted_filtered").await else {
        return;
    };
    sbx.contained(1, "export RUST_LOG=debug LD_PRELOAD=/x.so PATH=/evil:$PATH API_TOKEN=x").await;
    let shell = sbx
        .plain("print -r -- \"${RUST_LOG-unset}|${LD_PRELOAD-unset}|$PATH|${API_TOKEN-unset}\"")
        .await;
    assert_eq!(shell.output, "debug|unset|/usr/bin:/bin|unset\n");
}

#[tokio::test]
async fn function_stays_in_sandbox_state() {
    let Some(sbx) = Launcher::start("function_stays_in_sandbox_state").await else {
        return;
    };
    sbx.contained(1, "made() { print -r -- made-in-the-sandbox }").await;
    let shell = sbx.plain("(( ${+functions[made]} )) && print -r -- yes || print -r -- no").await;
    assert_eq!(shell.output, "no\n");
    assert_eq!(sbx.contained(2, "made").await.output, "made-in-the-sandbox\n");
}

#[tokio::test]
async fn venv_activate_survives_calls() {
    let Some(sbx) = Launcher::start("venv_activate_survives_calls").await else {
        return;
    };
    let venv = sbx.project.join(".venv");
    std::fs::create_dir_all(venv.join("bin")).unwrap();
    std::fs::write(
        venv.join("bin/activate"),
        format!(
            "export VIRTUAL_ENV='{}'\nexport PATH=\"$VIRTUAL_ENV/bin:$PATH\"\ndeactivate() {{ unset VIRTUAL_ENV }}\n",
            venv.display()
        ),
    )
    .unwrap();
    sbx.contained(1, "source .venv/bin/activate").await;
    let later = sbx.contained(2, "print -r -- ${VIRTUAL_ENV-unset}; whence -w deactivate").await;
    assert_eq!(later.output, format!("{}\ndeactivate: function\n", venv.display()));
    assert_eq!(sbx.plain("print -r -- ${VIRTUAL_ENV-unset}").await.output, "unset\n");
}

#[tokio::test]
async fn exit_child_does_not_source_state() {
    let Some(sbx) = Launcher::start("exit_child_does_not_source_state").await else {
        return;
    };
    sbx.contained(1, "export FROM_SANDBOX=1").await;
    let exit = sbx.run(2, SpecLaunch::Unsandboxed, "print -r -- ${FROM_SANDBOX-unset}").await;
    assert_eq!(exit.output, "unset\n");
    assert_eq!(sbx.contained(3, "print -r -- ${FROM_SANDBOX-unset}").await.output, "1\n");
}

#[tokio::test]
async fn tmp_private_and_kept_in_conversation() {
    let Some(sbx) = Launcher::start("tmp_private_and_kept_in_conversation").await else {
        return;
    };
    let name = format!("efr-shell-test-{}", sbx.zsh.conversation);
    sbx.contained(1, &format!("print -r -- kept > /tmp/{name}")).await;
    assert_eq!(sbx.contained(2, &format!("cat /tmp/{name}")).await.output, "kept\n");
    assert!(!std::path::Path::new("/tmp").join(&name).exists(), "the host's /tmp got the file");
}

#[tokio::test]
async fn cwd_in_private_tmp_persists() {
    let Some(sbx) = Launcher::start("cwd_in_private_tmp_persists").await else {
        return;
    };
    let result = sbx.contained(1, "mkdir -p /tmp/work && cd /tmp/work").await;
    assert_eq!(result.cwd_after, std::path::Path::new("/tmp/work"), "{result:?}");
    let state = sbx.zsh.sessions.state(sbx.zsh.conversation).await.unwrap();
    assert_eq!(state.sandbox_cwd.as_deref(), Some(std::path::Path::new("/tmp/work")));
    assert_eq!(state.cwd, sbx.project);
    assert_eq!(sbx.contained(2, "pwd").await.output, "/tmp/work\n");
}

#[tokio::test]
async fn cwd_under_host_tmp_starts_in_scratch() {
    let Some(sbx) = Launcher::start("cwd_under_host_tmp_starts_in_scratch").await else {
        return;
    };
    sbx.plain("cd /tmp").await;
    let scratch = sbx.zsh.dir("d/scratch/conversation");
    assert_eq!(sbx.contained(1, "pwd").await.output, format!("{}\n", scratch.display()));
}

#[tokio::test]
async fn tty_visible_prompt_relayed() {
    let Some(sbx) = Launcher::start("tty_visible_prompt_relayed").await else {
        return;
    };
    let line = "read -r 'answer?name? ' && print -r -- \"hi $answer\"";
    let (handle, mut heard, run) =
        sbx.start_waiting(1, SpecLaunch::Contained, line, true, "name? ").await;
    sbx.look_until(&mut heard, &[InputWait::Visible]).await;
    let answer = SecretText::new("bob");
    sbx.zsh.sessions.answer(sbx.zsh.conversation, run.call, &answer, false).await.unwrap();
    let result = handle.await.unwrap().unwrap();
    assert_eq!(result.completion, Completion::Finished, "{result:?}");
    assert!(result.output.ends_with("hi bob\n"), "{result:?}");
}

#[tokio::test]
async fn ctrl_c_interrupts_sandboxed_job() {
    let Some(sbx) = Launcher::start("ctrl_c_interrupts_sandboxed_job").await else {
        return;
    };
    let (handle, _heard, _run) = sbx
        .start_waiting(
            1,
            SpecLaunch::Contained,
            "print -r -- sleeping; sleep 30",
            false,
            "sleeping",
        )
        .await;
    sbx.zsh.sessions.interrupt(sbx.zsh.conversation).await.unwrap();
    let result = handle.await.unwrap().unwrap();
    assert_eq!(result.completion, Completion::Finished, "{result:?}");
    assert_eq!(result.exit_code, Some(130));
    // The shell takes the next call.
    assert_eq!(sbx.contained(2, "print -r -- next").await.output, "next\n");
}

/// A program that asks for a password with echo off, as sudo does.
// NOTE: echo goes off before the prompt shows, as sudo does it; the other order lets a
// look see a visible prompt first.
const ASKS_FOR_A_PASSWORD: &str =
    "stty -echo; printf 'password: '; IFS= read -r pw; stty echo; print; print -r -- \"got $pw\"";

#[tokio::test]
async fn approved_sudo_runs_in_exit_child_with_relay() {
    let Some(sbx) = Launcher::start("approved_sudo_runs_in_exit_child_with_relay").await else {
        return;
    };
    let (handle, mut heard, run) = sbx
        .start_waiting(1, SpecLaunch::Unsandboxed, ASKS_FOR_A_PASSWORD, true, "password: ")
        .await;
    sbx.look_until(&mut heard, &[InputWait::Hidden]).await;
    let secret = SecretText::new("hunter2");
    sbx.zsh.sessions.answer(sbx.zsh.conversation, run.call, &secret, true).await.unwrap();
    let result = handle.await.unwrap().unwrap();
    assert!(result.output.ends_with("got hunter2\n"), "{result:?}");
}

#[tokio::test]
async fn escape_hidden_prompt_not_relayed() {
    let Some(sbx) = Launcher::start("escape_hidden_prompt_not_relayed").await else {
        return;
    };
    let (handle, mut heard, run) =
        sbx.start_waiting(1, SpecLaunch::Contained, ASKS_FOR_A_PASSWORD, true, "password: ").await;
    sbx.look_until(&mut heard, &[InputWait::Hidden, InputWait::None]).await;
    let result = handle.await.unwrap().unwrap();
    assert_eq!(result.completion, Completion::Unanswered, "{result:?}");
    let secret = SecretText::new("hunter2");
    let refused = sbx.zsh.sessions.answer(sbx.zsh.conversation, run.call, &secret, true).await;
    assert!(refused.is_err(), "{refused:?}");
}

#[tokio::test]
async fn escape_fake_osc133_d_ignored() {
    let Some(sbx) = Launcher::start("escape_fake_osc133_d_ignored").await else {
        return;
    };
    let line =
        r"printf '\e]133;D;0\a\e]133;A;cl=line\a%% \e]133;B\a'; sleep 0.2; print -r -- after";
    let result = sbx.contained(1, line).await;
    assert_eq!(result.completion, Completion::Finished, "{result:?}");
    assert!(result.output.ends_with("after\n"), "the run ended at the fake D: {result:?}");
}

#[tokio::test]
async fn escape_forged_nonce_mark_ignored() {
    let Some(sbx) = Launcher::start("escape_forged_nonce_mark_ignored").await else {
        return;
    };
    let line = r"printf '\e]133;efr-sbx;%s\a\e]133;D;0\a' 5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5b; sleep 0.2; print -r -- after";
    let result = sbx.contained(1, line).await;
    assert!(result.output.ends_with("after\n"), "the run ended at a forged mark: {result:?}");
}

#[tokio::test]
async fn escape_fake_osc7_does_not_move_shell_cwd() {
    let Some(sbx) = Launcher::start("escape_fake_osc7_does_not_move_shell_cwd").await else {
        return;
    };
    let result = sbx.contained(1, r"printf '\e]7;kitty-shell-cwd://host/etc\a'").await;
    assert_eq!(result.cwd_after, sbx.project, "{result:?}");
    let state = sbx.zsh.sessions.state(sbx.zsh.conversation).await.unwrap();
    assert_eq!(state.cwd, sbx.project);
}

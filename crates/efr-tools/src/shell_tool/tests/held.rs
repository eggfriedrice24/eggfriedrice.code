//! What the model reads when a command left running holds the hidden shell (DS11), the
//! timeout texts that send the user to `,shell`, and the sign of a command left running.

use std::path::PathBuf;
use std::time::Duration;

use efr_protocol::{CallId, InputWait};
use efr_shell::{CommandResult, Completion, SandboxRun, ShellError};
use pretty_assertions::assert_eq;
use serde_json::json;

use super::tool;
use crate::testing::{FakeRunner, Fixture, ids};
use crate::{NoOutput, NotReady, ShellTool, Tool as _, ToolResult, not_ready_text};

const RSYNC: &str = "rsync -a -e 'ssh -o PreferredAuthentications=password'\n  u@h:src/ dst/";

#[test]
fn not_ready_text_names_what_holds_the_shell() {
    let timeout = Duration::from_secs(30);
    assert_eq!(
        not_ready_text(NotReady::Fresh, timeout),
        "The shell did not show a ready prompt within 30s, so the command was not run. No \
         command ran in this shell yet: the user's startup files (.zshrc) may wait for input \
         or draw a prompt that efr cannot read. Tell the user."
    );
    assert_eq!(
        not_ready_text(NotReady::Held, timeout),
        "The shell did not reach its prompt within 30s, so the command was not run: an \
         earlier command still runs in it. Wait for it, or tell the user that ,shell shows \
         it and stops it."
    );
    assert_eq!(
        not_ready_text(
            NotReady::Running { command: "cargo build".to_owned(), ran: Duration::from_secs(750) },
            timeout
        ),
        "The hidden shell is busy: `cargo build` still runs (for 12 min). The command was \
         not run. Wait for it, or tell the user that ,shell stops it."
    );
    let waiting = |secret, nested| NotReady::Waiting {
        command: RSYNC.to_owned(),
        input: if secret { InputWait::Hidden } else { InputWait::Visible },
        secret,
        nested,
    };
    assert_eq!(
        not_ready_text(waiting(true, false), timeout),
        "The hidden shell is busy: `rsync -a -e 'ssh -o PreferredAuthentications=password' \
         u@h:src/ dst/` still runs and waits for a password. efr told the user, who answers \
         or stops it with ,shell. The command was not run. Do not call shell again until the \
         user acts, and do not ask the user to type anything at their prompt; a background \
         job cannot reach that command."
    );
    assert!(not_ready_text(waiting(false, false), timeout).contains("waits for input. efr told"));
    assert_eq!(
        not_ready_text(
            NotReady::Waiting {
                command: "sudo -i".to_owned(),
                input: InputWait::Visible,
                secret: false,
                nested: true
            },
            timeout
        ),
        "The hidden shell is busy: `sudo -i` still runs and waits for input. The command was \
         not run. If it is a shell or a REPL that you started inside this one, call again \
         with nested_shell set to type into it. Else tell the user, who answers or stops it \
         with ,shell."
    );
}

#[test]
fn a_long_command_is_cut_on_one_line() {
    let command = format!("echo {}", "x".repeat(300));
    let text = not_ready_text(
        NotReady::Running { command, ran: Duration::from_secs(5) },
        Duration::from_secs(30),
    );
    assert!(text.contains(&format!("`echo {}...`", "x".repeat(195))), "{text}");
    assert!(text.contains("(for 5 s)"), "{text}");
}

fn contained() -> SandboxRun {
    let call: CallId = "01920000-0000-7000-8000-000000000003".parse().unwrap();
    // efr-tools has no efr-sandbox edge, so the launch comes from its wire name.
    let launch = serde_json::from_value(json!("contained")).unwrap();
    SandboxRun::new(PathBuf::from("/run/user/1000/efr/sbx/c/x"), call, [7; 16], launch)
}

async fn held_by(command: &str, waiting: InputWait, sandbox: Option<SandboxRun>) -> ToolResult {
    let error = ShellError::HeldBy {
        conversation: ids().conversation_id,
        command: command.to_owned(),
        waiting,
    };
    let tool = ShellTool::new(FakeRunner::answering(Err(error)));
    let context = Fixture::new().context().with_sandbox(sandbox);
    tool.invoke(context, json!({"command": "echo hi"}), &mut NoOutput).await.unwrap()
}

#[tokio::test]
async fn a_held_shell_is_an_error_that_names_the_command() {
    let result = held_by(RSYNC, InputWait::Hidden, None).await;
    assert!(result.is_error);
    assert!(result.output.contains("`rsync -a -e"), "{}", result.output);
    assert!(result.output.contains("waits for a password"), "{}", result.output);
    assert!(result.output.contains(",shell"), "{}", result.output);
    assert!(!result.output.contains("nested_shell"), "{}", result.output);
}

#[tokio::test]
async fn nested_shell_hint_only_outside_auto_for_a_shell() {
    // A shell that the model started, outside auto: it can type into it.
    let result = held_by("sudo -i", InputWait::Visible, None).await;
    assert!(result.output.contains("nested_shell"), "{}", result.output);
    // The same in auto, where nested_shell is refused.
    let result = held_by("sudo -i", InputWait::Visible, Some(contained())).await;
    assert!(!result.output.contains("nested_shell"), "{}", result.output);
    // A command that is no shell, outside auto.
    let result = held_by("read -r answer", InputWait::Visible, None).await;
    assert!(!result.output.contains("nested_shell"), "{}", result.output);
}

#[tokio::test]
async fn the_timeout_texts_send_the_user_to_shell_and_mark_the_command_left_running() {
    let fixture = Fixture::new();
    for (completion, needle) in [
        (Completion::Interactive, "who answers it in efr's view or after ,shell"),
        (Completion::FullScreen, "the user stops it with ,shell"),
        (Completion::StillRunning, "The user can stop it with ,shell"),
    ] {
        let outcome = CommandResult::finished(None, "", "/").with_completion(completion);
        let tool = ShellTool::new(FakeRunner::answering(Ok(outcome)));
        let result =
            tool.invoke(fixture.context(), json!({"command": "x"}), &mut NoOutput).await.unwrap();
        assert!(result.output.contains(needle), "{completion:?}: {}", result.output);
        assert!(result.left_running, "{completion:?}");
        assert!(!result.output.contains("in their terminal"), "{}", result.output);
    }
    let interactive =
        CommandResult::finished(None, "", "/").with_completion(Completion::Interactive);
    let result = ShellTool::new(FakeRunner::answering(Ok(interactive)))
        .invoke(fixture.context(), json!({"command": "x"}), &mut NoOutput)
        .await
        .unwrap();
    assert!(result.output.contains("never ask the user to type it at their prompt"));
    assert!(result.output.contains("the next call fails at once while it waits"));

    for completion in [Completion::Finished, Completion::Unanswered, Completion::NotStarted] {
        let outcome = CommandResult::finished(Some(0), "", "/").with_completion(completion);
        let tool = ShellTool::new(FakeRunner::answering(Ok(outcome)));
        let result =
            tool.invoke(fixture.context(), json!({"command": "x"}), &mut NoOutput).await.unwrap();
        assert!(!result.left_running, "{completion:?}");
    }
}

#[test]
fn the_description_names_the_soft_list_the_busy_shell_and_the_ssh_hint() {
    let description = tool().spec().description;
    for needle in [
        "rsync, scp and git over ssh may ask for a password: efr shows the prompt to the user \
         while they follow the turn.",
        "A command still running at the timeout holds the shell; efr tells the user, and the \
         next call fails at once while that command waits for input",
        "Do not start an ssh ControlMaster or use an ssh control socket (-S, -o ControlPath): \
         efr shows each password prompt to the user, and ~/.ssh is a secret path that efr \
         never opens for a command.",
        "Never ask the user to type an answer at their own prompt.",
    ] {
        assert!(description.contains(needle), "{needle:?} in {description}");
    }
}

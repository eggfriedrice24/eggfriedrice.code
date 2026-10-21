mod bus;
mod corpus;
mod held;
mod proxy;
mod sandbox;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use efr_protocol::{ApprovedBy, InputWait};
use efr_shell::{CommandResult, Completion, OutputUpdate, RunMode, RunRequest, ShellError};
use efr_test_support::TestClock;
use pretty_assertions::assert_eq;
use serde_json::json;

use super::{ShellTool, networked_git, programs};
use crate::testing::{FakeRunner, Fixture, ids};
use crate::{AccessMode, NoOutput, PathAccess, Tool as _, ToolContext, ToolError, ToolOutputSink};

fn tool() -> ShellTool {
    ShellTool::new(FakeRunner::answering(Ok(CommandResult::finished(Some(0), "", "/"))))
}

fn requirements(input: serde_json::Value) -> crate::ToolRequirements {
    let fixture = Fixture::new();
    tool().requirements(&fixture.context(), &input).unwrap()
}

fn paths_in(context: &ToolContext, command: &str) -> Vec<PathAccess> {
    tool().requirements(context, &json!({ "command": command })).unwrap().paths
}

fn read(path: impl Into<std::path::PathBuf>) -> PathAccess {
    PathAccess { path: path.into(), mode: AccessMode::Read }
}

fn tree(path: impl Into<std::path::PathBuf>) -> PathAccess {
    PathAccess { path: path.into(), mode: AccessMode::ReadTree }
}

#[test]
fn it_declares_the_command_and_the_directory_it_lists() {
    let fixture = Fixture::new();
    let requirements =
        tool().requirements(&fixture.context(), &json!({"command": "ls -la"})).unwrap();
    assert_eq!(requirements.command.as_deref(), Some("ls -la"));
    assert_eq!(requirements.command_dir, Some(fixture.cwd()));
    assert_eq!(requirements.paths, [read(fixture.cwd())]);
    assert!(!requirements.network && !requirements.interactive);
}

#[test]
fn a_secret_named_by_a_read_only_command_is_declared() {
    let fixture = Fixture::new();
    let context = fixture.context();
    assert_eq!(
        paths_in(&context, "cat ~/.ssh/id_ed25519"),
        [read(fixture.home().join(".ssh/id_ed25519"))]
    );
    assert_eq!(paths_in(&context, "rg TOKEN ~/.aws"), [tree(fixture.home().join(".aws"))]);
    assert_eq!(paths_in(&context, "wc -c < ~/.netrc"), [read(fixture.home().join(".netrc"))]);
    assert_eq!(
        paths_in(&context, "echo x >> ~/.zshrc"),
        [PathAccess { path: fixture.home().join(".zshrc"), mode: AccessMode::Write }]
    );
}

#[test]
fn ssh_o_controlpath_in_dot_ssh_is_a_read_of_dot_ssh() {
    let fixture = Fixture::new();
    let context = fixture.context();
    let socket = read(fixture.home().join(".ssh/cm"));
    for line in [
        "ssh -o ControlPath=~/.ssh/cm host true",
        "ssh -oControlPath=~/.ssh/cm host true",
        "rsync -e 'ssh' -a src/ host:dst/; ssh -o ControlMaster=yes -o ControlPath=~/.ssh/cm h",
    ] {
        assert!(paths_in(&context, line).contains(&socket), "{line}");
    }
}

#[test]
fn ssh_capital_s_in_dot_ssh_is_a_read_of_dot_ssh() {
    let fixture = Fixture::new();
    let paths = paths_in(&fixture.context(), "ssh -S ~/.ssh/cm host true");
    assert!(paths.contains(&read(fixture.home().join(".ssh/cm"))), "{paths:?}");
}

fn written(path: impl Into<std::path::PathBuf>) -> PathAccess {
    PathAccess { path: path.into(), mode: AccessMode::Write }
}

#[test]
fn the_operands_of_writer_programs_are_declared_as_writes() {
    let fixture = Fixture::new();
    let context = fixture.context();
    let home = fixture.home();
    assert_eq!(paths_in(&context, "rm -rf ~/.ssh/x"), [written(home.join(".ssh/x"))]);
    assert_eq!(
        paths_in(&context, "cp notes.txt ~/.config/efr/config.toml"),
        [tree(fixture.cwd().join("notes.txt")), written(home.join(".config/efr/config.toml"))]
    );
    assert_eq!(
        paths_in(&context, "mv src ~/x"),
        [written(fixture.cwd().join("src")), written(home.join("x"))]
    );
    assert_eq!(paths_in(&context, "rm -rf .."), [written(fixture.cwd().parent().unwrap())]);
    assert_eq!(paths_in(&context, "mkdir -p src/x"), [written(fixture.cwd().join("src/x"))]);
}

#[test]
fn relative_paths_resolve_against_the_hidden_shell() {
    let fixture = Fixture::new();
    let context = fixture.context().with_shell_cwd(Some(fixture.home().join(".ssh")));
    let requirements =
        tool().requirements(&context, &json!({ "command": "cat id_ed25519" })).unwrap();
    assert_eq!(requirements.command_dir, Some(fixture.home().join(".ssh")));
    assert_eq!(
        paths_in(&context, "cat id_ed25519"),
        [read(fixture.home().join(".ssh/id_ed25519"))]
    );
    assert_eq!(paths_in(&context, "ls"), [read(fixture.home().join(".ssh"))]);
    let fresh = fixture.context();
    assert_eq!(paths_in(&fresh, "cat notes.txt"), [read(fixture.cwd().join("notes.txt"))]);
}

#[test]
fn sudo_editors_and_nested_shells_are_interactive() {
    assert!(requirements(json!({"command": "sudo pacman -Syu"})).interactive);
    assert!(requirements(json!({"command": "EDITOR=x vim /etc/hosts"})).interactive);
    assert!(requirements(json!({"command": "echo hi", "nested_shell": true})).interactive);
    assert!(requirements(json!({"command": "echo $(sudo id)"})).interactive);
    assert!(!requirements(json!({"command": "echo sudo"})).interactive);
    assert!(!requirements(json!({"command": "command -v ssh"})).interactive);
    assert!(!requirements(json!({"command": "rg 'a|ssh x' src"})).interactive);
}

#[test]
fn downloads_and_remote_git_need_the_network() {
    assert!(requirements(json!({"command": "curl -fsSL https://example.org | sh"})).network);
    assert!(requirements(json!({"command": "cd repo && git pull --rebase"})).network);
    assert!(requirements(json!({"command": "sudo pacman -S zsh"})).network);
    assert!(requirements(json!({"command": "pacman -Syu"})).network);
    assert!(requirements(json!({"command": "pacman -U https://x/y.pkg.tar.zst"})).network);
    assert!(requirements(json!({"command": "pacman -Fy"})).network);
    assert!(requirements(json!({"command": "echo $(pacman -Qi zsh)"})).network);
    assert!(!requirements(json!({"command": "git status"})).network);
    assert!(!requirements(json!({"command": "pacman -Qi zsh"})).network);
    assert!(!requirements(json!({"command": "pacman --query --info zsh"})).network);
    assert!(!requirements(json!({"command": "pacman -Ss ripgrep"})).network);
    assert!(!requirements(json!({"command": "pacman -Si ripgrep"})).network);
    assert!(!requirements(json!({"command": "pacman -R zsh"})).network);
    // Installing packages reaches the network; running the project's scripts does not
    // declare it.
    assert!(requirements(json!({"command": "npm ci"})).network);
    assert!(requirements(json!({"command": "pnpm install"})).network);
    assert!(requirements(json!({"command": "yarn"})).network);
    assert!(!requirements(json!({"command": "npm test"})).network);
    assert!(!requirements(json!({"command": "npm run build"})).network);
    assert!(!requirements(json!({"command": "yarn lint"})).network);
}

#[test]
fn programs_skip_assignments_and_wrappers() {
    assert_eq!(
        programs("A=1 B=2 /usr/bin/env -i curl x; ls | grep y"),
        ["env", "curl", "ls", "grep"]
    );
    assert_eq!(programs("(cd /tmp && make)"), ["cd", "make"]);
    assert_eq!(programs("sudo -E nvim"), ["sudo", "nvim"]);
    assert!(networked_git("git -C repo fetch origin"));
    assert!(!networked_git("git log"));
}

#[test]
fn a_call_takes_a_manual_input_unless_its_line_goes_to_a_shell_that_reads_command_lines() {
    let takes = |input| tool().takes_manual_input(&input);
    assert!(takes(json!({"command": "./deploy"})));
    assert!(!takes(json!({"command": "sleep 60", "nested_shell": true})));
    assert!(!takes(json!({"command": "bash"})));
    assert!(!takes(json!({"command": "ssh host"})));
    assert!(!takes(json!({"cmd": "./deploy"})), "an input that does not parse");
    assert!(!takes(json!({"command": "./deploy", "background": true})), "a job reads no input");
}

#[test]
fn unknown_input_fields_are_invalid() {
    let fixture = Fixture::new();
    let tool = ShellTool::new(FakeRunner::answering(Ok(CommandResult::finished(Some(0), "", "/"))));
    let error = tool.requirements(&fixture.context(), &json!({"cmd": "ls"})).unwrap_err();
    assert!(matches!(error, ToolError::InvalidInput { .. }), "{error:?}");
}

#[tokio::test]
async fn a_finished_command_reports_output_exit_code_and_directory() {
    let fixture = Fixture::new();
    let runner = FakeRunner::answering(Ok(CommandResult::finished(Some(0), "a\nb", "/tmp")));
    let tool = ShellTool::new(runner.clone());
    let result =
        tool.invoke(fixture.context(), json!({"command": "ls"}), &mut NoOutput).await.unwrap();
    assert_eq!(result.output, "a\nb\n[exit code 0, cwd /tmp]");
    assert_eq!(result.exit_code, Some(0));
    assert!(!result.is_error);

    let request = runner.last_request();
    assert_eq!(request.command, "ls");
    assert_eq!(request.start_dir, fixture.cwd());
    assert_eq!(request.timeout, RunRequest::DEFAULT_TIMEOUT);
    assert_eq!(request.mode, RunMode::Auto);
    assert_eq!(request.call, Some(ids().call_id), "answers reach this call's command");
    assert!(!request.forget_credentials, "sudo keeps its cache unless the call says");
    assert_eq!(runner.requests.lock().unwrap()[0].0, ids().conversation_id);
}

#[tokio::test]
async fn a_call_that_forgets_credentials_asks_the_shell_to() {
    let fixture = Fixture::new();
    let runner = FakeRunner::answering(Ok(CommandResult::finished(Some(0), "", "/tmp")));
    let tool = ShellTool::new(runner.clone());
    let context = fixture.context().with_forget_credentials(true);

    tool.invoke(context, json!({"command": "sudo true"}), &mut NoOutput).await.unwrap();

    assert!(runner.last_request().forget_credentials);
}

#[tokio::test]
async fn a_call_that_forgets_before_asks_the_shell_to() {
    let fixture = Fixture::new();
    let runner = FakeRunner::answering(Ok(CommandResult::finished(Some(0), "", "/tmp")));
    let tool = ShellTool::new(runner.clone());
    let context = fixture.context().with_forget_credentials(true).with_forget_before(true);

    tool.invoke(context, json!({"command": "sudo true"}), &mut NoOutput).await.unwrap();

    let request = runner.last_request();
    assert!(request.forget_credentials && request.forget_before);
}

#[tokio::test]
async fn a_shell_without_the_integration_tells_the_model_that_full_mode_needs_it() {
    let fixture = Fixture::new();
    let conversation = ids().conversation_id;
    let tool =
        ShellTool::new(FakeRunner::answering(Err(ShellError::NoIntegration { conversation })));
    let context = fixture.context().with_forget_before(true);

    let result = tool.invoke(context, json!({"command": "true"}), &mut NoOutput).await.unwrap();

    assert!(result.is_error);
    assert_eq!(result.output, super::NO_INTEGRATION);
    assert!(result.output.starts_with("full mode needs efr's zsh integration"), "{result:?}");
}

#[tokio::test]
async fn a_failing_command_is_an_error_result() {
    let fixture = Fixture::new();
    let tool = ShellTool::new(FakeRunner::answering(Ok(CommandResult::finished(
        Some(2),
        "no such file\n",
        "/",
    ))));
    let result =
        tool.invoke(fixture.context(), json!({"command": "ls x"}), &mut NoOutput).await.unwrap();
    assert_eq!(result.output, "no such file\n[exit code 2, cwd /]");
    assert!(result.is_error);
    assert_eq!(result.exit_code, Some(2));
}

#[tokio::test]
async fn a_command_waiting_for_input_shows_the_screen() {
    let fixture = Fixture::new();
    let outcome = CommandResult::finished(None, "", "/home/u")
        .with_completion(Completion::Interactive)
        .with_screen_tail("$ sudo true\n[sudo] password for u:");
    let tool = ShellTool::new(FakeRunner::answering(Ok(outcome)));
    let result = tool
        .invoke(
            fixture.context(),
            json!({"command": "sudo true", "timeout_seconds": 5}),
            &mut NoOutput,
        )
        .await
        .unwrap();
    assert!(!result.is_error);
    assert!(
        result.output.starts_with("[still running after 5s and waiting for input."),
        "{}",
        result.output
    );
    assert!(result.output.contains("after ,shell"), "{}", result.output);
    assert!(
        result.output.contains("the next call fails at once while it waits"),
        "{}",
        result.output
    );
    assert!(!result.output.contains("shell's screen"), "{}", result.output);
    assert!(result.output.ends_with("[sudo] password for u:"), "{}", result.output);
}

#[tokio::test]
async fn a_full_screen_program_is_not_called_a_question_the_user_missed() {
    let fixture = Fixture::new();
    let outcome = CommandResult::finished(None, "", "/home/u")
        .with_completion(Completion::FullScreen)
        .with_screen_tail("~\n\"notes.txt\" 0L");
    let tool = ShellTool::new(FakeRunner::answering(Ok(outcome)));
    let result = tool
        .invoke(
            fixture.context(),
            json!({"command": "vim notes.txt", "timeout_seconds": 5}),
            &mut NoOutput,
        )
        .await
        .unwrap();
    assert!(!result.is_error);
    assert!(
        result.output.starts_with("[still running after 5s: a full-screen program"),
        "{}",
        result.output
    );
    assert!(result.output.contains("Nobody can use it"), "{}", result.output);
    assert!(result.output.contains("the next call waits"), "{}", result.output);
    assert!(!result.output.contains("follow the turn"), "{}", result.output);
    assert!(!result.output.contains("nobody did in time"), "{}", result.output);
    assert!(result.output.ends_with("\"notes.txt\" 0L"), "{}", result.output);
}

#[tokio::test]
async fn a_password_nobody_could_answer_is_an_error_that_says_what_to_do() {
    let fixture = Fixture::new();
    let outcome = CommandResult::finished(None, "[sudo] password for u: ", "/home/u")
        .with_completion(Completion::Unanswered);
    let tool = ShellTool::new(FakeRunner::answering(Ok(outcome)));
    let result = tool
        .invoke(fixture.context(), json!({"command": "sudo pacman -Syu"}), &mut NoOutput)
        .await
        .unwrap();
    assert!(result.is_error);
    assert_eq!(result.exit_code, None);
    assert!(result.output.starts_with("[sudo] password for u: \n[stopped:"), "{}", result.output);
    for needle in [
        "hidden input, such as a password",
        "no user could answer it at a terminal",
        "efr interrupted it",
        "run the command in their own terminal",
        "follow this turn in their terminal while you try again",
    ] {
        assert!(result.output.contains(needle), "{needle}: {}", result.output);
    }
}

#[tokio::test]
async fn input_waits_and_the_question_who_can_answer_reach_the_output_sink() {
    #[derive(Default)]
    struct Sink {
        inputs: Vec<(InputWait, bool)>,
        asked: usize,
    }
    impl ToolOutputSink for Sink {
        fn update(&mut self, _tail: &str, _bytes: u64) {}
        fn input_changed(&mut self, wait: InputWait, looks_secret: bool) {
            self.inputs.push((wait, looks_secret));
        }
        fn can_answer_hidden(&mut self) -> bool {
            self.asked += 1;
            false
        }
        fn can_answer(&mut self) -> bool {
            true
        }
    }
    let fixture = Fixture::new();
    let waits = vec![
        (InputWait::Visible, false),
        (InputWait::Visible, true),
        (InputWait::Hidden, false),
        (InputWait::None, false),
    ];
    let runner = FakeRunner::with_inputs(CommandResult::finished(Some(0), "", "/"), waits.clone());
    let tool = ShellTool::new(runner.clone());
    let mut sink = Sink::default();
    tool.invoke(fixture.context(), json!({"command": "sudo true"}), &mut sink).await.unwrap();
    assert_eq!(sink.inputs, waits);
    assert_eq!(sink.asked, 1);
    assert_eq!(*runner.answerable.lock().unwrap(), [false]);
    assert_eq!(*runner.followed.lock().unwrap(), [true], "the shell asks the sink who follows");
}

#[tokio::test]
async fn an_approved_interactive_call_hands_its_limit_to_the_shell_and_says_how_long_it_ran() {
    let fixture = Fixture::new();
    let clock = TestClock::new();
    let outcome = CommandResult::finished(None, "", "/home/u")
        .with_completion(Completion::Interactive)
        .with_screen_tail(":: Proceed with installation? [Y/n]");
    let runner = Arc::new(FakeRunner {
        outcomes: Mutex::new(vec![Ok(outcome)]),
        runs_for: Some((clock.clone(), Duration::from_secs(600))),
        ..FakeRunner::default()
    });
    let tool = ShellTool::new(runner.clone());
    let mut context = fixture.context().with_interactive_limit(Some(Duration::from_secs(3600)));
    context.clock = clock.shared();
    let result = tool
        .invoke(
            context,
            json!({"command": "sudo pacman -Syu", "timeout_seconds": 5}),
            &mut NoOutput,
        )
        .await
        .unwrap();
    assert_eq!(runner.last_request().interactive_limit, Some(Duration::from_secs(3600)));
    assert!(
        result.output.starts_with("[still running after 600s and waiting for input."),
        "{}",
        result.output
    );

    // A call that was not approved that way keeps its timeout.
    let runner = FakeRunner::answering(Ok(CommandResult::finished(Some(0), "", "/")));
    ShellTool::new(runner.clone())
        .invoke(fixture.context(), json!({"command": "ls"}), &mut NoOutput)
        .await
        .unwrap();
    assert_eq!(runner.last_request().interactive_limit, None);
    assert_eq!(runner.last_request().input_limit, None);
    assert!(!runner.last_request().may_prompt);
    assert_eq!(runner.last_request().approved_by, None);
}

#[tokio::test]
async fn a_call_hands_its_input_limit_its_flag_and_its_approver_to_the_shell() {
    let fixture = Fixture::new();
    let context = fixture
        .context()
        .with_input_limit(Some(Duration::from_secs(3600)))
        .with_approved_by(Some(ApprovedBy::You));
    let runner = FakeRunner::answering(Ok(CommandResult::finished(Some(0), "", "/")));
    let line = "rsync -a src/ host:dst/";
    ShellTool::new(runner.clone())
        .invoke(context, json!({ "command": line }), &mut NoOutput)
        .await
        .unwrap();
    let request = runner.last_request();
    assert_eq!(request.input_limit, Some(Duration::from_secs(3600)));
    assert!(request.may_prompt, "the soft flag goes with the run");
    assert_eq!(request.approved_by, Some(ApprovedBy::You));

    // The hard flag counts too.
    let runner = FakeRunner::answering(Ok(CommandResult::finished(Some(0), "", "/")));
    ShellTool::new(runner.clone())
        .invoke(fixture.context(), json!({"command": "sudo true"}), &mut NoOutput)
        .await
        .unwrap();
    assert!(runner.last_request().may_prompt);
}

#[test]
fn the_description_promises_no_screen_the_user_cannot_see() {
    let description = tool().spec().description;
    assert!(!description.contains("shell's screen"), "{description}");
    assert!(description.contains("The user does not see this shell"), "{description}");
    assert!(description.contains("while they follow the turn"), "{description}");
    assert!(description.contains("a password never does"), "{description}");
    assert!(description.contains("stopped at once"), "{description}");
    assert!(description.contains("the program on that inner terminal decides"), "{description}");
    assert!(description.contains("up to their interactive limit"), "{description}");
    assert!(description.contains("no editor works there"), "{description}");
    assert!(description.contains("fails at once with a message"), "{description}");
}

#[tokio::test]
async fn a_command_still_running_says_so() {
    let fixture = Fixture::new();
    let outcome = CommandResult::finished(None, "building\n", "/src")
        .with_completion(Completion::StillRunning)
        .with_screen_tail("building");
    let tool = ShellTool::new(FakeRunner::answering(Ok(outcome)));
    let result =
        tool.invoke(fixture.context(), json!({"command": "make"}), &mut NoOutput).await.unwrap();
    assert!(result.output.starts_with("building\n[still running after 30s;"), "{}", result.output);
}

#[tokio::test]
async fn a_line_that_did_not_run_is_an_error_result() {
    let fixture = Fixture::new();
    let outcome = CommandResult::finished(None, "zsh: parse error\n", "/")
        .with_completion(Completion::NotStarted);
    let tool = ShellTool::new(FakeRunner::answering(Ok(outcome)));
    let result =
        tool.invoke(fixture.context(), json!({"command": ")"}), &mut NoOutput).await.unwrap();
    assert!(result.is_error);
    assert!(result.output.contains("the command did not run"), "{}", result.output);
}

#[tokio::test]
async fn long_output_is_cut_in_the_middle() {
    let fixture = Fixture::new();
    let outcome = CommandResult::finished(Some(0), "x".repeat(5000), "/");
    let tool = ShellTool::new(FakeRunner::answering(Ok(outcome))).with_output_limit(300);
    let result =
        tool.invoke(fixture.context(), json!({"command": "yes"}), &mut NoOutput).await.unwrap();
    assert!(result.truncated);
    assert!(result.output.contains("bytes omitted"));
}

#[tokio::test]
async fn the_shells_own_truncation_is_reported() {
    let fixture = Fixture::new();
    let outcome = CommandResult::finished(Some(0), "short", "/").with_truncation(9_999_999);
    let tool = ShellTool::new(FakeRunner::answering(Ok(outcome)));
    let result =
        tool.invoke(fixture.context(), json!({"command": "seq 1e9"}), &mut NoOutput).await.unwrap();
    assert!(result.truncated);
}

#[tokio::test]
async fn timeouts_are_capped_and_nested_shells_use_sentinels() {
    let fixture = Fixture::new();
    let runner = FakeRunner::answering(Ok(CommandResult::finished(Some(0), "", "/")));
    let tool = ShellTool::new(runner.clone())
        .with_timeouts(Duration::from_secs(10), Duration::from_secs(60));
    tool.invoke(
        fixture.context(),
        json!({"command": "id", "timeout_seconds": 3600, "nested_shell": true}),
        &mut NoOutput,
    )
    .await
    .unwrap();
    let request = runner.last_request();
    assert_eq!(request.timeout, Duration::from_secs(60));
    assert_eq!(request.mode, RunMode::Sentinel);
}

#[test]
fn requirements_carry_the_capped_timeout() {
    let fixture = Fixture::new();
    let tool = tool().with_timeouts(Duration::from_secs(10), Duration::from_secs(60));
    let context = fixture.context();
    let named = tool.requirements(&context, &json!({"command": "id", "timeout_seconds": 3600}));
    assert_eq!(named.unwrap().timeout, Some(Duration::from_secs(60)));
    let default = tool.requirements(&context, &json!({"command": "id"}));
    assert_eq!(default.unwrap().timeout, Some(Duration::from_secs(10)));
}

#[tokio::test]
async fn progress_reaches_the_output_sink() {
    let fixture = Fixture::new();
    let runner = FakeRunner::with_progress(
        CommandResult::finished(Some(0), "1\n2\n", "/"),
        vec![OutputUpdate::new(2, "1\n"), OutputUpdate::new(4, "1\n2\n")],
    );
    let tool = ShellTool::new(runner);
    let mut seen = Vec::new();
    let mut out = |tail: &str, bytes: u64| seen.push((tail.to_owned(), bytes));
    tool.invoke(fixture.context(), json!({"command": "seq 2"}), &mut out).await.unwrap();
    assert_eq!(seen, [("1\n".to_owned(), 2), ("1\n2\n".to_owned(), 4)]);
}

#[tokio::test]
async fn shell_conditions_the_model_can_act_on_are_error_results() {
    let fixture = Fixture::new();
    let conversation = ids().conversation_id;
    for (error, needle) in [
        (ShellError::Busy { conversation }, "nobody can clear it from here now"),
        (
            ShellError::NotReady { conversation, fresh: false },
            "an earlier command still runs in it",
        ),
        // No command ran in a new shell yet, so none can hold it.
        (ShellError::NotReady { conversation, fresh: true }, "No command ran in this shell yet"),
        (ShellError::Exited { conversation, status: None }, "new shell"),
        (ShellError::InvalidCommand { reason: "it is empty" }, "it is empty"),
    ] {
        let tool = ShellTool::new(FakeRunner::answering(Err(error)));
        let result =
            tool.invoke(fixture.context(), json!({"command": "x"}), &mut NoOutput).await.unwrap();
        assert!(result.is_error);
        assert!(result.output.contains(needle), "{}", result.output);
        assert!(!result.output.contains("screen"), "{}", result.output);
        // An interrupt never reaches a shell that no call runs in.
        assert!(!result.output.contains("interrupt"), "{}", result.output);
    }
}

/// The member and the reason of a job rule that `input` breaks, or `None`.
fn job_refusal(input: serde_json::Value) -> Option<(&'static str, &'static str)> {
    let fixture = Fixture::new();
    let from_requirements = tool().requirements(&fixture.context(), &input).err();
    let from_request = super::job_request(&input).err();
    match (from_requirements, from_request) {
        (None, None) => None,
        (
            Some(ToolError::InvalidJob { member, reason }),
            Some(ToolError::InvalidJob { member: again, reason: same }),
        ) => {
            assert_eq!((member, reason), (again, same), "both places hold one rule");
            Some((member, reason))
        }
        other => panic!("not a job refusal: {other:?}"),
    }
}

#[test]
fn shell_background_refuses_timeout_and_nested_shell() {
    let (member, reason) =
        job_refusal(json!({"command": "npm run dev", "background": true, "timeout_seconds": 60}))
            .unwrap();
    assert_eq!(member, "background: true");
    assert!(reason.contains("timeout_seconds"), "{reason}");
    let (member, reason) =
        job_refusal(json!({"command": "make", "background": true, "nested_shell": true})).unwrap();
    assert_eq!(member, "background: true");
    assert!(reason.contains("nested_shell"), "{reason}");

    let fixture = Fixture::new();
    let job = json!({"command": "npm run dev", "background": true, "max_minutes": 120,
                     "watch": {"pattern": "error|panic", "max_wakes": 2}});
    let declared = tool().requirements(&fixture.context(), &job).unwrap();
    assert!(declared.background && declared.watch);
    assert_eq!(declared.max_minutes, Some(120));
    assert_eq!(declared.command.as_deref(), Some("npm run dev"));
    let request = super::job_request(&job).unwrap().unwrap();
    assert_eq!(request.max_minutes, Some(120));
    assert_eq!(
        request.watch.map(|watch| (watch.pattern, watch.max_wakes)),
        Some(("error|panic".to_owned(), Some(2)))
    );
    assert_eq!(super::job_request(&json!({"command": "ls"})).unwrap(), None, "a foreground call");
    let plain = tool().requirements(&fixture.context(), &json!({"command": "ls"})).unwrap();
    assert!(!plain.background && !plain.watch && plain.max_minutes.is_none());
}

#[test]
fn max_minutes_needs_background() {
    assert_eq!(
        job_refusal(json!({"command": "make", "max_minutes": 30})),
        Some(("max_minutes", "needs background: true"))
    );
    assert_eq!(
        job_refusal(json!({"command": "make", "background": false, "watch": {"pattern": "x"}})),
        Some(("watch", "needs background: true"))
    );
    assert_eq!(
        job_refusal(json!({"command": "make", "background": true, "max_minutes": 0})),
        Some(("max_minutes", "is from 1 to 1440"))
    );
    assert_eq!(
        job_refusal(json!({"command": "make", "background": true, "max_minutes": 1441})),
        Some(("max_minutes", "is from 1 to 1440"))
    );
    let long = "a".repeat(201);
    assert_eq!(
        job_refusal(json!({"command": "make", "background": true, "watch": {"pattern": long}})),
        Some(("watch.pattern", "is 1 to 200 characters"))
    );
    assert_eq!(
        job_refusal(
            json!({"command": "make", "background": true, "watch": {"pattern": "x", "max_wakes": 11}})
        ),
        Some(("watch.max_wakes", "is from 1 to 10"))
    );
    assert_eq!(
        job_refusal(json!({"command": "make", "background": true, "max_minutes": 1440,
                           "watch": {"pattern": "é".repeat(200), "max_wakes": 10}})),
        None,
        "each top is allowed"
    );
    let fixture = Fixture::new();
    let unknown =
        json!({"command": "make", "background": true, "watch": {"pattern": "x", "regex": true}});
    assert!(matches!(
        tool().requirements(&fixture.context(), &unknown),
        Err(ToolError::InvalidInput { .. })
    ));
}

#[test]
fn background_refuses_interactive_line() {
    for line in ["sudo pacman -Syu", "ssh host tail -f /var/log/x", "vim notes", "env sudo -n x"] {
        let (member, reason) = job_refusal(json!({"command": line, "background": true}))
            .unwrap_or_else(|| {
                panic!("{line} is refused as a job");
            });
        assert_eq!(member, "background: true", "{line}");
        assert!(reason.contains("no terminal"), "{line}: {reason}");
    }
    assert_eq!(job_refusal(json!({"command": "cargo build --release", "background": true})), None);
    assert_eq!(job_refusal(json!({"command": "sudo -n true"})), None, "a foreground line asks");
}

#[test]
fn job_started_text_has_four_lines() {
    let text = super::job_started_text("j3", "npm run dev", "$SCRATCH/jobs/j3.out", 60, false);
    assert_eq!(
        text,
        "job j3 started: npm run dev\noutput: $SCRATCH/jobs/j3.out\ntime limit: 60 minutes\n\
         You will get a notice when it ends. Do not poll. Do other work, or end your response."
    );
    let lines =
        super::job_started_text("j4", "cd app\n  make\tall", "$SCRATCH/jobs/j4.out", 1, false);
    assert_eq!(lines.lines().count(), 4, "{lines}");
    assert!(lines.starts_with("job j4 started: cd app make all\n"), "{lines}");
    assert!(lines.contains("time limit: 1 minute\n"));
    let long = super::job_started_text("j5", &"x".repeat(300), "$SCRATCH/jobs/j5.out", 5, false);
    assert!(long.lines().next().unwrap().ends_with("xxx..."), "a long command is cut");
}

#[test]
fn job_started_text_names_the_clamp() {
    let text = super::job_started_text("j3", "sleep 99999", "$SCRATCH/jobs/j3.out", 480, true);
    assert!(text.contains("\ntime limit: 480 minutes (the most that efr allows)\n"), "{text}");
    assert_eq!(text.lines().count(), 4);
}

#[test]
fn the_schema_names_the_job_members_with_their_limits() {
    let schema = tool().spec().input_schema;
    let properties = &schema["properties"];
    assert_eq!(properties["background"]["type"], json!("boolean"));
    assert_eq!(properties["max_minutes"]["minimum"], json!(1));
    assert_eq!(properties["max_minutes"]["maximum"], json!(1440));
    let text = schema.to_string();
    assert!(text.contains("\"maxLength\":200"), "{text}");
    assert!(text.contains("\"maximum\":10"), "{text}");
    let background = properties["background"]["description"].as_str().unwrap();
    assert!(background.contains("Outside auto, the user is asked"), "{background}");
    assert!(!background.contains("only in auto"), "{background}");
}

#[tokio::test]
async fn other_shell_failures_are_tool_errors() {
    let fixture = Fixture::new();
    let error = ShellError::NoShell { conversation: ids().conversation_id };
    let tool = ShellTool::new(FakeRunner::answering(Err(error)));
    let result = tool.invoke(fixture.context(), json!({"command": "x"}), &mut NoOutput).await;
    assert!(matches!(result, Err(ToolError::Shell { .. })), "{result:?}");
}

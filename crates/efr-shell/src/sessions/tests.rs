//! The manager over a fake holder: the test plays the shell on the other end of a
//! socketpair and drives time with a manual clock, so nothing waits on real time.

use std::os::fd::OwnedFd;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use efr_holder::{ChildStatus, Signal, SignalTarget, Size};
use efr_protocol::{CallId, ConversationId, InputWait, SecretText};
use efr_stdx::time::Clock as _;
use efr_test_support::Wait;
use pretty_assertions::assert_eq;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

use super::{CommandRunner, ShellSessions, replay_name, screen_name, tail_name};
use crate::input::Probe;
use crate::modes::{Job, Terminal};
use crate::session::{Life, Msg, SessionHandle};
use crate::testing::{
    AnsweringScreens, CountingScreens, FakeModes, FakeTerminal, Harness, Heard, OTHER_JOB, SIZE,
    Vt100Screens, conversation, listener,
};
use crate::{
    CommandResult, Completion, Delimiter, InputModes, NoProgress, OutputUpdate, Phase, RunMode,
    RunProgress, RunRequest, ScreenFactory, ShellError, ShellNotice, TerminalModes,
};

const ZSH: &str = "/usr/bin/zsh";

fn request(command: &str) -> RunRequest {
    RunRequest::new(command, "/home/u")
}

fn spawn_run(
    sessions: &ShellSessions,
    request: RunRequest,
) -> JoinHandle<Result<CommandResult, ShellError>> {
    let sessions = sessions.clone();
    tokio::spawn(
        async move { sessions.run_command(conversation(1), request, &mut NoProgress).await },
    )
}

/// Starts a run of `command`, plays a ready prompt, and checks what was typed.
async fn typed(
    harness: &Harness,
    command: &str,
) -> (FakeTerminal, JoinHandle<Result<CommandResult, ShellError>>) {
    let run = spawn_run(&harness.sessions, request(command));
    let mut terminal = harness.holder.terminal(0).await;
    terminal.prompt().await;
    let line = terminal.typed_line().await;
    assert_eq!(line, format!("\x1b[efr-clear~\x1b[200~{command}\x1b[201~\r").into_bytes());
    (terminal, run)
}

/// Waits until the screen shows `text`, without moving any clock.
async fn screen_shows(sessions: &ShellSessions, conversation: ConversationId, text: &str) {
    let screen = sessions.screen(conversation).unwrap();
    Wait::new(&format!("{text:?} on the screen"))
        .until_some_async(async || {
            let capture = screen.snapshot(0).await.unwrap();
            capture
                .snapshot
                .rows
                .iter()
                .any(|row| efr_screen::row_text(row).contains(text))
                .then_some(())
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn a_marked_run_returns_the_output_the_status_and_the_directory() {
    let harness = Harness::new(ZSH);
    let (mut terminal, run) = typed(&harness, "cd /tmp && ls").await;
    terminal.run(b"\x1b]7;kitty-shell-cwd://box/tmp\x07a\r\nb\r\n", 0).await;
    let result = run.await.unwrap().unwrap();
    assert_eq!(result.completion, Completion::Finished);
    assert_eq!(result.exit_code, Some(0));
    assert_eq!(result.output, "a\nb\n");
    assert!(!result.truncated);
    assert_eq!(result.cwd_after, PathBuf::from("/tmp"));
    assert_eq!(result.delimiter, Delimiter::Marks);
    assert_eq!(result.screen_tail, None);
}

#[tokio::test]
async fn the_output_range_points_into_the_recording() {
    let harness = Harness::new(ZSH);
    let (mut terminal, run) = typed(&harness, "echo hi").await;
    terminal.run(b"hi\r\n", 0).await;
    let result = run.await.unwrap().unwrap();
    let stream = harness.recorded.stream(terminal.pty_id);
    let range = result.output_range.unwrap();
    let start = usize::try_from(range.start.get()).unwrap();
    let end = usize::try_from(range.end.get()).unwrap();
    assert_eq!(&stream[start..end], b"hi\r\n");
}

#[tokio::test]
async fn a_dropped_run_frees_the_shell_for_the_next_run() {
    let harness = Harness::new(ZSH);
    let (mut terminal, run) = typed(&harness, "sleep 100").await;
    // The turn drops the future when the user interrupts it.
    run.abort();
    assert!(run.await.unwrap_err().is_cancelled());
    terminal.print(b"\r\n\x1b]133;C\x07").await;

    // The next run waits for the prompt instead of being refused as busy.
    let next = spawn_run(&harness.sessions, request("ls"));
    terminal.print(b"^C\r\n\x1b]133;D;130\x07").await;
    terminal.prompt().await;
    assert_eq!(terminal.typed_line().await, b"\x1b[efr-clear~\x1b[200~ls\x1b[201~\r");
    terminal.run(b"a\r\n", 0).await;
    assert_eq!(next.await.unwrap().unwrap().output, "a\n");
}

#[tokio::test]
async fn until_free_waits_for_a_run_left_running_and_times_out() {
    let harness = Harness::new(ZSH);
    // No shell yet: nothing to wait for.
    harness.sessions.until_free(conversation(1), Duration::from_secs(5)).await.unwrap();

    let (mut terminal, run) = typed(&harness, "sleep 100").await;
    terminal.print(b"\r\n\x1b]133;C\x07").await;
    harness.clock.wait_for_sleeps(1).await;
    harness.clock.advance(RunRequest::DEFAULT_TIMEOUT);
    run.await.unwrap().unwrap();

    // The run left running keeps the shell busy until its timeout.
    let sessions = harness.sessions.clone();
    let wait =
        tokio::spawn(
            async move { sessions.until_free(conversation(1), Duration::from_secs(5)).await },
        );
    harness.clock.wait_for_sleeps(1).await;
    harness.clock.advance(Duration::from_secs(5));
    assert!(matches!(wait.await.unwrap(), Err(ShellError::NotReady { fresh: false, .. })));

    // Its end frees the shell.
    let sessions = harness.sessions.clone();
    let wait =
        tokio::spawn(
            async move { sessions.until_free(conversation(1), Duration::from_secs(5)).await },
        );
    harness.clock.wait_for_sleeps(1).await;
    terminal.print(b"\x1b]133;D;0\x07").await;
    wait.await.unwrap().unwrap();
}

#[tokio::test]
async fn a_failing_command_reports_its_status() {
    let harness = Harness::new(ZSH);
    let (mut terminal, run) = typed(&harness, "false").await;
    terminal.run(b"", 1).await;
    let result = run.await.unwrap().unwrap();
    assert_eq!(result.exit_code, Some(1));
    assert_eq!(result.output, "");
    assert_eq!(harness.sessions.state(conversation(1)).await.unwrap().last_exit, Some(1));
}

#[tokio::test]
async fn output_over_the_limit_keeps_its_head_and_tail() {
    let harness = Harness::new(ZSH);
    let run = spawn_run(&harness.sessions, request("seq 1 1000").with_output_limit(8));
    let mut terminal = harness.holder.terminal(0).await;
    terminal.prompt().await;
    terminal.typed_line().await;
    terminal.run(b"0123456789abcdef", 0).await;
    let result = run.await.unwrap().unwrap();
    assert!(result.truncated);
    assert_eq!(result.output_bytes, 16);
    assert_eq!(result.output, "0123\n[... 8 bytes omitted ...]\ncdef");
}

#[tokio::test]
async fn a_redrawn_progress_display_leaves_its_last_frame() {
    let harness = Harness::new(ZSH);
    let (mut terminal, run) = typed(&harness, "docker pull alpine").await;
    terminal
        .run(b"one: 10%\r\ntwo: 0%\r\n\x1b[2Aone: 90%\r\ntwo: 50%\r\n\x1b[2Aone: done\r\ntwo: done\r\n", 0)
        .await;
    let result = run.await.unwrap().unwrap();
    assert_eq!(result.output, "one: done\ntwo: done");
    assert_eq!(result.exit_code, Some(0));
}

#[tokio::test]
async fn the_capture_screen_takes_the_width_of_the_last_resize() {
    let screens = Arc::new(CountingScreens::default());
    let harness = Harness::with(ZSH, Arc::clone(&screens) as Arc<dyn ScreenFactory>);
    let run = spawn_run(&harness.sessions, request("progress"));
    let mut terminal = harness.holder.terminal(0).await;
    let resized = Size { cols: 100, rows: 30 };
    harness.sessions.resize(conversation(1), resized).await.unwrap();
    terminal.prompt().await;
    terminal.typed_line().await;
    terminal.run(b"step 1\r\n\x1b[Astep 2\r\n", 0).await;
    assert_eq!(run.await.unwrap().unwrap().output, "step 2");
    let captures = screens.captures();
    assert_eq!(captures.len(), 1);
    assert_eq!(captures[0].0, resized);
}

#[tokio::test]
async fn a_run_left_running_shows_its_output_as_the_screen_does() {
    let harness = Harness::new(ZSH);
    let (mut terminal, run) = typed(&harness, "cargo build").await;
    terminal.print(b"\r\n\x1b]133;C\x07a: 1/3\r\nb: 1/3\r\n\x1b[2Aa: 2/3\r\nb: 2/3\r\n").await;
    screen_shows(&harness.sessions, conversation(1), "b: 2/3").await;
    harness.clock.advance(RunRequest::DEFAULT_TIMEOUT);
    let result = run.await.unwrap().unwrap();
    assert_eq!(result.completion, Completion::StillRunning);
    assert_eq!(result.output, "a: 2/3\nb: 2/3");
}

#[tokio::test]
async fn a_sentinel_run_is_replayed_too() {
    let harness = Harness::new("/bin/bash");
    let run = spawn_run(&harness.sessions, request("progress"));
    let mut terminal = harness.holder.terminal(0).await;
    let line = terminal.typed_line().await;
    let token = sentinel_token(&line);
    terminal
        .print(
            format!("__efr_{token}_b\r\nold\r\n\x1b[Anew\r\n\r\n__efr_{token}_e:0:/srv\r\n")
                .as_bytes(),
        )
        .await;
    let result = run.await.unwrap().unwrap();
    assert_eq!(result.delimiter, Delimiter::Sentinel);
    assert_eq!(result.output, "new");
}

#[tokio::test]
async fn a_run_before_the_first_prompt_waits_for_it() {
    let harness = Harness::new(ZSH);
    let run = spawn_run(&harness.sessions, request("true"));
    let mut terminal = harness.holder.terminal(0).await;
    // The shell is still starting: nothing may be typed into its startup files' output.
    terminal.print(b"loading plugins...\r\n").await;
    assert_eq!(harness.sessions.state(conversation(1)).await.unwrap().phase, Phase::Starting);
    terminal.prompt().await;
    assert_eq!(terminal.typed_line().await, b"\x1b[efr-clear~\x1b[200~true\x1b[201~\r");
    terminal.run(b"", 0).await;
    assert_eq!(run.await.unwrap().unwrap().exit_code, Some(0));
}

#[tokio::test]
async fn the_wait_for_a_new_shells_prompt_is_not_part_of_the_timeout() {
    let harness = Harness::new(ZSH);
    let run = spawn_run(&harness.sessions, request("true").with_timeout(Duration::from_secs(5)));
    let mut terminal = harness.holder.terminal(0).await;
    // Only the startup timer runs while the shell starts: the run's deadline waits for
    // the first prompt.
    harness.clock.wait_for_sleeps(1).await;
    harness.clock.advance(Duration::from_secs(8));
    terminal.print(b"a slow rc\r\n").await;
    terminal.prompt().await;
    assert_eq!(terminal.typed_line().await, b"\x1b[efr-clear~\x1b[200~true\x1b[201~\r");
    // The startup timer and the run's own deadline, which starts now.
    harness.clock.wait_for_sleeps(2).await;
    harness.clock.advance(Duration::from_secs(4));
    terminal.run(b"", 0).await;
    let result = run.await.unwrap().unwrap();
    assert_eq!(result.completion, Completion::Finished, "{result:?}");
    assert_eq!(result.exit_code, Some(0));
}

#[tokio::test]
async fn a_new_shell_whose_prompt_never_gets_ready_is_not_ready_with_no_earlier_command() {
    let harness = Harness::new(ZSH);
    let run = spawn_run(&harness.sessions, request("true").with_timeout(Duration::from_secs(5)));
    let mut terminal = harness.holder.terminal(0).await;
    // The prompt starts but never takes input, and no command ran in this shell.
    terminal.print(b"\x1b]133;A\x07").await;
    harness.clock.wait_for_sleeps(2).await;
    harness.clock.advance(Duration::from_secs(5));
    let result = run.await.unwrap();
    assert!(matches!(result, Err(ShellError::NotReady { fresh: true, .. })), "{result:?}");
}

#[tokio::test]
async fn a_shell_without_marks_falls_back_to_sentinels_after_the_startup_timeout() {
    let harness = Harness::new(ZSH);
    let run = spawn_run(&harness.sessions, request("uname"));
    let mut terminal = harness.holder.terminal(0).await;
    // The startup deadline; the run's own deadline starts after it.
    harness.clock.wait_for_sleeps(1).await;
    harness.clock.advance(Duration::from_secs(10));
    let line = terminal.typed_line().await;
    let token = sentinel_token(&line);
    terminal
        .print(format!("__efr_{token}_b\r\nLinux\r\n\r\n__efr_{token}_e:0:/home/u\r\n").as_bytes())
        .await;
    let result = run.await.unwrap().unwrap();
    assert_eq!(result.delimiter, Delimiter::Sentinel);
    assert_eq!(result.output, "Linux\n");
    assert_eq!(harness.sessions.state(conversation(1)).await.unwrap().phase, Phase::Unmarked);
}

/// The token in a typed sentinel line: the word after the first `printf` format.
fn sentinel_token(line: &[u8]) -> String {
    let line = String::from_utf8(line.to_vec()).unwrap();
    let after = line.split("_b\\n' ").nth(1).unwrap();
    after.split(';').next().unwrap().to_owned()
}

#[tokio::test]
async fn a_shell_that_is_not_a_zsh_is_driven_by_sentinels_at_once() {
    let harness = Harness::new("/bin/bash");
    let run = spawn_run(&harness.sessions, request("cd /srv; false"));
    let mut terminal = harness.holder.terminal(0).await;
    let line = terminal.typed_line().await;
    assert!(String::from_utf8_lossy(&line).contains("eval 'cd /srv; false'"));
    let token = sentinel_token(&line);
    terminal.print(format!("__efr_{token}_b\r\n\r\n__efr_{token}_e:1:/srv\r\n").as_bytes()).await;
    let result = run.await.unwrap().unwrap();
    assert_eq!(result.exit_code, Some(1));
    assert_eq!(result.cwd_after, PathBuf::from("/srv"));
    // Without marks the sentinel's directory is the shell's own.
    assert_eq!(harness.sessions.state(conversation(1)).await.unwrap().cwd, PathBuf::from("/srv"));
    harness
        .notices
        .wait_for(|notice| matches!(notice, ShellNotice::CwdChanged { cwd, .. } if cwd == Path::new("/srv")))
        .await;
    let spec = &harness.holder.specs()[0];
    assert!(!spec.env.contains_key("ZDOTDIR"), "bash gets no shim");
}

#[tokio::test]
async fn the_timeout_reports_a_command_that_waits_for_input() {
    let harness = Harness::new(ZSH);
    let (mut terminal, run) = typed(&harness, "sudo true").await;
    terminal.print(b"\r\n\x1b]133;C\x07[sudo] password for u: ").await;
    screen_shows(&harness.sessions, conversation(1), "password for u:").await;
    harness.clock.advance(RunRequest::DEFAULT_TIMEOUT);
    let result = run.await.unwrap().unwrap();
    assert_eq!(result.completion, Completion::Interactive);
    assert!(result.interactive());
    assert_eq!(result.exit_code, None);
    assert_eq!(result.output, "[sudo] password for u: ");
    assert!(result.screen_tail.unwrap().ends_with("[sudo] password for u:"));

    // The command still runs; the next run waits for its prompt.
    let next = spawn_run(&harness.sessions, request("true"));
    terminal.print(b"\r\n\x1b]133;D;0\x07").await;
    terminal.prompt().await;
    assert_eq!(terminal.typed_line().await, b"\x1b[efr-clear~\x1b[200~true\x1b[201~\r");
    terminal.run(b"", 0).await;
    assert_eq!(next.await.unwrap().unwrap().exit_code, Some(0));
}

#[tokio::test]
async fn the_timeout_reports_a_slow_command_as_still_running() {
    let harness = Harness::new(ZSH);
    let (mut terminal, run) = typed(&harness, "make").await;
    terminal.print(b"\r\n\x1b]133;C\x07compiling\r\n").await;
    screen_shows(&harness.sessions, conversation(1), "compiling").await;
    harness.clock.advance(RunRequest::DEFAULT_TIMEOUT);
    let result = run.await.unwrap().unwrap();
    assert_eq!(result.completion, Completion::StillRunning);
    assert_eq!(result.output, "compiling\n");
    assert!(result.output_range.is_some());
}

#[tokio::test]
async fn a_run_that_never_reaches_a_prompt_is_not_ready() {
    let harness = Harness::new(ZSH);
    let run = spawn_run(&harness.sessions, request("true").with_timeout(Duration::from_secs(5)));
    let mut terminal = harness.holder.terminal(0).await;
    terminal.print(b"\x1b]133;A\x07").await;
    harness.clock.wait_for_sleeps(2).await;
    harness.clock.advance(Duration::from_secs(5));
    let result = run.await.unwrap();
    assert!(matches!(result, Err(ShellError::NotReady { .. })), "{result:?}");
}

#[tokio::test]
async fn an_unfinished_line_is_cancelled_and_did_not_start() {
    let harness = Harness::new(ZSH);
    let (mut terminal, run) = typed(&harness, "echo \"oops").await;
    terminal.print(b"\r\n\x1b]133;P;k=s\x07dquote> \x1b]133;B\x07").await;
    terminal.typed_until(b"\x1b[efr-cancel~").await;
    terminal.print(b"\r\n\x1b]133;D\x07").await;
    terminal.prompt().await;
    let result = run.await.unwrap().unwrap();
    assert_eq!(result.completion, Completion::NotStarted);
    assert_eq!(result.exit_code, None);
}

#[tokio::test]
async fn a_line_that_does_not_parse_says_why() {
    let harness = Harness::new(ZSH);
    let (mut terminal, run) = typed(&harness, ")").await;
    terminal.print(b")\r\nzsh: parse error near `)'\r\n\x1b]133;D\x07").await;
    terminal.prompt().await;
    let result = run.await.unwrap().unwrap();
    assert_eq!(result.completion, Completion::NotStarted);
    assert!(result.output.contains("zsh: parse error near `)'"), "{:?}", result.output);
}

#[tokio::test]
async fn a_second_run_while_one_runs_is_busy() {
    let harness = Harness::new(ZSH);
    let (mut terminal, first) = typed(&harness, "sleep 1").await;
    let second = spawn_run(&harness.sessions, request("true")).await.unwrap();
    assert!(matches!(second, Err(ShellError::Busy { .. })), "{second:?}");
    terminal.run(b"", 0).await;
    first.await.unwrap().unwrap();
}

#[tokio::test]
async fn a_sentinel_run_goes_into_a_nested_shell() {
    let harness = Harness::new(ZSH);
    let (mut terminal, run) = typed(&harness, "sudo -i").await;
    terminal.print(b"\r\n\x1b]133;C\x07root# ").await;
    screen_shows(&harness.sessions, conversation(1), "root#").await;
    harness.clock.advance(RunRequest::DEFAULT_TIMEOUT);
    assert!(run.await.unwrap().unwrap().interactive());

    let nested = spawn_run(&harness.sessions, request("whoami").with_mode(RunMode::Sentinel));
    let line = terminal.typed_line().await;
    let token = sentinel_token(&line);
    terminal
        .print(
            format!("__efr_{token}_b\r\nroot\r\n\r\n__efr_{token}_e:0:/root\r\nroot# ").as_bytes(),
        )
        .await;
    let result = nested.await.unwrap().unwrap();
    assert_eq!(result.output, "root\n");
    assert_eq!(result.cwd_after, PathBuf::from("/root"));
    // The hidden zsh did not move: it still runs `sudo -i` in /home/u.
    let state = harness.sessions.state(conversation(1)).await.unwrap();
    assert_eq!(state.phase, Phase::Running);
    assert_eq!(state.cwd, PathBuf::from("/home/u"));
}

#[tokio::test]
async fn a_sentinel_run_left_running_holds_the_next_run_until_its_end_marker() {
    let harness = Harness::new("/bin/bash");
    let run = spawn_run(&harness.sessions, request("sleep 100"));
    let mut terminal = harness.holder.terminal(0).await;
    let line = terminal.typed_line().await;
    let token = sentinel_token(&line);
    terminal.print(format!("__efr_{token}_b\r\n").as_bytes()).await;
    harness.clock.wait_for_sleeps(1).await;
    harness.clock.advance(RunRequest::DEFAULT_TIMEOUT);
    assert_eq!(run.await.unwrap().unwrap().completion, Completion::StillRunning);

    let next = spawn_run(&harness.sessions, request("true"));
    terminal.print(format!("\r\n__efr_{token}_e:0:/\r\n").as_bytes()).await;
    let line = terminal.typed_line().await;
    assert!(String::from_utf8_lossy(&line).contains("eval 'true'"));
    let token = sentinel_token(&line);
    terminal.print(format!("__efr_{token}_b\r\n\r\n__efr_{token}_e:0:/\r\n").as_bytes()).await;
    assert_eq!(next.await.unwrap().unwrap().exit_code, Some(0));
}

#[tokio::test]
async fn invalid_commands_are_refused_without_typing() {
    let harness = Harness::new(ZSH);
    let run = spawn_run(&harness.sessions, request("   "));
    let mut terminal = harness.holder.terminal(0).await;
    terminal.prompt().await;
    let result = run.await.unwrap();
    assert!(matches!(result, Err(ShellError::InvalidCommand { .. })), "{result:?}");
    let pasted = spawn_run(&harness.sessions, request("a\x1b[201~b")).await.unwrap();
    assert!(matches!(pasted, Err(ShellError::InvalidCommand { .. })), "{pasted:?}");
}

#[tokio::test]
async fn progress_hears_the_output_grow() {
    let harness = Harness::new(ZSH);
    let (seen, mut updates) = watch::channel(OutputUpdate::default());
    let sessions = harness.sessions.clone();
    let run = tokio::spawn(async move {
        let mut progress = move |update: &OutputUpdate| {
            seen.send_replace(update.clone());
        };
        sessions.run_command(conversation(1), request("yes | head"), &mut progress).await
    });
    let mut terminal = harness.holder.terminal(0).await;
    terminal.prompt().await;
    terminal.typed_line().await;
    terminal.print(b"\r\n\x1b]133;C\x07y\r\ny\r\n").await;
    updates.wait_for(|update| update.bytes == 6).await.unwrap();
    assert_eq!(updates.borrow().tail, "y\ny\n");
    terminal.print(b"\x1b]133;D;0\x07").await;
    run.await.unwrap().unwrap();
}

/// Starts a run of `command` whose progress goes to the returned watch, and plays the
/// prompt and the start of the command.
async fn following(
    harness: &Harness,
    command: &str,
) -> (FakeTerminal, watch::Receiver<OutputUpdate>, JoinHandle<Result<CommandResult, ShellError>>) {
    let (seen, updates) = watch::channel(OutputUpdate::default());
    let sessions = harness.sessions.clone();
    let request = request(command);
    let run = tokio::spawn(async move {
        let mut progress = move |update: &OutputUpdate| {
            seen.send_replace(update.clone());
        };
        sessions.run_command(conversation(1), request, &mut progress).await
    });
    let mut terminal = harness.holder.terminal(0).await;
    terminal.prompt().await;
    terminal.typed_line().await;
    terminal.print(b"\r\n\x1b]133;C\x07").await;
    (terminal, updates, run)
}

/// Waits until the run has asked for a sleep of `duration`, without moving the clock.
async fn slept(harness: &Harness, duration: Duration) {
    Wait::new(&format!("a sleep of {duration:?}"))
        .until(|| harness.clock.requested_sleeps().contains(&duration))
        .await
        .unwrap();
}

#[tokio::test]
async fn the_live_tail_of_a_redrawn_display_is_its_current_frame() {
    let harness = Harness::new(ZSH);
    let interval = crate::ShellConfig::new("/z", std::collections::BTreeMap::new()).tail_interval;
    let (mut terminal, mut updates, run) = following(&harness, "docker pull x").await;
    terminal.print(b"one 1\r\ntwo 1\r\n").await;
    updates.wait_for(|update| update.tail == "one 1\ntwo 1\n").await.unwrap();

    // The byte cleaner would show every frame; the screen shows the current one.
    terminal.print(b"\x1b[2Aone 2\r\ntwo 2\r\n").await;
    updates.wait_for(|update| update.tail == "one 2\ntwo 2").await.unwrap();
    assert_eq!(updates.borrow().bytes, 32);

    // The next frame within the interval waits for it, so a screen is read at most
    // once per interval.
    terminal.print(b"\x1b[2Aone 3\r\ntwo 3\r\n").await;
    slept(&harness, interval).await;
    assert_eq!(updates.borrow().tail, "one 2\ntwo 2");
    harness.clock.advance(interval);
    updates.wait_for(|update| update.tail == "one 3\ntwo 3").await.unwrap();
    assert_eq!(updates.borrow().bytes, 50);

    terminal.print(b"\x1b]133;D;0\x07").await;
    assert_eq!(run.await.unwrap().unwrap().output, "one 3\ntwo 3");
}

#[tokio::test]
async fn the_live_tail_of_a_carriage_return_bar_is_its_current_line() {
    let harness = Harness::new(ZSH);
    let (mut terminal, mut updates, run) = following(&harness, "curl -O x").await;
    terminal.print(b" 10% [#    ]\r 50% [###  ]").await;
    updates.wait_for(|update| update.bytes == 25).await.unwrap();
    assert_eq!(updates.borrow().tail, " 50% [###  ]");
    terminal.print(b"\x1b]133;D;0\x07").await;
    run.await.unwrap().unwrap();
}

#[tokio::test]
async fn the_live_tail_falls_back_to_the_cleaner_when_its_screen_cannot_start() {
    let harness = Harness::with(ZSH, Arc::new(ShellScreenOnly));
    let (mut terminal, mut updates, run) = following(&harness, "docker pull x").await;
    let display = b"one 1\r\ntwo 1\r\n\x1b[2Aone 2\r\ntwo 2\r\n";
    terminal.print(display).await;
    updates.wait_for(|update| update.bytes == 32).await.unwrap();
    assert_eq!(updates.borrow().tail, "one 1\ntwo 1\none 2\ntwo 2\n");
    terminal.print(b"\x1b]133;D;0\x07").await;
    run.await.unwrap().unwrap();
}

/// Screens for the shell itself and none for anything else, as when the system refuses
/// one more thread.
#[derive(Debug)]
struct ShellScreenOnly;

impl ScreenFactory for ShellScreenOnly {
    fn spawn(
        &self,
        name: &str,
        size: Size,
    ) -> Result<(efr_screen::ScreenHandle, efr_screen::ScreenEvents), efr_screen::ScreenError> {
        if name.starts_with("screen-") {
            Vt100Screens.spawn(name, size)
        } else {
            crate::testing::NoScreens.spawn(name, size)
        }
    }
}

#[tokio::test]
async fn every_byte_reaches_the_recording_in_order() {
    let harness = Harness::new(ZSH);
    let (mut terminal, run) = typed(&harness, "echo 1").await;
    terminal.run(b"1\r\n", 0).await;
    run.await.unwrap().unwrap();
    let stream = harness.recorded.stream(terminal.pty_id);
    let expected = b"\x1b]133;A;cl=line\x07% \x1b]133;B\x07\r\n\x1b]133;C\x071\r\n\x1b]133;D;0\x07";
    assert!(stream.starts_with(expected), "{stream:?}");
}

#[tokio::test]
async fn the_spawn_spec_has_the_shim_the_arguments_and_the_start_directory() {
    let harness = Harness::new(ZSH);
    let info = harness.sessions.open(conversation(1), Path::new("/etc/nixos")).await.unwrap();
    assert!(info.spawned);
    let spec = &harness.holder.specs()[0];
    assert_eq!(spec.pty_id, info.pty_id);
    assert_eq!(spec.program, PathBuf::from(ZSH));
    assert_eq!(spec.args, ["-l", "-i"]);
    assert_eq!(spec.cwd, PathBuf::from("/etc/nixos"));
    assert_eq!(spec.size, SIZE);
    let zdotdir = harness.dir.path().join("zsh");
    assert_eq!(spec.env["ZDOTDIR"], zdotdir.to_string_lossy());
    assert_eq!(spec.env["PWD"], "/etc/nixos");
    assert!(zdotdir.join(".zshenv").is_file());
    assert!(zdotdir.join("efr-integration.zsh").is_file());
    harness
        .notices
        .wait_for(|notice| matches!(notice, ShellNotice::Started { cwd, .. } if cwd == Path::new("/etc/nixos")))
        .await;
}

#[tokio::test]
async fn open_is_idempotent() {
    let harness = Harness::new(ZSH);
    let first = harness.sessions.open(conversation(1), Path::new("/")).await.unwrap();
    let second = harness.sessions.open(conversation(1), Path::new("/tmp")).await.unwrap();
    assert!(first.spawned);
    assert!(!second.spawned);
    assert_eq!(first.pty_id, second.pty_id);
    assert_eq!(harness.holder.specs().len(), 1);
}

#[tokio::test]
async fn each_conversation_gets_its_own_shell() {
    let harness = Harness::new(ZSH);
    let one = harness.sessions.open(conversation(1), Path::new("/")).await.unwrap();
    let two = harness.sessions.open(conversation(2), Path::new("/")).await.unwrap();
    assert_ne!(one.pty_id, two.pty_id);
}

#[tokio::test]
async fn a_shell_that_exits_fails_its_run_and_the_next_run_starts_a_new_one() {
    let harness = Harness::new(ZSH);
    let (terminal, run) = typed(&harness, "exit").await;
    harness.holder.end(terminal.pty_id, ChildStatus::Exited { code: 0 });
    let result = run.await.unwrap();
    assert!(
        matches!(
            result,
            Err(ShellError::Exited { status: Some(ChildStatus::Exited { code: 0 }), .. })
        ),
        "{result:?}"
    );
    harness
        .notices
        .wait_for(|notice| matches!(notice, ShellNotice::Exited { pty_id, .. } if *pty_id == terminal.pty_id))
        .await;
    assert!(matches!(
        harness.sessions.state(conversation(1)).await,
        Err(ShellError::NoShell { .. })
    ));

    let next = spawn_run(&harness.sessions, request("true"));
    let mut second = harness.holder.terminal(1).await;
    assert_ne!(second.pty_id, terminal.pty_id);
    second.prompt().await;
    second.typed_line().await;
    second.run(b"", 0).await;
    next.await.unwrap().unwrap();
    // The ended shell's PTY was released by its actor.
    assert!(harness.holder.released().contains(&terminal.pty_id));
}

#[tokio::test]
async fn a_shell_that_dies_at_startup_is_not_respawned_in_a_loop() {
    let harness = Harness::new(ZSH);
    harness.holder.die_on_arrival(ChildStatus::Exited { code: 1 });
    for _ in 0..3 {
        let result = spawn_run(&harness.sessions, request("true")).await.unwrap();
        assert!(matches!(result, Err(ShellError::Exited { .. })), "{result:?}");
    }
    // One spawn per call at most, never a loop of them.
    assert!(harness.holder.specs().len() <= 3, "{}", harness.holder.specs().len());
}

#[tokio::test]
async fn close_hangs_up_and_waits_for_the_end() {
    let harness = Harness::new(ZSH);
    let info = harness.sessions.open(conversation(1), Path::new("/")).await.unwrap();
    harness.sessions.close(conversation(1)).await.unwrap();
    assert_eq!(harness.holder.signals(), [(info.pty_id, Signal::Hangup, SignalTarget::Child)]);
    assert!(matches!(
        harness.sessions.state(conversation(1)).await,
        Err(ShellError::NoShell { .. })
    ));
    // Closing a conversation without a shell is fine.
    harness.sessions.close(conversation(2)).await.unwrap();
}

#[tokio::test]
async fn interrupt_and_resize_go_through_the_holder() {
    let harness = Harness::new(ZSH);
    let info = harness.sessions.open(conversation(1), Path::new("/")).await.unwrap();
    harness.sessions.interrupt(conversation(1)).await.unwrap();
    let size = Size { cols: 100, rows: 30 };
    harness.sessions.resize(conversation(1), size).await.unwrap();
    assert_eq!(
        harness.holder.signals(),
        [(info.pty_id, Signal::Interrupt, SignalTarget::ForegroundGroup)]
    );
    assert_eq!(harness.holder.resizes(), [(info.pty_id, size)]);
    let capture = harness.sessions.screen(conversation(1)).unwrap().snapshot(0).await.unwrap();
    assert_eq!(capture.snapshot.size, size);
}

#[tokio::test]
async fn operations_on_a_conversation_without_a_shell_fail() {
    let harness = Harness::new(ZSH);
    assert!(matches!(
        harness.sessions.interrupt(conversation(1)).await,
        Err(ShellError::NoShell { .. })
    ));
    assert!(matches!(
        harness.sessions.write(conversation(1), Bytes::from_static(b"x")).await,
        Err(ShellError::NoShell { .. })
    ));
    assert!(harness.sessions.screen(conversation(1)).is_none());
}

#[tokio::test]
async fn written_input_reaches_the_shell() {
    let harness = Harness::new(ZSH);
    harness.sessions.open(conversation(1), Path::new("/")).await.unwrap();
    let mut terminal = harness.holder.terminal(0).await;
    harness.sessions.write(conversation(1), Bytes::from_static(b"hunter2\r")).await.unwrap();
    assert_eq!(terminal.typed_line().await, b"hunter2\r");
}

#[tokio::test]
async fn screen_replies_to_terminal_queries_reach_the_shell() {
    let harness = Harness::with(ZSH, Arc::new(AnsweringScreens));
    harness.sessions.open(conversation(1), Path::new("/")).await.unwrap();
    let mut terminal = harness.holder.terminal(0).await;
    terminal.print(b"\x1b[c").await;
    assert_eq!(terminal.typed_until(b"c").await, b"\x1b[?62c");
}

#[tokio::test]
async fn a_cwd_report_is_a_notice_and_part_of_the_state() {
    let harness = Harness::new(ZSH);
    harness.sessions.open(conversation(1), Path::new("/")).await.unwrap();
    let mut terminal = harness.holder.terminal(0).await;
    terminal.print(b"\x1b]7;kitty-shell-cwd://box/var/log\x07").await;
    harness
        .notices
        .wait_for(|notice| {
            matches!(notice, ShellNotice::CwdChanged { cwd, host, .. }
                if cwd == Path::new("/var/log") && host.as_deref() == Some("box"))
        })
        .await;
    assert_eq!(
        harness.sessions.state(conversation(1)).await.unwrap().cwd,
        PathBuf::from("/var/log")
    );
}

#[tokio::test]
async fn the_trait_runs_through_the_manager() {
    let harness = Harness::new(ZSH);
    let runner: Arc<dyn CommandRunner> = Arc::new(harness.sessions.clone());
    let run = tokio::spawn(async move {
        runner.run_command(conversation(1), request("true"), &mut NoProgress).await
    });
    let mut terminal = harness.holder.terminal(0).await;
    terminal.prompt().await;
    terminal.typed_line().await;
    terminal.run(b"", 0).await;
    assert_eq!(run.await.unwrap().unwrap().exit_code, Some(0));
}

#[test]
fn screen_names_use_the_random_end_of_the_id() {
    assert_eq!(screen_name(conversation(7)), "screen-00000007");
    assert!(screen_name(conversation(7)).len() <= 15);
    assert_eq!(replay_name(conversation(7)), "replay-00000007");
    assert!(replay_name(conversation(7)).len() <= 15);
    assert_eq!(tail_name(conversation(7)), "tail-00000007");
}

#[test]
fn a_missing_zsh_is_an_error() {
    let mut config = crate::ShellConfig::new("/tmp/zsh", std::collections::BTreeMap::new());
    config.program = None;
    let clock = efr_test_support::TestClock::new();
    let deps = crate::ShellDeps::new(
        crate::testing::FakeHolder::new(),
        Arc::new(Vt100Screens),
        clock.shared(),
        Arc::new(efr_test_support::TestRng::new(1)),
    );
    let error = ShellSessions::new(config, deps).unwrap_err();
    assert!(matches!(error, ShellError::ProgramNotFound { .. }), "{error:?}");
}

const CALL: &str = "01920000-0000-7000-8000-0000000cca11";

fn call() -> CallId {
    CALL.parse().unwrap()
}

/// Starts a run of `command` for [`call`] whose listener says whether someone can
/// answer, plays a ready prompt, and starts the command with `output`.
async fn waiting(
    harness: &Harness,
    command: &str,
    can_answer: bool,
    output: &[u8],
) -> (FakeTerminal, JoinHandle<Result<CommandResult, ShellError>>, Heard) {
    let (mut listener, heard) = listener(can_answer);
    let sessions = harness.sessions.clone();
    let request = request(command).with_call(call()).with_timeout(Duration::from_secs(600));
    let run =
        tokio::spawn(
            async move { sessions.run_command(conversation(1), request, &mut listener).await },
        );
    let mut terminal = harness.holder.terminal(0).await;
    terminal.prompt().await;
    terminal.typed_line().await;
    let mut started = b"\r\n\x1b]133;C\x07".to_vec();
    started.extend_from_slice(output);
    terminal.print(&started).await;
    (terminal, run, heard)
}

/// Lets one look pass: waits until the run sleeps until its next look (with its
/// deadline and the shell's startup timer), then moves the clock by the quiet period.
async fn one_look(harness: &Harness) {
    harness.clock.wait_for_sleeps(3).await;
    harness.clock.advance(Duration::from_secs(1));
}

const HIDDEN: InputModes = InputModes::new(false, true);

#[tokio::test]
async fn a_command_that_reads_with_echo_off_waits_for_hidden_input_and_takes_an_answer() {
    let harness = Harness::new(ZSH);
    let (mut terminal, run, mut heard) =
        waiting(&harness, "sudo true", true, b"[sudo] password for u: ").await;
    harness.modes.set(Some(HIDDEN));
    one_look(&harness).await;
    heard.inputs(&[InputWait::Hidden]).await;

    let secret = SecretText::new("hunter2");
    harness.sessions.answer(conversation(1), call(), &secret, true).await.unwrap();
    assert_eq!(terminal.typed_line().await, b"hunter2\r");

    // The look after an answer says the prompt is gone, even before sudo moves on.
    one_look(&harness).await;
    heard.inputs(&[InputWait::Hidden, InputWait::None]).await;
    terminal.print(b"\r\n\x1b]133;D;0\x07").await;
    terminal.prompt().await;
    let result = run.await.unwrap().unwrap();
    assert_eq!(result.completion, Completion::Finished);
    assert_eq!(*heard.inputs.borrow(), [InputWait::Hidden, InputWait::None]);
}

#[tokio::test]
async fn hidden_input_waits_for_the_quiet_period_and_ends_when_output_resumes() {
    let harness = Harness::new(ZSH);
    let (mut terminal, run, mut heard) = waiting(&harness, "sudo true", true, b"pw: ").await;
    harness.modes.set(Some(HIDDEN));
    harness.clock.wait_for_sleeps(3).await;
    harness.clock.advance(Duration::from_millis(500));
    // Output half a second ago is not quiet enough.
    terminal.print(b"x").await;
    screen_shows(&harness.sessions, conversation(1), "pw: x").await;
    harness.clock.advance(Duration::from_millis(500));
    harness.clock.wait_for_sleeps(3).await;
    assert!(heard.inputs.borrow().is_empty(), "{:?}", heard.inputs.borrow());
    one_look(&harness).await;
    heard.inputs(&[InputWait::Hidden]).await;
    harness.modes.set(Some(InputModes::new(true, true)));
    terminal.print(b"\r\nworking\r\n").await;
    one_look(&harness).await;
    heard.inputs(&[InputWait::Hidden, InputWait::None]).await;
    terminal.print(b"\x1b]133;D;0\x07").await;
    run.await.unwrap().unwrap();
}

#[tokio::test]
async fn a_question_with_echo_on_waits_for_visible_input_at_the_first_look() {
    let harness = Harness::new(ZSH);
    let (mut terminal, run, mut heard) =
        waiting(&harness, "pacman -Syu", true, b"Proceed with installation? [Y/n] ").await;
    screen_shows(&harness.sessions, conversation(1), "[Y/n]").await;
    // One second of quiet is more than the half second that a question needs.
    one_look(&harness).await;
    heard.inputs(&[InputWait::Visible]).await;
    harness.sessions.answer(conversation(1), call(), &SecretText::new("y"), false).await.unwrap();
    assert_eq!(terminal.typed_line().await, b"y\r");
    terminal.print(b"y\r\n\x1b]133;D;0\x07").await;
    assert_eq!(run.await.unwrap().unwrap().completion, Completion::Finished);
    assert_eq!(*heard.inputs.borrow(), [InputWait::Visible, InputWait::None]);
}

#[tokio::test]
async fn a_question_that_printed_less_than_the_question_quiet_ago_waits_for_the_next_look() {
    let harness = Harness::new(ZSH);
    let (mut terminal, run, mut heard) = waiting(&harness, "./setup", true, b"step 1").await;
    harness.clock.wait_for_sleeps(3).await;
    harness.clock.advance(Duration::from_millis(600));
    terminal.print(b"\r\nContinue? ").await;
    screen_shows(&harness.sessions, conversation(1), "Continue?").await;
    // The look comes 400 ms after the question.
    harness.clock.advance(Duration::from_millis(400));
    harness.clock.wait_for_sleeps(3).await;
    assert!(heard.inputs.borrow().is_empty(), "{:?}", heard.inputs.borrow());
    one_look(&harness).await;
    heard.inputs(&[InputWait::Visible]).await;
    terminal.print(b"\r\n\x1b]133;D;0\x07").await;
    run.await.unwrap().unwrap();
}

#[tokio::test]
async fn a_prompt_with_echo_on_waits_for_visible_input_after_the_longer_quiet() {
    let harness = Harness::new(ZSH);
    let (mut terminal, run, mut heard) =
        waiting(&harness, "./install", true, b"Pick a mirror > ").await;
    screen_shows(&harness.sessions, conversation(1), "mirror >").await;
    one_look(&harness).await;
    one_look(&harness).await;
    harness.clock.wait_for_sleeps(3).await;
    assert!(
        heard.inputs.borrow().is_empty(),
        "two seconds are not enough: {:?}",
        heard.inputs.borrow()
    );
    one_look(&harness).await;
    heard.inputs(&[InputWait::Visible]).await;

    // A visible answer may go to a terminal that echoes; a hidden one may not.
    let refused =
        harness.sessions.answer(conversation(1), call(), &SecretText::new("secret"), true).await;
    assert!(matches!(refused, Err(ShellError::NotWaiting { .. })), "{refused:?}");
    harness.sessions.answer(conversation(1), call(), &SecretText::new("y"), false).await.unwrap();
    // Nothing of the refused answer was typed before this one.
    assert_eq!(terminal.typed_line().await, b"y\r");
    terminal.print(b"y\r\n\x1b]133;D;0\x07").await;
    assert_eq!(run.await.unwrap().unwrap().completion, Completion::Finished);
    assert_eq!(*heard.inputs.borrow(), [InputWait::Visible, InputWait::None]);
}

#[tokio::test]
async fn a_full_screen_program_is_not_a_visible_prompt() {
    let harness = Harness::new(ZSH);
    let (mut terminal, run, heard) =
        waiting(&harness, "htop", true, b"\x1b[?1049h\x1b[Htop - 12:00").await;
    screen_shows(&harness.sessions, conversation(1), "top - 12:00").await;
    for _ in 0..4 {
        one_look(&harness).await;
    }
    harness.clock.wait_for_sleeps(3).await;
    assert!(heard.inputs.borrow().is_empty(), "{:?}", heard.inputs.borrow());
    terminal.print(b"\x1b[?1049l\x1b]133;D;0\x07").await;
    run.await.unwrap().unwrap();
}

#[tokio::test]
async fn the_timeout_reports_a_full_screen_program_as_one() {
    let harness = Harness::new(ZSH);
    let (mut terminal, run) = typed(&harness, "vim notes.txt").await;
    terminal.print(b"\r\n\x1b]133;C\x07\x1b[?1049h\x1b[H~\r\n~\r\n\"notes.txt\" 0L").await;
    screen_shows(&harness.sessions, conversation(1), "notes.txt").await;
    harness.clock.advance(RunRequest::DEFAULT_TIMEOUT);
    let result = run.await.unwrap().unwrap();
    assert_eq!(result.completion, Completion::FullScreen);
    assert!(result.interactive());
    assert_eq!(result.exit_code, None);
    terminal.print(b"\x1b[?1049l\x1b]133;D;0\x07").await;
}

#[tokio::test]
async fn nothing_is_looked_at_before_the_command_runs_or_without_modes() {
    let harness = Harness::new(ZSH);
    let (mut listener, heard) = listener(false);
    let sessions = harness.sessions.clone();
    let run = tokio::spawn(async move {
        sessions.run_command(conversation(1), request("sudo true"), &mut listener).await
    });
    let mut terminal = harness.holder.terminal(0).await;
    terminal.prompt().await;
    terminal.typed_line().await;
    // The startup timer and the deadline: no look before `C`.
    harness.clock.wait_for_sleeps(2).await;
    harness.clock.advance(Duration::from_secs(2));
    assert_eq!(harness.modes.reads(), 0);

    harness.modes.set(None);
    terminal.print(b"\r\n\x1b]133;C\x07pw: ").await;
    one_look(&harness).await;
    one_look(&harness).await;
    harness.clock.wait_for_sleeps(3).await;
    assert!(harness.modes.reads() >= 2);
    // Modes that cannot be read never count as hidden input, so nothing is stopped.
    assert!(heard.inputs.borrow().is_empty(), "{:?}", heard.inputs.borrow());
    terminal.print(b"\r\n\x1b]133;D;0\x07").await;
    assert_eq!(run.await.unwrap().unwrap().completion, Completion::Finished);
}

#[tokio::test]
async fn hidden_input_that_nobody_can_answer_stops_the_command_at_once() {
    let harness = Harness::new(ZSH);
    let (mut terminal, run, _heard) =
        waiting(&harness, "sudo pacman -Syu", false, b"[sudo] password for u: ").await;
    harness.modes.set(Some(HIDDEN));
    one_look(&harness).await;
    let result = run.await.unwrap().unwrap();
    assert_eq!(result.completion, Completion::Unanswered);
    assert_eq!(result.exit_code, None);
    assert_eq!(result.output, "[sudo] password for u: ");
    assert_eq!(
        harness.holder.signals(),
        [(terminal.pty_id, Signal::Interrupt, SignalTarget::ForegroundGroup)]
    );

    // The interrupted command still owns the shell until its `D`.
    let next = spawn_run(&harness.sessions, request("true"));
    terminal.print(b"^C\r\nsudo: a password is required\r\n\x1b]133;D;1\x07").await;
    terminal.prompt().await;
    assert_eq!(terminal.typed_line().await, b"\x1b[efr-clear~\x1b[200~true\x1b[201~\r");
    terminal.run(b"", 0).await;
    assert_eq!(next.await.unwrap().unwrap().exit_code, Some(0));
}

#[tokio::test]
async fn the_listener_is_asked_at_every_look_while_hidden_input_waits() {
    let harness = Harness::new(ZSH);
    let (_terminal, run, mut heard) = waiting(&harness, "sudo true", true, b"pw: ").await;
    harness.modes.set(Some(HIDDEN));
    one_look(&harness).await;
    heard.inputs(&[InputWait::Hidden]).await;
    one_look(&harness).await;
    harness.clock.wait_for_sleeps(3).await;
    // The client that could answer went away.
    heard.can_answer.store(false, Ordering::SeqCst);
    one_look(&harness).await;
    let result = run.await.unwrap().unwrap();
    assert_eq!(result.completion, Completion::Unanswered);
    assert_eq!(*heard.inputs.borrow(), [InputWait::Hidden, InputWait::None]);
}

#[tokio::test]
async fn the_timeout_ends_a_wait() {
    let harness = Harness::new(ZSH);
    let (mut listener, mut heard) = listener(true);
    let sessions = harness.sessions.clone();
    let request = request("sudo true").with_call(call()).with_timeout(Duration::from_secs(5));
    let run =
        tokio::spawn(
            async move { sessions.run_command(conversation(1), request, &mut listener).await },
        );
    let mut terminal = harness.holder.terminal(0).await;
    terminal.prompt().await;
    terminal.typed_line().await;
    terminal.print(b"\r\n\x1b]133;C\x07[sudo] password for u: ").await;
    harness.modes.set(Some(HIDDEN));
    screen_shows(&harness.sessions, conversation(1), "password for u:").await;
    one_look(&harness).await;
    heard.inputs(&[InputWait::Hidden]).await;
    harness.clock.wait_for_sleeps(3).await;
    harness.clock.advance(Duration::from_secs(4));
    let result = run.await.unwrap().unwrap();
    assert_eq!(result.completion, Completion::Interactive);
    assert_eq!(*heard.inputs.borrow(), [InputWait::Hidden, InputWait::None]);
    // The call is over: its answers no longer reach the command.
    let late = harness.sessions.answer(conversation(1), call(), &SecretText::new("pw"), true).await;
    assert!(matches!(late, Err(ShellError::NoCall { .. })), "{late:?}");
}

#[tokio::test]
async fn answers_reach_only_the_running_command_of_their_call() {
    let harness = Harness::new(ZSH);
    let text = SecretText::new("hunter2");
    let no_shell = harness.sessions.answer(conversation(1), call(), &text, true).await;
    assert!(matches!(no_shell, Err(ShellError::NoShell { .. })), "{no_shell:?}");

    harness.sessions.open(conversation(1), Path::new("/")).await.unwrap();
    let idle = harness.sessions.answer(conversation(1), call(), &text, true).await;
    assert!(matches!(idle, Err(ShellError::NoCall { .. })), "{idle:?}");

    let harness = Harness::new(ZSH);
    let (mut terminal, run, mut heard) = waiting(&harness, "sudo true", true, b"pw: ").await;
    harness.modes.set(Some(HIDDEN));
    // The command runs and reads with echo off, but no look has said so: nobody was
    // asked yet. The screen is fed after the session, so the session has seen `C`.
    screen_shows(&harness.sessions, conversation(1), "pw:").await;
    let unasked = harness.sessions.answer(conversation(1), call(), &text, true).await;
    assert!(matches!(unasked, Err(ShellError::NotWaiting { .. })), "{unasked:?}");
    one_look(&harness).await;
    heard.inputs(&[InputWait::Hidden]).await;
    let other: CallId = "01920000-0000-7000-8000-0000000c0002".parse().unwrap();
    let wrong = harness.sessions.answer(conversation(1), other, &text, true).await;
    assert!(matches!(wrong, Err(ShellError::NotWaiting { .. })), "{wrong:?}");
    let raw = InputModes::new(false, false);
    harness.modes.set(Some(raw));
    let editor = harness.sessions.answer(conversation(1), call(), &text, true).await;
    assert!(matches!(editor, Err(ShellError::NotWaiting { .. })), "{editor:?}");
    let invalid =
        harness.sessions.answer(conversation(1), call(), &SecretText::new("a\rb"), true).await;
    assert!(matches!(invalid, Err(ShellError::InvalidAnswer { .. })), "{invalid:?}");
    let error = format!("{unasked:?} {wrong:?} {editor:?} {invalid:?}");
    assert!(!error.contains("hunter2"), "{error}");

    harness.modes.set(Some(HIDDEN));
    harness.sessions.answer(conversation(1), call(), &SecretText::new("ok"), true).await.unwrap();
    // Only the accepted answer was typed.
    assert_eq!(terminal.typed_line().await, b"ok\r");
    terminal.print(b"\r\n\x1b]133;D;0\x07").await;
    run.await.unwrap().unwrap();
}

#[tokio::test]
async fn an_answer_is_refused_unless_it_is_of_the_kind_of_the_reported_wait() {
    // A hidden wait: a visible answer would pass the modes, which only a hidden answer
    // needs, and could reach a relay that runs it as a command line.
    let harness = Harness::new(ZSH);
    let (mut terminal, run, mut heard) =
        waiting(&harness, "sudo true", true, b"[sudo] password for u: ").await;
    harness.modes.set(Some(HIDDEN));
    one_look(&harness).await;
    heard.inputs(&[InputWait::Hidden]).await;
    let visible =
        harness.sessions.answer(conversation(1), call(), &SecretText::new("leak"), false).await;
    assert!(matches!(visible, Err(ShellError::NotWaiting { .. })), "{visible:?}");
    harness.sessions.answer(conversation(1), call(), &SecretText::new("pw"), true).await.unwrap();
    // Nothing of the refused answer was typed before this one.
    assert_eq!(terminal.typed_line().await, b"pw\r");
    terminal.print(b"\r\n\x1b]133;D;0\x07").await;
    run.await.unwrap().unwrap();

    // A visible wait whose program turned echo off since the look: a hidden answer would
    // pass the modes, though the user was asked for a visible one.
    let harness = Harness::new(ZSH);
    let (mut terminal, run, mut heard) =
        waiting(&harness, "pacman -Syu", true, b"Proceed with installation? [Y/n] ").await;
    screen_shows(&harness.sessions, conversation(1), "[Y/n]").await;
    one_look(&harness).await;
    heard.inputs(&[InputWait::Visible]).await;
    harness.modes.set(Some(HIDDEN));
    let hidden =
        harness.sessions.answer(conversation(1), call(), &SecretText::new("secret"), true).await;
    assert!(matches!(hidden, Err(ShellError::NotWaiting { .. })), "{hidden:?}");
    harness.sessions.answer(conversation(1), call(), &SecretText::new("y"), false).await.unwrap();
    assert_eq!(terminal.typed_line().await, b"y\r");
    terminal.print(b"\r\n\x1b]133;D;0\x07").await;
    run.await.unwrap().unwrap();
}

#[tokio::test]
async fn nothing_is_asked_or_typed_while_the_shell_itself_holds_the_terminal() {
    let harness = Harness::new(ZSH);
    let (mut terminal, run, mut heard) = waiting(&harness, "sudo true", true, b"pw: ").await;
    harness.modes.set(Some(HIDDEN));
    // The job ended and zsh took the terminal back for its precmd hooks, which run in
    // cooked mode; the command's `D` has not arrived yet.
    let shell = harness.sessions.state(conversation(1)).await.unwrap().pid;
    harness.modes.set_foreground(shell);
    for _ in 0..3 {
        one_look(&harness).await;
    }
    harness.clock.wait_for_sleeps(3).await;
    assert!(heard.inputs.borrow().is_empty(), "{:?}", heard.inputs.borrow());
    for hidden in [true, false] {
        let refused =
            harness.sessions.answer(conversation(1), call(), &SecretText::new("y"), hidden).await;
        assert!(matches!(refused, Err(ShellError::NotWaiting { .. })), "{refused:?}");
    }

    // Back with the job: the same answer goes through, and it is the first one typed.
    harness.modes.set_foreground(crate::testing::JOB);
    one_look(&harness).await;
    heard.inputs(&[InputWait::Hidden]).await;
    harness.sessions.answer(conversation(1), call(), &SecretText::new("ok"), true).await.unwrap();
    assert_eq!(terminal.typed_line().await, b"ok\r");
    terminal.print(b"\r\n\x1b]133;D;0\x07").await;
    run.await.unwrap().unwrap();
}

#[tokio::test]
async fn an_answer_reaches_only_the_job_whose_wait_was_reported() {
    let harness = Harness::new(ZSH);
    let (mut terminal, run, mut heard) =
        waiting(&harness, "pacman -Syu", true, b"Proceed with installation? [Y/n] ").await;
    screen_shows(&harness.sessions, conversation(1), "[Y/n]").await;
    one_look(&harness).await;
    heard.inputs(&[InputWait::Visible]).await;

    // The job ended, and a command that a precmd hook after efr's started holds the
    // terminal in cooked mode before the command's `D` arrives.
    harness.modes.set_foreground(OTHER_JOB);
    let refused =
        harness.sessions.answer(conversation(1), call(), &SecretText::new("y"), false).await;
    assert!(matches!(refused, Err(ShellError::NotWaiting { .. })), "{refused:?}");
    // The next look says that the job that waited is gone; a wait of the new job is a
    // new change, and only an answer after it reaches that job.
    one_look(&harness).await;
    heard.inputs(&[InputWait::Visible, InputWait::None]).await;
    let early =
        harness.sessions.answer(conversation(1), call(), &SecretText::new("y"), false).await;
    assert!(matches!(early, Err(ShellError::NotWaiting { .. })), "{early:?}");
    one_look(&harness).await;
    heard.inputs(&[InputWait::Visible, InputWait::None, InputWait::Visible]).await;
    harness.sessions.answer(conversation(1), call(), &SecretText::new("n"), false).await.unwrap();
    // Nothing of the refused answers was typed before this one.
    assert_eq!(terminal.typed_line().await, b"n\r");
    terminal.print(b"n\r\n\x1b]133;D;0\x07").await;
    run.await.unwrap().unwrap();
}

#[tokio::test]
async fn a_run_whose_shell_goes_away_while_it_waits_reports_no_wait_last() {
    // The test plays the session's actor, so its inbox can close while the run's reply
    // is still open, as when the actor ends between two looks.
    let harness = Harness::new(ZSH);
    let (inbox, mut messages) = mpsc::channel(8);
    let (writer, _writes) = mpsc::channel(8);
    let (_life, life) = watch::channel(Life::Running);
    let (screen, _events) = Vt100Screens.spawn("screen-test", SIZE).unwrap();
    let (master, _shell_end) = std::os::unix::net::UnixStream::pair().unwrap();
    let master = crate::reader::master(OwnedFd::from(master)).unwrap();
    let modes = Arc::clone(&harness.modes) as Arc<dyn TerminalModes>;
    let session = SessionHandle {
        conversation: conversation(1),
        pty_id: "01920000-0000-7000-8000-0000000000aa".parse().unwrap(),
        pid: 1000,
        screen,
        inbox,
        writer,
        terminal: Terminal::new(master, modes, 1000),
        life,
        activity: watch::channel(crate::session::Activity::default()).1,
        size: Arc::new(Mutex::new(SIZE)),
        trusted: None,
    };
    let (mut listener, mut heard) = listener(true);
    let sessions = harness.sessions.clone();
    let request = request("sudo true").with_timeout(Duration::from_secs(600));
    let run = tokio::spawn(async move { sessions.run_on(&session, request, &mut listener).await });
    let Some(Msg::Run(order)) = messages.recv().await else {
        panic!("the run was not handed to the actor");
    };
    order.progress.send_modify(|progress| progress.started = true);

    // The deadline and the first look.
    harness.clock.wait_for_sleeps(2).await;
    harness.clock.advance(Duration::from_secs(1));
    let Some(Msg::Probe { id, reply }) = messages.recv().await else {
        panic!("the run did not look");
    };
    assert_eq!(id, order.id);
    let job = Some(Job { group: crate::testing::JOB, modes: HIDDEN });
    let probe = Probe { running: true, last_output: None, job, answers: 0 };
    reply.send(Some(probe)).unwrap();
    heard.inputs(&[InputWait::Hidden]).await;

    drop(messages);
    harness.clock.wait_for_sleeps(2).await;
    harness.clock.advance(Duration::from_secs(1));
    let error = run.await.unwrap().unwrap_err();
    assert!(matches!(error, ShellError::Exited { .. }), "{error:?}");
    assert_eq!(*heard.inputs.borrow(), [InputWait::Hidden, InputWait::None]);
    drop(order);
}

/// A listener at whose question nobody can answer, and at which the command's job
/// leaves the terminal to process group `group`: the shell's own, as when `sudo` gives up
/// right then, or another job's, such as a command that a later precmd hook starts.
struct LeavesAtTheQuestion {
    modes: Arc<FakeModes>,
    group: u32,
}

impl RunProgress for LeavesAtTheQuestion {
    fn update(&mut self, _update: &OutputUpdate) {}

    fn can_answer_hidden(&mut self) -> bool {
        self.modes.set_foreground(self.group);
        false
    }
}

/// Stops a hidden wait that nobody can answer after the job left the terminal to the
/// group that `next` picks from the shell's pid, and checks that no signal was sent and
/// that the command's own `D` still frees the shell.
async fn a_stop_after_the_job_left(next: impl FnOnce(u32) -> u32) {
    let harness = Harness::new(ZSH);
    let shell = harness.sessions.open(conversation(1), Path::new("/home/u")).await.unwrap().pid;
    let mut progress =
        LeavesAtTheQuestion { modes: Arc::clone(&harness.modes), group: next(shell) };
    let sessions = harness.sessions.clone();
    let sudo = request("sudo true").with_call(call()).with_timeout(Duration::from_secs(600));
    let run =
        tokio::spawn(
            async move { sessions.run_command(conversation(1), sudo, &mut progress).await },
        );
    let mut terminal = harness.holder.terminal(0).await;
    terminal.prompt().await;
    terminal.typed_line().await;
    terminal.print(b"\r\n\x1b]133;C\x07[sudo] password for u: ").await;
    harness.modes.set(Some(HIDDEN));
    one_look(&harness).await;
    let result = run.await.unwrap().unwrap();
    assert_eq!(result.completion, Completion::Unanswered);
    assert_eq!(harness.holder.signals(), []);

    let next = spawn_run(&harness.sessions, request("true"));
    terminal.print(b"\r\nsudo: timed out\r\n\x1b]133;D;1\x07").await;
    terminal.prompt().await;
    assert_eq!(terminal.typed_line().await, b"\x1b[efr-clear~\x1b[200~true\x1b[201~\r");
    terminal.run(b"", 0).await;
    assert_eq!(next.await.unwrap().unwrap().exit_code, Some(0));
}

#[tokio::test]
async fn a_stop_sends_no_signal_once_the_shell_holds_the_terminal_again() {
    // A SIGINT now would reach zsh in its precmd hooks.
    a_stop_after_the_job_left(|shell| shell).await;
}

#[tokio::test]
async fn a_stop_sends_no_signal_once_another_job_holds_the_terminal() {
    // A SIGINT now would reach a command that never waited, such as one that a precmd
    // hook after efr's runs.
    a_stop_after_the_job_left(|_| OTHER_JOB).await;
}

#[tokio::test]
async fn new_trusted_programs_restart_an_idle_shell_in_its_directory_before_the_next_run() {
    let harness = Harness::new(ZSH);
    let (mut first, run) = typed(&harness, "cd /srv").await;
    first.run(b"\x1b]7;kitty-shell-cwd://box/srv\x07", 0).await;
    run.await.unwrap().unwrap();

    harness.sessions.set_trusted_programs(vec!["ls".to_owned(), "git".to_owned()]);
    let next = spawn_run(&harness.sessions, request("ls"));
    let mut second = harness.holder.terminal(1).await;
    second.prompt().await;
    assert_eq!(second.typed_line().await, b"\x1b[efr-clear~\x1b[200~ls\x1b[201~\r");
    second.run(b"a\r\n", 0).await;
    assert_eq!(next.await.unwrap().unwrap().output, "a\n");

    assert!(
        harness.holder.signals().contains(&(first.pty_id, Signal::Hangup, SignalTarget::Child)),
        "{:?}",
        harness.holder.signals()
    );
    let spec = &harness.holder.specs()[1];
    assert_eq!(spec.cwd, PathBuf::from("/srv"));
    assert_eq!(spec.env["_EFR_HS_TRUSTED_PROGRAMS"], "ls git");
    assert_eq!(harness.holder.specs()[0].env.get("_EFR_HS_TRUSTED_PROGRAMS"), None);
}

#[tokio::test]
async fn the_same_trusted_programs_keep_the_running_shell() {
    let harness = Harness::new(ZSH);
    let (mut terminal, run) = typed(&harness, "true").await;
    terminal.run(b"", 0).await;
    run.await.unwrap().unwrap();

    harness.sessions.set_trusted_programs(Vec::new());
    let next = spawn_run(&harness.sessions, request("true"));
    assert_eq!(terminal.typed_line().await, b"\x1b[efr-clear~\x1b[200~true\x1b[201~\r");
    terminal.run(b"", 0).await;
    next.await.unwrap().unwrap();
    assert_eq!(harness.holder.specs().len(), 1);
    assert_eq!(harness.holder.signals(), []);
}

#[tokio::test]
async fn a_shell_with_old_trusted_programs_and_a_running_command_refuses_the_next_run() {
    let harness = Harness::new(ZSH);
    let (mut terminal, first) = typed(&harness, "sleep 100").await;
    terminal.print(b"\r\n\x1b]133;C\x07").await;
    Wait::new("the running command")
        .until_some_async(async || {
            let state = harness.sessions.state(conversation(1)).await.unwrap();
            (state.phase == Phase::Running).then_some(())
        })
        .await
        .unwrap();

    harness.sessions.set_trusted_programs(vec!["sleep".to_owned()]);
    let second = spawn_run(&harness.sessions, request("true")).await.unwrap();

    assert!(matches!(second, Err(ShellError::Busy { .. })), "{second:?}");
    assert_eq!(harness.holder.signals(), [], "the running command is never killed");
    terminal.print(b"\x1b]133;D;0\x07").await;
    terminal.prompt().await;
    first.await.unwrap().unwrap();
}

#[tokio::test]
async fn a_shell_without_the_integration_keeps_running_when_the_trusted_programs_change() {
    let harness = Harness::new("/bin/bash");
    let mut terminal = None;
    for round in 0..2 {
        let run = spawn_run(&harness.sessions, request("true"));
        if terminal.is_none() {
            terminal = Some(harness.holder.terminal(0).await);
        }
        let shell = terminal.as_mut().unwrap();
        let token = sentinel_token(&shell.typed_line().await);
        shell.print(format!("__efr_{token}_b\r\n__efr_{token}_e:0:/\r\n").as_bytes()).await;
        run.await.unwrap().unwrap();
        if round == 0 {
            harness.sessions.set_trusted_programs(vec!["ls".to_owned()]);
        }
    }

    assert_eq!(harness.holder.specs().len(), 1);
    assert_eq!(harness.holder.signals(), []);
}

#[tokio::test]
async fn a_new_start_applies_to_shells_spawned_after_it() {
    let harness = Harness::new(ZSH);
    harness.sessions.open(conversation(1), Path::new("/")).await.unwrap();

    harness.sessions.set_start(Some(Path::new("/usr/bin/zsh-5.9")), false).unwrap();
    harness.sessions.open(conversation(2), Path::new("/")).await.unwrap();

    let specs = harness.holder.specs();
    assert_eq!(specs[0].program, PathBuf::from(ZSH));
    assert_eq!(specs[0].args, ["-l", "-i"]);
    assert_eq!(specs[1].program, PathBuf::from("/usr/bin/zsh-5.9"));
    assert_eq!(specs[1].args, ["-i"]);
    assert!(specs[1].env.contains_key("ZDOTDIR"), "a zsh still gets the integration");
}

/// The modes a relay such as `sudo`'s own pty leaves on the hidden shell's terminal.
const RELAY: InputModes = InputModes::new(false, false);

/// Plays a run of `command` whose program prints `prompt` with the terminal in `modes`,
/// until the look that reports a visible wait; returns what the listener heard.
async fn visible_wait(
    command: &str,
    modes: InputModes,
    prompt: &[u8],
) -> (Vec<InputWait>, Vec<bool>) {
    let harness = Harness::new(ZSH);
    let (mut terminal, run, mut heard) = waiting(&harness, command, true, prompt).await;
    harness.modes.set(Some(modes));
    one_look(&harness).await;
    heard.inputs(&[InputWait::Visible]).await;
    // The answer is a visible one either way, so the kind check holds.
    harness.sessions.answer(conversation(1), call(), &SecretText::new("pw"), false).await.unwrap();
    assert_eq!(terminal.typed_line().await, b"pw\r");
    terminal.print(b"\r\n\x1b]133;D;0\x07").await;
    run.await.unwrap().unwrap();
    let inputs = heard.inputs.borrow().clone();
    let secrets = heard.secrets.borrow().clone();
    (inputs, secrets)
}

#[tokio::test]
async fn a_password_prompt_behind_a_relay_looks_secret() {
    let (inputs, secrets) =
        visible_wait("sudo -u build passwd", RELAY, b"Current password: ").await;
    assert_eq!(inputs, [InputWait::Visible, InputWait::None]);
    assert_eq!(secrets, [true, false]);
}

#[tokio::test]
async fn a_question_behind_a_relay_or_a_password_prompt_in_line_mode_does_not_look_secret() {
    let (_, secrets) =
        visible_wait("sudo pacman -Syu", RELAY, b"Proceed with installation? [Y/n] ").await;
    assert_eq!(secrets, [false, false]);
    // A program that reads a line with echo on asks no password, whatever it prints.
    let cooked = InputModes::new(true, true);
    let (_, secrets) = visible_wait("./setup", cooked, b"Password: ").await;
    assert_eq!(secrets, [false, false]);
}

/// Starts a run of `command` for [`call`] with a timeout of five seconds and an
/// interactive `limit`, whose listener says `can_answer`, and starts the command with
/// `output`.
async fn approved_interactive(
    harness: &Harness,
    limit: Option<Duration>,
    can_answer: bool,
    output: &[u8],
) -> (FakeTerminal, JoinHandle<Result<CommandResult, ShellError>>, Heard) {
    let (mut listener, heard) = listener(can_answer);
    let sessions = harness.sessions.clone();
    let request = request("sudo pacman -Syu")
        .with_call(call())
        .with_timeout(Duration::from_secs(5))
        .with_interactive_limit(limit);
    let run =
        tokio::spawn(
            async move { sessions.run_command(conversation(1), request, &mut listener).await },
        );
    let mut terminal = harness.holder.terminal(0).await;
    terminal.prompt().await;
    terminal.typed_line().await;
    let mut started = b"\r\n\x1b]133;C\x07".to_vec();
    started.extend_from_slice(output);
    terminal.print(&started).await;
    (terminal, run, heard)
}

/// Moves the clock one second at a time, `seconds` times, each once the run sleeps
/// again: `sleeps` is how many sleeps are pending then (the deadline, the look, and the
/// shell's startup timer for the first ten seconds).
async fn seconds(harness: &Harness, seconds: u64) {
    for _ in 0..seconds {
        let started = harness.clock.now();
        let sleeps = if started.duration_since(START_OF_TEST).as_secs() < 10 { 3 } else { 2 };
        harness.clock.wait_for_sleeps(sleeps).await;
        harness.clock.advance(Duration::from_secs(1));
    }
}

const START_OF_TEST: jiff::Timestamp = efr_test_support::TestClock::START;

#[tokio::test]
async fn an_approved_interactive_run_waits_past_its_timeout_while_someone_can_answer() {
    let harness = Harness::new(ZSH);
    let (mut terminal, run, mut heard) = approved_interactive(
        &harness,
        Some(Duration::from_secs(60)),
        true,
        b":: Proceed with installation? [Y/n] ",
    )
    .await;
    seconds(&harness, 12).await;
    harness.clock.wait_for_sleeps(2).await;
    assert!(!run.is_finished(), "the timeout of five seconds passed");
    heard.inputs(&[InputWait::Visible]).await;
    // An answer still reaches the command past the timeout.
    harness.sessions.answer(conversation(1), call(), &SecretText::new("y"), false).await.unwrap();
    assert_eq!(terminal.typed_line().await, b"y\r");
    terminal.print(b"y\r\n\x1b]133;D;0\x07").await;
    let result = run.await.unwrap().unwrap();
    assert_eq!(result.completion, Completion::Finished);
    assert_eq!(result.exit_code, Some(0));
}

#[tokio::test]
async fn an_approved_interactive_run_ends_soon_after_nobody_can_answer() {
    let harness = Harness::new(ZSH);
    let (_terminal, run, heard) = approved_interactive(
        &harness,
        Some(Duration::from_secs(60)),
        true,
        b":: Proceed with installation? [Y/n] ",
    )
    .await;
    seconds(&harness, 12).await;
    harness.clock.wait_for_sleeps(2).await;
    assert!(!run.is_finished());
    heard.can_answer.store(false, Ordering::SeqCst);
    seconds(&harness, 1).await;
    let result = run.await.unwrap().unwrap();
    assert_eq!(result.completion, Completion::Interactive);
    assert_eq!(harness.clock.now().duration_since(START_OF_TEST).as_secs(), 13);
}

#[tokio::test]
async fn an_approved_interactive_run_ends_at_its_limit() {
    let harness = Harness::new(ZSH);
    let (_terminal, run, _heard) = approved_interactive(
        &harness,
        Some(Duration::from_secs(8)),
        true,
        b":: Proceed with installation? [Y/n] ",
    )
    .await;
    seconds(&harness, 7).await;
    harness.clock.wait_for_sleeps(3).await;
    assert!(!run.is_finished());
    seconds(&harness, 1).await;
    let result = run.await.unwrap().unwrap();
    assert_eq!(result.completion, Completion::Interactive);
}

#[tokio::test]
async fn a_run_keeps_its_timeout_without_a_limit_or_without_someone_to_answer() {
    for (limit, can_answer) in [(None, true), (Some(Duration::from_secs(60)), false)] {
        let harness = Harness::new(ZSH);
        let (_terminal, run, _heard) =
            approved_interactive(&harness, limit, can_answer, b"compiling\r\n").await;
        seconds(&harness, 5).await;
        let result = run.await.unwrap().unwrap();
        assert_eq!(result.completion, Completion::StillRunning, "{limit:?} {can_answer}");
    }
}

#[tokio::test]
async fn a_run_that_waits_for_the_prompt_is_not_kept_past_its_timeout() {
    let harness = Harness::new(ZSH);
    let (mut listener, _heard) = listener(true);
    let sessions = harness.sessions.clone();
    let request = request("sudo true")
        .with_call(call())
        .with_timeout(Duration::from_secs(5))
        .with_interactive_limit(Some(Duration::from_secs(60)));
    let run =
        tokio::spawn(
            async move { sessions.run_command(conversation(1), request, &mut listener).await },
        );
    // A prompt starts but never takes input: the startup timer and the deadline.
    let mut terminal = harness.holder.terminal(0).await;
    terminal.print(b"\x1b]133;A\x07").await;
    harness.clock.wait_for_sleeps(2).await;
    harness.clock.advance(Duration::from_secs(5));
    let result = run.await.unwrap();
    assert!(matches!(result, Err(ShellError::NotReady { .. })), "{result:?}");
}

#[tokio::test]
async fn a_manual_answer_reaches_a_silent_command_that_reported_no_wait() {
    let harness = Harness::new(ZSH);
    let (mut terminal, run, heard) = waiting(&harness, "./deploy", true, b"deploying\r\n").await;
    screen_shows(&harness.sessions, conversation(1), "deploying").await;
    // Before any look, no job is known that a manual answer may reach.
    let early = harness
        .sessions
        .answer_manual(conversation(1), call(), &SecretText::new("go"), false)
        .await;
    assert!(matches!(early, Err(ShellError::NotWaiting { .. })), "{early:?}");
    looked(&harness).await;
    // The command printed nothing, so no look reports a wait, and a plain answer has
    // nothing to answer.
    let plain =
        harness.sessions.answer(conversation(1), call(), &SecretText::new("go"), false).await;
    assert!(matches!(plain, Err(ShellError::NotWaiting { .. })), "{plain:?}");
    // A manual hidden answer still needs a getpass-style read.
    let hidden =
        harness.sessions.answer_manual(conversation(1), call(), &SecretText::new("pw"), true).await;
    assert!(matches!(hidden, Err(ShellError::NotWaiting { .. })), "{hidden:?}");
    let other: CallId = "01920000-0000-7000-8000-0000000c0002".parse().unwrap();
    let wrong =
        harness.sessions.answer_manual(conversation(1), other, &SecretText::new("go"), false).await;
    assert!(matches!(wrong, Err(ShellError::NotWaiting { .. })), "{wrong:?}");
    let invalid = harness
        .sessions
        .answer_manual(conversation(1), call(), &SecretText::new("a\rb"), false)
        .await;
    assert!(matches!(invalid, Err(ShellError::InvalidAnswer { .. })), "{invalid:?}");

    harness
        .sessions
        .answer_manual(conversation(1), call(), &SecretText::new("go"), false)
        .await
        .unwrap();
    // Only the accepted answer was typed.
    assert_eq!(terminal.typed_line().await, b"go\r");
    terminal.print(b"\r\n\x1b]133;D;0\x07").await;
    assert_eq!(run.await.unwrap().unwrap().completion, Completion::Finished);
    assert!(heard.inputs.borrow().is_empty(), "{:?}", heard.inputs.borrow());

    // The call is over: a manual answer no longer reaches anything.
    let late = harness
        .sessions
        .answer_manual(conversation(1), call(), &SecretText::new("go"), false)
        .await;
    assert!(matches!(late, Err(ShellError::NoCall { .. })), "{late:?}");
}

/// Lets one look pass and waits until it is over, so the session knows the job that
/// it saw.
async fn looked(harness: &Harness) {
    one_look(harness).await;
    harness.clock.wait_for_sleeps(3).await;
}

#[tokio::test]
async fn a_manual_answer_is_refused_while_a_job_that_no_look_saw_holds_the_terminal() {
    let harness = Harness::new(ZSH);
    let (mut terminal, run, _heard) = waiting(&harness, "./deploy", true, b"deploying\r\n").await;
    screen_shows(&harness.sessions, conversation(1), "deploying").await;
    looked(&harness).await;
    // The command ended, and a precmd hook after the integration's runs an external
    // command in a group of its own; the command's `D` has not arrived.
    harness.modes.set_foreground(OTHER_JOB);
    let refused = harness
        .sessions
        .answer_manual(conversation(1), call(), &SecretText::new("rm -rf ~"), false)
        .await;
    assert!(matches!(refused, Err(ShellError::NotWaiting { .. })), "{refused:?}");
    terminal.print(b"\r\n\x1b]133;D;0\x07").await;
    terminal.prompt().await;
    run.await.unwrap().unwrap();
    // Nothing was typed after the line of the run.
    let next = spawn_run(&harness.sessions, request("true"));
    assert_eq!(terminal.typed_line().await, b"\x1b[efr-clear~\x1b[200~true\x1b[201~\r");
    terminal.run(b"", 0).await;
    next.await.unwrap().unwrap();
}

#[tokio::test]
async fn a_manual_answer_is_refused_for_a_run_that_starts_a_shell() {
    let harness = Harness::new(ZSH);
    let (mut terminal, run, _heard) = waiting(&harness, "bash", true, b"$ ").await;
    screen_shows(&harness.sessions, conversation(1), "$").await;
    looked(&harness).await;
    // The answer would be the started shell's next command line.
    let refused = harness
        .sessions
        .answer_manual(conversation(1), call(), &SecretText::new("rm -rf ~"), false)
        .await;
    assert!(matches!(refused, Err(ShellError::NotWaiting { .. })), "{refused:?}");
    terminal.print(b"exit\r\n\x1b]133;D;0\x07").await;
    run.await.unwrap().unwrap();
}

#[tokio::test]
async fn a_manual_answer_is_refused_for_a_sentinel_run() {
    let harness = Harness::new(ZSH);
    let (mut listener, _heard) = listener(true);
    let sessions = harness.sessions.clone();
    let request = request("sleep 60")
        .with_call(call())
        .with_mode(RunMode::Sentinel)
        .with_timeout(Duration::from_secs(600));
    let run =
        tokio::spawn(
            async move { sessions.run_command(conversation(1), request, &mut listener).await },
        );
    let mut terminal = harness.holder.terminal(0).await;
    let token = sentinel_token(&terminal.typed_line().await);
    terminal.print(format!("__efr_{token}_b\r\n").as_bytes()).await;
    looked(&harness).await;
    // The line goes to a shell inside the hidden one, which reads what the command
    // leaves unread as its next command line, with no drain.
    let refused = harness
        .sessions
        .answer_manual(conversation(1), call(), &SecretText::new("rm -rf ~"), false)
        .await;
    assert!(matches!(refused, Err(ShellError::NotWaiting { .. })), "{refused:?}");
    terminal.print(format!("\r\n__efr_{token}_e:0:/home/u\r\n").as_bytes()).await;
    run.await.unwrap().unwrap();
}

#[tokio::test]
async fn a_manual_answer_is_refused_while_the_shell_itself_holds_the_terminal() {
    let harness = Harness::new(ZSH);
    let (mut terminal, run, _heard) = waiting(&harness, "./deploy", true, b"deploying\r\n").await;
    screen_shows(&harness.sessions, conversation(1), "deploying").await;
    // The job ended and zsh runs its precmd hooks; the command's `D` has not arrived.
    let shell = harness.sessions.state(conversation(1)).await.unwrap().pid;
    harness.modes.set_foreground(shell);
    let refused = harness
        .sessions
        .answer_manual(conversation(1), call(), &SecretText::new("rm -rf ~"), false)
        .await;
    assert!(matches!(refused, Err(ShellError::NotWaiting { .. })), "{refused:?}");
    terminal.print(b"\r\n\x1b]133;D;0\x07").await;
    terminal.prompt().await;
    run.await.unwrap().unwrap();
    // Nothing was typed after the line of the run.
    let next = spawn_run(&harness.sessions, request("true"));
    assert_eq!(terminal.typed_line().await, b"\x1b[efr-clear~\x1b[200~true\x1b[201~\r");
    terminal.run(b"", 0).await;
    next.await.unwrap().unwrap();
}

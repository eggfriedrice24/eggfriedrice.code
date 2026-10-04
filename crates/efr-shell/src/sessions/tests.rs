//! The manager over a fake holder: the test plays the shell on the other end of a
//! socketpair and drives time with a manual clock, so nothing waits on real time.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use efr_holder::{ChildStatus, Signal, SignalTarget, Size};
use efr_protocol::ConversationId;
use pretty_assertions::assert_eq;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use super::{CommandRunner, ShellSessions, replay_name, screen_name};
use crate::testing::{AnsweringScreens, CountingScreens, FakeTerminal, Harness, conversation};
use crate::{
    CommandResult, Completion, Delimiter, NoProgress, OutputUpdate, Phase, RunMode, RunRequest,
    ScreenFactory, ShellError, ShellNotice,
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

/// Yields until the screen shows `text`, without waiting on any clock.
async fn screen_shows(sessions: &ShellSessions, conversation: ConversationId, text: &str) {
    let screen = sessions.screen(conversation).unwrap();
    loop {
        let capture = screen.snapshot(0).await.unwrap();
        if capture.snapshot.rows.iter().any(|row| efr_screen::row_text(row).contains(text)) {
            return;
        }
        tokio::task::yield_now().await;
    }
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
async fn a_shell_without_marks_falls_back_to_sentinels_after_the_startup_timeout() {
    let harness = Harness::new(ZSH);
    let run = spawn_run(&harness.sessions, request("uname"));
    let mut terminal = harness.holder.terminal(0).await;
    // The startup deadline and the run's own deadline.
    harness.clock.wait_for_sleeps(2).await;
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
    assert_eq!(spec.size, crate::testing::SIZE);
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
}

#[test]
fn a_missing_zsh_is_an_error() {
    let mut config = crate::ShellConfig::new("/tmp/zsh", std::collections::BTreeMap::new());
    config.program = None;
    let clock = efr_test_support::TestClock::new();
    let deps = crate::ShellDeps::new(
        crate::testing::FakeHolder::new(),
        Arc::new(crate::testing::Vt100Screens),
        clock.shared(),
        Arc::new(efr_test_support::TestRng::new(1)),
    );
    let error = ShellSessions::new(config, deps).unwrap_err();
    assert!(matches!(error, ShellError::ProgramNotFound { .. }), "{error:?}");
}

//! The end of a sandboxed run over a fake holder (efr's auto spec, sections 3.11, 3.12
//! and 3.15). The test plays the trusted shell and the sandboxed command on the other
//! end of a socketpair, scripts the holder's foreground answer and writes the
//! launcher's files itself, so no process and no clock is real.

use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use efr_holder::{Signal, SignalTarget};
use efr_protocol::{CallId, InputWait, SecretText};
use efr_sandbox::{SandboxResult, SpecLaunch, nonce_hex};
use efr_test_support::Wait;
use pretty_assertions::assert_eq;
use tokio::task::JoinHandle;

use super::SandboxRun;
use crate::run::{FORGET_CREDENTIALS, WRAPPER_CHECK};
use crate::testing::{FakeTerminal, Harness, Vt100Screens, conversation, listener};
use crate::{
    CommandResult, Completion, InputModes, NoProgress, Phase, RunMode, RunRequest, ShellError,
};

const NONCE: [u8; 16] = [0xa5; 16];

/// The pid of the fake shell, which is also its process group.
const SHELL: u32 = 1000;

/// A process group of the sandboxed job.
const SANDBOXED_JOB: u32 = 4242;

const HIDDEN: InputModes = InputModes::new(false, true);

fn call() -> CallId {
    "01920000-0000-7000-8000-0000000ca11a".parse().unwrap()
}

/// The call's dir as efrd prepares it, below the harness's sandbox dir.
fn prepare(harness: &Harness, launch: SpecLaunch) -> SandboxRun {
    let dir =
        harness.dir.path().join("sbx").join(conversation(1).to_string()).join(call().to_string());
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("nonce"), nonce_hex(&NONCE)).unwrap();
    SandboxRun::new(dir, call(), NONCE, launch)
}

fn sandboxed(run: &SandboxRun, command: &str) -> RunRequest {
    RunRequest::new(command, "/home/u")
        .with_call(call())
        .with_sandbox(Some(run.clone()))
        .with_timeout(Duration::from_secs(600))
}

fn spawn(harness: &Harness, request: RunRequest) -> JoinHandle<Result<CommandResult, ShellError>> {
    let sessions = harness.sessions.clone();
    tokio::spawn(
        async move { sessions.run_command(conversation(1), request, &mut NoProgress).await },
    )
}

/// The bytes that type the wrapper line of [`call`].
fn wrapper_line() -> Vec<u8> {
    format!("\x1b[efr-clear~\x1b[200~{WRAPPER_CHECK}{}\x1b[201~\r", call()).into_bytes()
}

/// Starts a sandboxed run of `command`, plays a ready prompt and checks that only the
/// wrapper line was typed.
async fn typed(
    harness: &Harness,
    run: &SandboxRun,
    command: &str,
) -> (FakeTerminal, JoinHandle<Result<CommandResult, ShellError>>) {
    let handle = spawn(harness, sandboxed(run, command));
    let mut terminal = harness.holder.terminal(0).await;
    terminal.prompt().await;
    assert_eq!(terminal.typed_line().await, wrapper_line());
    (terminal, handle)
}

/// The end mark that the wrapper prints for the call.
fn end_mark() -> Vec<u8> {
    format!("\x1b]133;efr-sbx;{}\x07", nonce_hex(&NONCE)).into_bytes()
}

fn started(run: &SandboxRun) {
    std::fs::write(run.dir.join("started"), "").unwrap();
}

fn result(run: &SandboxRun, result: &SandboxResult) {
    std::fs::write(run.dir.join("result.json"), result.to_json().unwrap()).unwrap();
}

fn finished(exit_code: i32, cwd: &str) -> SandboxResult {
    SandboxResult {
        started: true,
        exit_code: Some(exit_code),
        cwd: Some(PathBuf::from(cwd)),
        state_kept: true,
        ..SandboxResult::default()
    }
}

/// What the trusted shell prints once the launcher returned: the end mark, the
/// directory report of the precmd hook, `D` and the next prompt.
async fn trusted_end(terminal: &mut FakeTerminal, cwd: &str, status: i32) {
    let mut bytes = end_mark();
    bytes.extend_from_slice(format!("\x1b]7;kitty-shell-cwd://box{cwd}\x07").as_bytes());
    bytes.extend_from_slice(format!("\x1b]133;D;{status}\x07").as_bytes());
    terminal.print(&bytes).await;
    terminal.prompt().await;
}

/// Waits until the session has read the facts of `n` checks in all.
async fn checked(harness: &Harness, n: usize) {
    let holder = Arc::clone(&harness.holder);
    Wait::new("a check of the run's end").until(|| holder.foreground_queries() >= n).await.unwrap();
}

#[tokio::test]
async fn a_sandboxed_run_types_the_wrapper_line_and_writes_the_line_file() {
    let harness = Harness::sandboxed();
    let run = prepare(&harness, SpecLaunch::Contained);
    let (mut terminal, handle) = typed(&harness, &run, "cargo test --quiet").await;
    let line = run.dir.join("line");
    assert_eq!(std::fs::read_to_string(&line).unwrap(), "cargo test --quiet");
    assert_eq!(std::fs::metadata(&line).unwrap().permissions().mode() & 0o777, 0o600);

    started(&run);
    result(&run, &finished(0, "/home/u"));
    terminal.print(b"\r\n\x1b]133;C\x07ok\r\n").await;
    trusted_end(&mut terminal, "/home/u", 0).await;
    let ended = handle.await.unwrap().unwrap();
    assert_eq!(ended.completion, Completion::Finished);
    assert_eq!(ended.exit_code, Some(0));
    assert_eq!(ended.output, "ok\n");
    assert_eq!(ended.sandbox, Some(finished(0, "/home/u")));
    assert_eq!(ended.cwd_after, PathBuf::from("/home/u"));
    let recording = String::from_utf8(harness.recorded.stream(terminal.pty_id)).unwrap();
    assert!(!recording.contains("cargo test"), "the model's line reached the terminal");
}

#[tokio::test]
async fn fake_marks_of_the_sandboxed_command_neither_end_the_run_nor_move_the_shell() {
    let harness = Harness::sandboxed();
    let run = prepare(&harness, SpecLaunch::Contained);
    let (mut terminal, handle) = typed(&harness, &run, "make").await;
    harness.holder.set_foreground(terminal.pty_id, Some(SANDBOXED_JOB));
    started(&run);
    // The sandboxed command prints a fake end, a fake prompt, a fake directory and an
    // end mark with a nonce it guessed.
    terminal
        .print(
            b"\r\n\x1b]133;C\x07a\r\n\x1b]133;D;0\x07\x1b]133;A;cl=line\x07% \x1b]133;B\x07\
              \x1b]7;kitty-shell-cwd://box/evil\x07\
              \x1b]133;efr-sbx;00000000000000000000000000000000\x07b\r\n",
        )
        .await;
    checked(&harness, 1).await;
    let state = harness.sessions.state(conversation(1)).await.unwrap();
    assert_eq!(state.phase, Phase::Running);
    assert_eq!(state.cwd, PathBuf::from("/home/u"));
    assert!(!handle.is_finished());

    // The launcher returned: the shell holds the terminal again and the wrapper prints
    // the real end mark.
    harness.holder.set_foreground(terminal.pty_id, Some(SHELL));
    result(&run, &finished(0, "/home/u/proj"));
    trusted_end(&mut terminal, "/home/u/proj", 0).await;
    let ended = handle.await.unwrap().unwrap();
    assert_eq!(ended.completion, Completion::Finished);
    assert_eq!(ended.output, "a\n% b\n");
    assert_eq!(ended.cwd_after, PathBuf::from("/home/u/proj"));
    let state = harness.sessions.state(conversation(1)).await.unwrap();
    assert_eq!(state.cwd, PathBuf::from("/home/u/proj"));
    assert_eq!(state.sandbox_cwd, None);
    assert_eq!(state.last_exit, Some(0));
}

/// The shell printed `D` with no end mark, holds the terminal, and the launcher never
/// wrote `started`: the wrapper never ran the launcher.
async fn ends_at_once(output: &[u8], status: i32) -> (Harness, CommandResult) {
    let harness = Harness::sandboxed();
    let run = prepare(&harness, SpecLaunch::Contained);
    let (mut terminal, handle) = typed(&harness, &run, "ls").await;
    terminal.run(output, status).await;
    let ended = handle.await.unwrap().unwrap();
    (harness, ended)
}

#[tokio::test]
async fn wrapper_check_failure_ends_without_timeout() {
    let (harness, ended) = ends_at_once(b"", 1).await;
    assert_eq!(ended.completion, Completion::SandboxFailed);
    assert_eq!(ended.exit_code, Some(1));
    assert_eq!(ended.sandbox, None);
    // The marks the run held back were the shell's own: it is ready for the next line.
    let sessions = harness.sessions.clone();
    let state = Wait::new("a ready prompt")
        .until_some_async(async || {
            let state = sessions.state(conversation(1)).await.unwrap();
            (state.phase == Phase::Ready).then_some(state)
        })
        .await
        .unwrap();
    assert_eq!(state.last_exit, Some(1));
}

#[tokio::test]
async fn missing_integration_ends_without_timeout() {
    let (_harness, ended) = ends_at_once(b"zsh: command not found: _efr_hs_sbx\r\n", 127).await;
    assert_eq!(ended.completion, Completion::SandboxFailed);
    assert_eq!(ended.exit_code, Some(127));
    assert_eq!(ended.output, "zsh: command not found: _efr_hs_sbx\n");
}

#[tokio::test]
async fn bad_call_id_ends_without_timeout() {
    let (_harness, ended) = ends_at_once(b"efr: bad call id\r\n", 125).await;
    assert_eq!(ended.completion, Completion::SandboxFailed);
    assert_eq!(ended.exit_code, Some(125));
}

#[tokio::test]
async fn launcher_crash_is_sandbox_failed() {
    let harness = Harness::sandboxed();
    let run = prepare(&harness, SpecLaunch::Contained);
    let (mut terminal, handle) = typed(&harness, &run, "ls").await;
    started(&run);
    terminal.print(b"\r\n\x1b]133;C\x07").await;
    trusted_end(&mut terminal, "/home/u", 139).await;
    let ended = handle.await.unwrap().unwrap();
    assert_eq!(ended.completion, Completion::SandboxFailed);
    assert_eq!(ended.exit_code, Some(139));
    // The launcher had started the command, so the result says it may have run.
    let sandbox = ended.sandbox.unwrap();
    assert!(sandbox.started && sandbox.setup_error.is_none(), "{sandbox:?}");
    assert!(sandbox.launch_error.is_some(), "{sandbox:?}");
}

#[tokio::test]
async fn a_setup_failure_is_sandbox_failed_with_its_result() {
    let harness = Harness::sandboxed();
    let run = prepare(&harness, SpecLaunch::Contained);
    let (mut terminal, handle) = typed(&harness, &run, "ls").await;
    started(&run);
    let failed = SandboxResult {
        started: true,
        setup_error: Some("bwrap: Can't mount proc".to_owned()),
        ..SandboxResult::default()
    };
    result(&run, &failed);
    terminal.print(b"\r\n\x1b]133;C\x07").await;
    trusted_end(&mut terminal, "/home/u", 125).await;
    let ended = handle.await.unwrap().unwrap();
    assert_eq!(ended.completion, Completion::SandboxFailed);
    assert_eq!(ended.exit_code, Some(125));
    assert_eq!(ended.sandbox, Some(failed));
}

#[tokio::test]
async fn the_end_waits_for_the_shell_to_hold_the_terminal() {
    let harness = Harness::sandboxed();
    let run = prepare(&harness, SpecLaunch::Contained);
    let handle = spawn(&harness, sandboxed(&run, "ls").with_timeout(Duration::from_secs(5)));
    let mut terminal = harness.holder.terminal(0).await;
    terminal.prompt().await;
    terminal.typed_line().await;
    harness.holder.set_foreground(terminal.pty_id, Some(SANDBOXED_JOB));
    started(&run);
    result(&run, &finished(0, "/home/u"));
    terminal.print(b"\r\n\x1b]133;C\x07").await;
    trusted_end(&mut terminal, "/home/u", 0).await;
    checked(&harness, 1).await;
    assert!(!handle.is_finished());

    harness.clock.wait_for_sleeps(3).await;
    harness.clock.advance(Duration::from_secs(5));
    let ended = handle.await.unwrap().unwrap();
    assert!(
        matches!(ended.completion, Completion::StillRunning | Completion::Interactive),
        "{ended:?}"
    );
}

#[tokio::test]
async fn the_cwd_comes_from_result_json_and_a_private_tmp_dir_is_kept() {
    let harness = Harness::sandboxed();
    let run = prepare(&harness, SpecLaunch::Contained);
    let (mut terminal, handle) = typed(&harness, &run, "cd /tmp/work").await;
    started(&run);
    result(&run, &finished(0, "/tmp/work"));
    terminal.print(b"\r\n\x1b]133;C\x07").await;
    trusted_end(&mut terminal, "/home/u", 0).await;
    let ended = handle.await.unwrap().unwrap();
    assert_eq!(ended.cwd_after, PathBuf::from("/tmp/work"));
    let state = harness.sessions.state(conversation(1)).await.unwrap();
    assert_eq!(state.cwd, PathBuf::from("/home/u"));
    assert_eq!(state.sandbox_cwd, Some(PathBuf::from("/tmp/work")));
    assert_eq!(state.effective_cwd(), Path::new("/tmp/work"));

    // The user's own `cd` in the shell wins.
    let next = spawn(&harness, RunRequest::new("cd /srv", "/home/u"));
    terminal.typed_line().await;
    terminal.run(b"\x1b]7;kitty-shell-cwd://box/srv\x07", 0).await;
    next.await.unwrap().unwrap();
    let state = harness.sessions.state(conversation(1)).await.unwrap();
    assert_eq!(state.sandbox_cwd, None);
    assert_eq!(state.effective_cwd(), Path::new("/srv"));
}

#[tokio::test]
async fn a_contained_call_never_takes_hidden_input() {
    let harness = Harness::sandboxed();
    let run = prepare(&harness, SpecLaunch::Contained);
    // A client that could answer follows the turn.
    let (mut listener, heard) = listener(true);
    let sessions = harness.sessions.clone();
    let request = sandboxed(&run, "./fake-sudo");
    let handle =
        tokio::spawn(
            async move { sessions.run_command(conversation(1), request, &mut listener).await },
        );
    let mut terminal = harness.holder.terminal(0).await;
    terminal.prompt().await;
    terminal.typed_line().await;
    started(&run);
    terminal.print(b"\r\n\x1b]133;C\x07[sudo] password for u: ").await;
    // The looks start once the session has seen `C`.
    harness.clock.wait_for_sleeps(3).await;

    let secret = SecretText::new("hunter2");
    let refused = harness.sessions.answer(conversation(1), call(), &secret, true).await;
    assert!(
        matches!(refused, Err(ShellError::NotWaiting { reason, .. }) if reason.contains("sandboxed")),
        "{refused:?}"
    );

    harness.modes.set(Some(HIDDEN));
    harness.clock.advance(Duration::from_secs(1));
    let ended = handle.await.unwrap().unwrap();
    assert_eq!(ended.completion, Completion::Unanswered);
    assert_eq!(*heard.inputs.borrow(), [InputWait::Hidden, InputWait::None]);
    assert_eq!(
        harness.holder.signals(),
        [(terminal.pty_id, Signal::Interrupt, SignalTarget::ForegroundGroup)]
    );
}

#[tokio::test]
async fn the_exit_child_of_an_approved_exit_takes_hidden_input() {
    let harness = Harness::sandboxed();
    let run = prepare(&harness, SpecLaunch::Unsandboxed);
    let (mut listener, mut heard) = listener(true);
    let sessions = harness.sessions.clone();
    let request = sandboxed(&run, "sudo pacman -Syu");
    let handle =
        tokio::spawn(
            async move { sessions.run_command(conversation(1), request, &mut listener).await },
        );
    let mut terminal = harness.holder.terminal(0).await;
    terminal.prompt().await;
    terminal.typed_line().await;
    started(&run);
    terminal.print(b"\r\n\x1b]133;C\x07[sudo] password for u: ").await;
    harness.modes.set(Some(HIDDEN));
    harness.clock.wait_for_sleeps(3).await;
    harness.clock.advance(Duration::from_secs(1));
    heard.inputs(&[InputWait::Hidden]).await;

    let secret = SecretText::new("hunter2");
    harness.sessions.answer(conversation(1), call(), &secret, true).await.unwrap();
    assert_eq!(terminal.typed_line().await, b"hunter2\r");
    result(&run, &finished(0, "/home/u"));
    trusted_end(&mut terminal, "/home/u", 0).await;
    assert_eq!(handle.await.unwrap().unwrap().completion, Completion::Finished);
}

#[tokio::test]
async fn the_forget_key_follows_a_sandboxed_run_whose_end_is_checked_at_a_ready_prompt() {
    let harness = Harness::sandboxed();
    let run = prepare(&harness, SpecLaunch::Unsandboxed);
    let handle = spawn(&harness, sandboxed(&run, "sudo true").with_forget_credentials(true));
    let mut terminal = harness.holder.terminal(0).await;
    terminal.prompt().await;
    terminal.typed_line().await;
    started(&run);
    result(&run, &finished(0, "/home/u"));
    terminal.print(b"\r\n\x1b]133;C\x07").await;
    // The end mark, D and the next prompt arrive in one chunk, so the prompt is ready
    // before the facts are read.
    let mut end = end_mark();
    end.extend_from_slice(b"\x1b]133;D;0\x07\x1b]133;A;cl=line\x07% \x1b]133;B\x07");
    terminal.print(&end).await;
    assert_eq!(terminal.typed_until(FORGET_CREDENTIALS).await, FORGET_CREDENTIALS);
    assert_eq!(handle.await.unwrap().unwrap().completion, Completion::Finished);
}

#[tokio::test]
async fn a_detached_sandboxed_run_holds_the_shell_until_its_real_end() {
    let harness = Harness::sandboxed();
    let run = prepare(&harness, SpecLaunch::Contained);
    let handle =
        spawn(&harness, sandboxed(&run, "cargo watch").with_timeout(Duration::from_secs(5)));
    let mut terminal = harness.holder.terminal(0).await;
    terminal.prompt().await;
    terminal.typed_line().await;
    started(&run);
    harness.holder.set_foreground(terminal.pty_id, Some(SANDBOXED_JOB));
    terminal.print(b"\r\n\x1b]133;C\x07watching\r\n").await;
    harness.clock.wait_for_sleeps(3).await;
    harness.clock.advance(Duration::from_secs(5));
    let left = handle.await.unwrap().unwrap();
    assert_eq!(left.completion, Completion::StillRunning);

    // A fake end of the orphan does not free the shell.
    let next = spawn(&harness, RunRequest::new("ls", "/home/u"));
    terminal.print(b"\x1b]133;D;0\x07\x1b]133;A;cl=line\x07% \x1b]133;B\x07").await;
    checked(&harness, 1).await;
    let state = harness.sessions.state(conversation(1)).await.unwrap();
    assert_eq!(state.phase, Phase::Running);

    harness.holder.set_foreground(terminal.pty_id, Some(SHELL));
    result(&run, &finished(130, "/home/u"));
    trusted_end(&mut terminal, "/home/u", 130).await;
    assert_eq!(terminal.typed_line().await, b"\x1b[efr-clear~\x1b[200~ls\x1b[201~\r");
    terminal.run(b"a\r\n", 0).await;
    assert_eq!(next.await.unwrap().unwrap().output, "a\n");
}

#[tokio::test]
async fn a_shell_without_marks_refuses_a_sandboxed_run_and_types_nothing() {
    let harness = Harness::build("/bin/sh", Arc::new(Vt100Screens), |config, dir| {
        config.sandbox_dir = Some(dir.join("sbx"));
        config.sandbox_launcher = Some(dir.join("bin/efr-sbx"));
    });
    let run = prepare(&harness, SpecLaunch::Contained);
    let ended = spawn(&harness, sandboxed(&run, "ls")).await.unwrap().unwrap();
    assert_eq!(ended.completion, Completion::SandboxFailed);
    let terminal = harness.holder.terminal(0).await;
    assert_eq!(harness.recorded.stream(terminal.pty_id), b"");
}

#[tokio::test]
async fn a_sandboxed_run_is_never_typed_as_sentinels() {
    let harness = Harness::sandboxed();
    let run = prepare(&harness, SpecLaunch::Contained);
    let handle = spawn(&harness, sandboxed(&run, "ls").with_mode(RunMode::Sentinel));
    let mut terminal = harness.holder.terminal(0).await;
    terminal.prompt().await;
    let ended = handle.await.unwrap().unwrap();
    assert_eq!(ended.completion, Completion::SandboxFailed);
}

#[tokio::test]
async fn a_sandboxed_run_needs_the_shells_sandbox_dir_and_a_fresh_line_file() {
    let plain = Harness::new("/usr/bin/zsh");
    let run = prepare(&plain, SpecLaunch::Contained);
    let refused = spawn(&plain, sandboxed(&run, "ls")).await.unwrap();
    assert!(matches!(refused, Err(ShellError::Sandbox { .. })), "{refused:?}");

    let harness = Harness::sandboxed();
    let mut elsewhere = prepare(&harness, SpecLaunch::Contained);
    elsewhere.dir = harness.dir.path().join("elsewhere");
    let refused = spawn(&harness, sandboxed(&elsewhere, "ls")).await.unwrap();
    assert!(matches!(refused, Err(ShellError::Sandbox { .. })), "{refused:?}");

    let run = prepare(&harness, SpecLaunch::Contained);
    std::fs::write(run.dir.join("line"), "planted").unwrap();
    let refused = spawn(&harness, sandboxed(&run, "ls")).await.unwrap();
    assert!(matches!(refused, Err(ShellError::SandboxFile { .. })), "{refused:?}");
    assert_eq!(std::fs::read_to_string(run.dir.join("line")).unwrap(), "planted");
}

#[test]
fn debug_hides_the_nonce() {
    let run = SandboxRun::new("/run/efr/sbx/c/x", call(), NONCE, SpecLaunch::Contained);
    let debug = format!("{run:?}");
    assert!(debug.contains("[16 bytes]"), "{debug}");
    assert!(!debug.contains("165"), "{debug}");
    assert!(run.contained());
    assert!(!SandboxRun { launch: SpecLaunch::Unsandboxed, ..run }.contained());
}

#[tokio::test]
async fn ctrl_z_never_reaches_a_sandboxed_call() {
    let harness = Harness::sandboxed();
    let run = prepare(&harness, SpecLaunch::Contained);
    let (mut terminal, handle) = typed(&harness, &run, "sleep 100").await;
    started(&run);
    terminal.print(b"\r\n\x1b]133;C\x07").await;
    // An attached client types Ctrl+Z and a line: the stop never goes out.
    let write = harness.sessions.write(conversation(1), bytes::Bytes::from_static(b"\x1aab\r"));
    write.await.unwrap();
    assert_eq!(terminal.typed_line().await, b"ab\r");
    trusted_end(&mut terminal, "/home/u", 130).await;
    handle.await.unwrap().unwrap();

    // With no sandboxed call, the key goes through.
    let write = harness.sessions.write(conversation(1), bytes::Bytes::from_static(b"\x1a\r"));
    write.await.unwrap();
    assert_eq!(terminal.typed_line().await, b"\x1a\r");
}

#[test]
fn the_wrapper_times_read_as_steps_until_a_word_that_is_not_one() {
    let text = "snapshot 0.0012 launcher 0.25 apply 1e-05\n";
    assert_eq!(
        super::wrapper_times(text),
        [("snapshot", 0.0012), ("launcher", 0.25), ("apply", 0.000_01)]
    );
    assert_eq!(super::wrapper_times("snapshot 0.1 evil 3 apply 0.2"), [("snapshot", 0.1)]);
    assert_eq!(super::wrapper_times("launcher -1 apply 0.2"), []);
    assert_eq!(super::wrapper_times("launcher NaN"), []);
    assert_eq!(super::wrapper_times(""), []);
}

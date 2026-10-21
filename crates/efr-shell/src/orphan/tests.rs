//! The watch of a command left running, over the fake holder: the test plays the shell
//! on the other end of a socketpair, sets the terminal's modes and the followers, and
//! moves a manual clock, so nothing waits on real time.

use std::path::PathBuf;
use std::time::Duration;

use efr_holder::{Signal, SignalTarget};
use efr_protocol::{ApprovedBy, CallId, InputWait, SecretText, ShellCommandEnd};
use efr_sandbox::{SandboxResult, SpecLaunch, nonce_hex};
use efr_stdx::time::Clock as _;
use efr_test_support::{TestClock, Wait};
use pretty_assertions::assert_eq;

use crate::testing::{FakeTerminal, Harness, JOB, OTHER_JOB, conversation};
use crate::{Completion, InputModes, NoProgress, RunRequest, SandboxRun, ShellError, ShellNotice};

const ZSH: &str = "/usr/bin/zsh";

/// A getpass-style read: echo off, line mode on.
const HIDDEN: InputModes = InputModes::new(false, true);

/// A terminal that a program put in raw mode with echo off, as a relay does.
const RAW: InputModes = InputModes::new(false, false);

const NONCE: [u8; 16] = [0x3c; 16];

fn call() -> CallId {
    "01920000-0000-7000-8000-0000000c0de1".parse().unwrap()
}

fn request(command: &str) -> RunRequest {
    RunRequest::new(command, "/home/u")
}

/// Starts `request` for [`call`] with a timeout of five seconds, plays a ready prompt,
/// starts the command with `output` and lets the timeout pass: the command is left
/// running, and its watch looks from then on.
async fn left(harness: &Harness, request: RunRequest, output: &[u8]) -> FakeTerminal {
    let sessions = harness.sessions.clone();
    let request = request.with_call(call()).with_timeout(Duration::from_secs(5));
    let run = tokio::spawn(async move {
        sessions.run_command(conversation(1), request, &mut NoProgress).await
    });
    let mut terminal = harness.holder.terminal(0).await;
    terminal.prompt().await;
    terminal.typed_line().await;
    let mut started = b"\r\n\x1b]133;C\x07".to_vec();
    started.extend_from_slice(output);
    terminal.print(&started).await;
    // The deadline, the run's look and the shell's startup timer.
    for _ in 0..5 {
        harness.clock.wait_for_sleeps(3).await;
        harness.clock.advance(Duration::from_secs(1));
    }
    let result = run.await.unwrap().unwrap();
    assert!(
        matches!(result.completion, Completion::StillRunning | Completion::Interactive),
        "{result:?}"
    );
    terminal
}

/// Lets `n` looks of the watch pass: each once the watch sleeps until its next look
/// (beside the shell's startup timer for the first ten seconds).
async fn looks(harness: &Harness, n: usize) {
    for _ in 0..n {
        let sleeps = if seconds(harness) < 10 { 2 } else { 1 };
        harness.clock.wait_for_sleeps(sleeps).await;
        harness.clock.advance(Duration::from_secs(1));
    }
    // The last look is over once the watch sleeps again.
    let sleeps = if seconds(harness) < 10 { 2 } else { 1 };
    harness.clock.wait_for_sleeps(sleeps).await;
}

/// The seconds since the test's start.
fn seconds(harness: &Harness) -> u64 {
    harness.clock.now().duration_since(TestClock::START).as_secs().unsigned_abs()
}

/// Plays a password prompt of the command left running, with the terminal in `modes`.
async fn asks(harness: &Harness, terminal: &mut FakeTerminal, prompt: &[u8], modes: InputModes) {
    terminal.print(prompt).await;
    harness.modes.set(Some(modes));
    let text = String::from_utf8_lossy(prompt).trim_end().to_owned();
    let screen = harness.sessions.screen(conversation(1)).unwrap();
    Wait::new(&format!("{text:?} on the screen"))
        .until_some_async(async || {
            let capture = screen.snapshot(0).await.unwrap();
            capture
                .snapshot
                .rows
                .iter()
                .any(|row| efr_screen::row_text(row).contains(&text))
                .then_some(())
        })
        .await
        .unwrap();
}

fn interrupted(harness: &Harness, terminal: &FakeTerminal) -> bool {
    harness.holder.signals().contains(&(
        terminal.pty_id,
        Signal::Interrupt,
        SignalTarget::ForegroundGroup,
    ))
}

async fn wait_interrupted(harness: &Harness, terminal: &FakeTerminal) {
    Wait::new("the interrupt of the command left running")
        .until(|| interrupted(harness, terminal))
        .await
        .unwrap();
}

/// Waits until the notice of the command's end came, and returns it.
async fn ended(harness: &Harness) -> ShellNotice {
    harness.notices.wait_for(|notice| matches!(notice, ShellNotice::CommandEnded { .. })).await;
    harness
        .notices
        .all()
        .into_iter()
        .find(|notice| matches!(notice, ShellNotice::CommandEnded { .. }))
        .unwrap()
}

#[tokio::test]
async fn orphan_reports_a_hidden_wait_with_its_prompt_row() {
    let harness = Harness::new(ZSH);
    harness.followers.set(1);
    let command = "rsync -a src/ u@h:dst/";
    let mut terminal = left(&harness, request(command), b"sending file list\r\n").await;
    harness.notices.wait_for(|notice| matches!(notice, ShellNotice::CommandDetached { .. })).await;
    let detached = harness
        .notices
        .all()
        .into_iter()
        .find(|notice| matches!(notice, ShellNotice::CommandDetached { .. }));
    assert_eq!(
        detached,
        Some(ShellNotice::CommandDetached {
            conversation: conversation(1),
            call: Some(call()),
            command: command.to_owned(),
            since: TestClock::START,
            contained: false,
            input: InputWait::None,
            looks_secret: false,
            prompt: String::new(),
            approved_by: None,
        })
    );

    asks(&harness, &mut terminal, b"u@h's password: ", HIDDEN).await;
    looks(&harness, 1).await;
    let changed = ShellNotice::CommandInputChanged {
        conversation: conversation(1),
        call: Some(call()),
        input: InputWait::Hidden,
        contained: false,
        looks_secret: false,
        prompt: "u@h's password:".to_owned(),
    };
    harness.notices.wait_for(|notice| *notice == changed).await;
    let busy = harness.sessions.busy(conversation(1)).unwrap();
    assert_eq!(busy.input, InputWait::Hidden);
    assert_eq!(busy.prompt, "u@h's password:");
    assert!(!interrupted(&harness, &terminal), "a client that can answer follows");
}

#[tokio::test]
async fn orphan_reports_the_end_with_exit_code() {
    let harness = Harness::new(ZSH);
    let mut terminal = left(&harness, request("make"), b"cc -c a.c\r\n").await;
    terminal.print(b"\x1b]133;D;3\x07").await;
    assert_eq!(
        ended(&harness).await,
        ShellNotice::CommandEnded {
            conversation: conversation(1),
            call: Some(call()),
            exit_code: Some(3),
            how: ShellCommandEnd::Ended,
        }
    );
    let sessions = harness.sessions.clone();
    Wait::new("a free shell").until(|| sessions.busy(conversation(1)).is_none()).await.unwrap();
}

/// A contained call of `harness`, prepared as efrd does.
fn contained(harness: &Harness) -> SandboxRun {
    let dir =
        harness.dir.path().join("sbx").join(conversation(1).to_string()).join(call().to_string());
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("nonce"), nonce_hex(&NONCE)).unwrap();
    SandboxRun::new(dir, call(), NONCE, SpecLaunch::Contained)
}

/// The trusted shell's end of a sandboxed call after the launcher wrote `started` and a
/// `result.json` with `exit_code`: the end mark, `D` and the next prompt.
async fn sandboxed_end(terminal: &mut FakeTerminal, run: &SandboxRun, exit_code: i32) {
    let result = SandboxResult {
        started: true,
        exit_code: Some(exit_code),
        cwd: Some(PathBuf::from("/home/u")),
        state_kept: true,
        ..SandboxResult::default()
    };
    std::fs::write(run.dir.join("result.json"), result.to_json().unwrap()).unwrap();
    let mut bytes = format!("\x1b]133;efr-sbx;{}\x07", nonce_hex(&NONCE)).into_bytes();
    bytes.extend_from_slice(format!("\x1b]133;D;{exit_code}\x07").as_bytes());
    terminal.print(&bytes).await;
    terminal.prompt().await;
}

/// A contained call left running at its timeout, which then asks `prompt` with the
/// terminal in `modes` while a client that could answer follows: the watch stops it.
async fn contained_wait_is_stopped(prompt: &[u8], modes: InputModes) {
    let harness = Harness::sandboxed();
    harness.followers.set(1);
    let run = contained(&harness);
    std::fs::write(run.dir.join("started"), "").unwrap();
    let request = request("./fake-login").with_sandbox(Some(run.clone()));
    let mut terminal = left(&harness, request, b"connecting\r\n").await;
    asks(&harness, &mut terminal, prompt, modes).await;
    looks(&harness, 1).await;
    wait_interrupted(&harness, &terminal).await;
    sandboxed_end(&mut terminal, &run, 130).await;
    let ShellNotice::CommandEnded { how, exit_code, .. } = ended(&harness).await else {
        unreachable!("the notice is an end");
    };
    assert_eq!(how, ShellCommandEnd::Limit);
    assert_eq!(exit_code, Some(130));
}

#[tokio::test]
async fn contained_orphan_hidden_wait_is_stopped() {
    contained_wait_is_stopped(b"[sudo] password for u: ", HIDDEN).await;
}

#[tokio::test]
async fn contained_orphan_looks_secret_wait_is_stopped() {
    contained_wait_is_stopped(b"Password: ", RAW).await;
}

#[tokio::test]
async fn contained_orphan_password_prompt_in_line_mode_is_stopped() {
    // NOTE: the prompt alone counts for a contained command, in any modes (DS50).
    contained_wait_is_stopped(b"Password: ", InputModes::new(true, true)).await;
}

/// A command left running whose request is `request`, at a hidden password prompt,
/// while `followers` clients that can answer follow; after five looks, tells whether
/// the watch interrupted it.
async fn hidden_wait_interrupted(request: RunRequest, followers: usize) -> bool {
    let harness = Harness::new(ZSH);
    harness.followers.set(followers);
    let mut terminal = left(&harness, request, b"connecting\r\n").await;
    asks(&harness, &mut terminal, b"[sudo] password for u: ", HIDDEN).await;
    looks(&harness, 1).await;
    let busy = harness.sessions.busy(conversation(1)).unwrap();
    assert_eq!(busy.input, InputWait::Hidden);
    if interrupted(&harness, &terminal) {
        return true;
    }
    looks(&harness, 4).await;
    interrupted(&harness, &terminal)
}

#[tokio::test]
async fn orphan_wait_is_kept_while_followed() {
    assert!(!hidden_wait_interrupted(request("sudo pacman -Syu"), 1).await);
}

#[tokio::test]
async fn flagged_orphan_approved_by_you_keeps_wait_unfollowed() {
    for by in [ApprovedBy::You, ApprovedBy::YourRule] {
        let request = request("sudo pacman -Syu").with_approval(true, Some(by));
        assert!(!hidden_wait_interrupted(request, 0).await, "{by:?}");
    }
}

#[tokio::test]
async fn reviewer_approved_orphan_wait_stops_unfollowed() {
    let request = request("sudo pacman -Syu").with_approval(true, Some(ApprovedBy::Reviewer));
    assert!(hidden_wait_interrupted(request, 0).await);
}

#[tokio::test]
async fn unflagged_orphan_wait_stops_unfollowed() {
    let request = request("./deploy").with_approval(false, Some(ApprovedBy::You));
    assert!(hidden_wait_interrupted(request, 0).await);
    assert!(hidden_wait_interrupted(self::request("./deploy"), 0).await, "no approval");
}

#[tokio::test]
async fn full_mode_orphan_wait_stops_unfollowed() {
    let request = request("sudo pacman -Syu").with_approval(true, Some(ApprovedBy::FullMode));
    assert!(hidden_wait_interrupted(request, 0).await);
}

/// A command left running whose wait is kept (a client that can answer follows), with
/// an input limit of twelve seconds from its start; it asks at second five.
async fn waits_until_the_limit(harness: &Harness) -> FakeTerminal {
    harness.followers.set(1);
    let request = request("sudo pacman -Syu").with_input_limit(Some(Duration::from_secs(12)));
    let mut terminal = left(harness, request, b"connecting\r\n").await;
    asks(harness, &mut terminal, b"[sudo] password for u: ", HIDDEN).await;
    // The looks at seconds six to eleven keep the wait.
    looks(harness, 6).await;
    assert!(!interrupted(harness, &terminal), "the limit has not passed");
    // The look at second twelve stops it.
    harness.clock.wait_for_sleeps(1).await;
    harness.clock.advance(Duration::from_secs(1));
    wait_interrupted(harness, &terminal).await;
    terminal
}

#[tokio::test]
async fn orphan_limit_interrupts_a_waiting_orphan() {
    let harness = Harness::new(ZSH);
    let mut terminal = waits_until_the_limit(&harness).await;
    terminal.print(b"^C\r\n\x1b]133;D;130\x07").await;
    let ShellNotice::CommandEnded { how, exit_code, .. } = ended(&harness).await else {
        unreachable!("the notice is an end");
    };
    assert_eq!((how, exit_code), (ShellCommandEnd::Limit, Some(130)));
    assert!(
        !harness.holder.signals().iter().any(|(_, signal, _)| *signal == Signal::Hangup),
        "the command ended within the grace: the shell stays"
    );
}

#[tokio::test]
async fn orphan_limit_closes_a_shell_that_ignores_sigint() {
    let harness = Harness::new(ZSH);
    let terminal = waits_until_the_limit(&harness).await;
    // The command ignores SIGINT: the grace passes.
    harness.clock.wait_for_sleeps(1).await;
    harness.clock.advance(Duration::from_secs(5));
    let ShellNotice::CommandEnded { how, exit_code, .. } = ended(&harness).await else {
        unreachable!("the notice is an end");
    };
    assert_eq!((how, exit_code), (ShellCommandEnd::Limit, None));
    assert!(harness.holder.signals().contains(&(
        terminal.pty_id,
        Signal::Hangup,
        SignalTarget::Child
    )));
    harness.notices.wait_for(|notice| matches!(notice, ShellNotice::Exited { .. })).await;
}

#[tokio::test]
async fn orphan_that_does_not_wait_has_no_limit() {
    let harness = Harness::new(ZSH);
    let request = request("cargo build").with_input_limit(Some(Duration::from_secs(8)));
    let terminal = left(&harness, request, b"Compiling\r\n").await;
    looks(&harness, 10).await;
    assert!(harness.holder.signals().is_empty(), "{:?}", harness.holder.signals());
    assert!(harness.sessions.busy(conversation(1)).is_some());
    drop(terminal);
}

/// A command left running at a hidden password prompt that a client can answer.
async fn left_at_a_password_prompt(harness: &Harness) -> FakeTerminal {
    harness.followers.set(1);
    let mut terminal = left(harness, request("sudo pacman -Syu"), b"connecting\r\n").await;
    asks(harness, &mut terminal, b"[sudo] password for u: ", HIDDEN).await;
    looks(harness, 1).await;
    let hidden = ShellNotice::CommandInputChanged {
        conversation: conversation(1),
        call: Some(call()),
        input: InputWait::Hidden,
        contained: false,
        looks_secret: false,
        prompt: "[sudo] password for u:".to_owned(),
    };
    harness.notices.wait_for(|notice| *notice == hidden).await;
    terminal
}

fn other_call() -> CallId {
    "01920000-0000-7000-8000-0000000c0de2".parse().unwrap()
}

#[tokio::test]
async fn answer_reaches_the_orphan_of_its_call() {
    let harness = Harness::new(ZSH);
    let mut terminal = left_at_a_password_prompt(&harness).await;
    harness
        .sessions
        .answer(conversation(1), call(), &SecretText::new("hunter2"), true)
        .await
        .unwrap();
    assert_eq!(terminal.typed_line().await, b"hunter2\r");
    // The look after an answer says that the prompt is gone, so a prompt asked again
    // is a new change.
    looks(&harness, 1).await;
    let ended = ShellNotice::CommandInputChanged {
        conversation: conversation(1),
        call: Some(call()),
        input: InputWait::None,
        contained: false,
        looks_secret: false,
        prompt: String::new(),
    };
    harness.notices.wait_for(|notice| *notice == ended).await;
    terminal.print(b"\r\n\x1b]133;D;0\x07").await;
    let ShellNotice::CommandEnded { how, exit_code, .. } = self::ended(&harness).await else {
        unreachable!("the notice is an end");
    };
    assert_eq!((how, exit_code), (ShellCommandEnd::Ended, Some(0)));
}

#[tokio::test]
async fn answer_for_another_call_is_refused() {
    let harness = Harness::new(ZSH);
    let mut terminal = left_at_a_password_prompt(&harness).await;
    let text = SecretText::new("hunter2");
    let other = harness.sessions.answer(conversation(1), other_call(), &text, true).await;
    assert!(matches!(other, Err(ShellError::NoCall { .. })), "{other:?}");
    // A manual answer goes only to a call that runs.
    let manual = harness.sessions.answer_manual(conversation(1), call(), &text, true).await;
    assert!(matches!(manual, Err(ShellError::NotWaiting { .. })), "{manual:?}");
    harness.sessions.answer(conversation(1), call(), &SecretText::new("ok"), true).await.unwrap();
    // Only the accepted answer was typed.
    assert_eq!(terminal.typed_line().await, b"ok\r");
}

#[tokio::test]
async fn hidden_answer_needs_echo_off_for_orphan() {
    let harness = Harness::new(ZSH);
    let mut terminal = left_at_a_password_prompt(&harness).await;
    // The program turned echo back on, as sudo does when its own timeout passes.
    harness.modes.set(Some(InputModes::new(true, true)));
    let text = SecretText::new("hunter2");
    let echoed = harness.sessions.answer(conversation(1), call(), &text, true).await;
    assert!(matches!(echoed, Err(ShellError::NotWaiting { .. })), "{echoed:?}");
    let visible = harness.sessions.answer(conversation(1), call(), &text, false).await;
    assert!(matches!(visible, Err(ShellError::NotWaiting { .. })), "a visible answer: {visible:?}");
    harness.modes.set(Some(HIDDEN));
    harness.sessions.answer(conversation(1), call(), &SecretText::new("ok"), true).await.unwrap();
    assert_eq!(terminal.typed_line().await, b"ok\r");
}

#[tokio::test]
async fn interrupt_left_only_while_group_holds_terminal() {
    let harness = Harness::new(ZSH);
    let mut terminal = left_at_a_password_prompt(&harness).await;
    // The job ended and a command that a later precmd hook started holds the terminal.
    harness.modes.set_foreground(OTHER_JOB);
    let refused = harness.sessions.interrupt_left(conversation(1), call(), false).await;
    assert!(matches!(refused, Err(ShellError::NotWaiting { .. })), "{refused:?}");
    assert!(!interrupted(&harness, &terminal));

    harness.modes.set_foreground(JOB);
    harness.sessions.interrupt_left(conversation(1), call(), false).await.unwrap();
    assert!(interrupted(&harness, &terminal));
    terminal.print(b"^C\r\n\x1b]133;D;130\x07").await;
    let ShellNotice::CommandEnded { how, exit_code, .. } = ended(&harness).await else {
        unreachable!("the notice is an end");
    };
    assert_eq!((how, exit_code), (ShellCommandEnd::Stopped, Some(130)));
}

#[tokio::test]
async fn interrupt_left_with_no_known_group_never_signals_the_zsh() {
    let harness = Harness::new(ZSH);
    // The shell itself holds the terminal at every look of the run, so no look saw a
    // job of the command.
    let shell = 1000;
    harness.modes.set_foreground(shell);
    let terminal = left(&harness, request("./deploy"), b"deploying\r\n").await;
    let refused = harness.sessions.interrupt_left(conversation(1), call(), false).await;
    assert!(
        matches!(refused, Err(ShellError::NotWaiting { reason, .. }) if reason.contains("shell itself")),
        "{refused:?}"
    );
    assert!(harness.holder.signals().is_empty(), "{:?}", harness.holder.signals());

    // A job that is not the shell's own holds the terminal now.
    harness.modes.set_foreground(JOB);
    harness.sessions.interrupt_left(conversation(1), call(), false).await.unwrap();
    assert!(interrupted(&harness, &terminal));
}

#[tokio::test]
async fn interrupt_left_close_ends_the_shell() {
    let harness = Harness::new(ZSH);
    let terminal = left_at_a_password_prompt(&harness).await;
    harness.sessions.interrupt_left(conversation(1), call(), true).await.unwrap();
    assert!(harness.holder.signals().contains(&(
        terminal.pty_id,
        Signal::Hangup,
        SignalTarget::Child
    )));
    let ShellNotice::CommandEnded { how, exit_code, .. } = ended(&harness).await else {
        unreachable!("the notice is an end");
    };
    assert_eq!((how, exit_code), (ShellCommandEnd::Closed, None));
    harness.notices.wait_for(|notice| matches!(notice, ShellNotice::Exited { .. })).await;
    assert_eq!(harness.sessions.busy(conversation(1)), None);
}

#[tokio::test]
async fn interrupt_left_after_end_is_no_call() {
    let harness = Harness::new(ZSH);
    let none = harness.sessions.interrupt_left(conversation(1), call(), false).await;
    assert!(matches!(none, Err(ShellError::NoShell { .. })), "{none:?}");
    let mut terminal = left(&harness, request("make"), b"cc -c a.c\r\n").await;
    terminal.print(b"\x1b]133;D;0\x07").await;
    ended(&harness).await;
    let sessions = harness.sessions.clone();
    Wait::new("a free shell").until(|| sessions.busy(conversation(1)).is_none()).await.unwrap();
    let late = harness.sessions.interrupt_left(conversation(1), call(), false).await;
    assert!(matches!(late, Err(ShellError::NoCall { .. })), "{late:?}");
    let late = harness.sessions.interrupt_left(conversation(1), call(), true).await;
    assert!(matches!(late, Err(ShellError::NoCall { .. })), "{late:?}");
    assert!(harness.holder.signals().is_empty(), "{:?}", harness.holder.signals());
}

#[tokio::test]
async fn until_free_fails_at_once_when_orphan_waits() {
    let harness = Harness::new(ZSH);
    left_at_a_password_prompt(&harness).await;
    let held = harness.sessions.until_free(conversation(1), Duration::from_secs(30)).await;
    let Err(ShellError::HeldBy { command, waiting, .. }) = held else {
        panic!("{held:?}");
    };
    assert_eq!(command, "sudo pacman -Syu");
    assert_eq!(waiting, InputWait::Hidden);
    let message = ShellError::HeldBy { conversation: conversation(1), command, waiting };
    assert!(!message.to_string().contains("pacman"), "the message names no command");
}

#[tokio::test]
async fn until_free_still_waits_for_a_running_orphan() {
    let harness = Harness::new(ZSH);
    let mut terminal = left(&harness, request("cargo build"), b"Compiling\r\n").await;
    let sessions = harness.sessions.clone();
    let wait =
        tokio::spawn(
            async move { sessions.until_free(conversation(1), Duration::from_secs(30)).await },
        );
    // The watch and the wait sleep, beside the shell's startup timer.
    harness.clock.wait_for_sleeps(3).await;
    assert!(!wait.is_finished());
    terminal.print(b"\x1b]133;D;0\x07").await;
    wait.await.unwrap().unwrap();
}

#[tokio::test]
async fn queued_run_gets_held_by() {
    let harness = Harness::new(ZSH);
    harness.followers.set(1);
    let mut terminal = left(&harness, request("rsync -a src/ u@h:dst/"), b"sending\r\n").await;
    // A run that comes while the command runs waits for its end.
    let sessions = harness.sessions.clone();
    let queued = tokio::spawn(async move {
        sessions.run_command(conversation(1), request("ls"), &mut NoProgress).await
    });
    // The watch, the queued run's deadline and the shell's startup timer.
    harness.clock.wait_for_sleeps(3).await;
    assert!(!queued.is_finished());
    // The command asks for a password: the queued run would wait for the user.
    asks(&harness, &mut terminal, b"u@h's password: ", HIDDEN).await;
    harness.clock.advance(Duration::from_secs(1));
    let held = queued.await.unwrap();
    assert!(
        matches!(&held, Err(ShellError::HeldBy { waiting: InputWait::Hidden, .. })),
        "{held:?}"
    );
    // A run that comes now fails at once, and nothing was typed into the prompt.
    let now = harness.sessions.run_command(conversation(1), request("ls"), &mut NoProgress).await;
    assert!(matches!(&now, Err(ShellError::HeldBy { .. })), "{now:?}");
    harness.sessions.answer(conversation(1), call(), &SecretText::new("pw"), true).await.unwrap();
    assert_eq!(terminal.typed_line().await, b"pw\r");
}

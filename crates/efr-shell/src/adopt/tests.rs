//! The move of a running call into a background job, over the fake holder: the test
//! plays the shell on the other end of a socketpair, sets the terminal's modes and moves
//! a manual clock, so nothing waits on real time.

use std::io::Read as _;
use std::time::Duration;

use efr_holder::{ChildStatus, Signal, SignalTarget};
use efr_protocol::{CallId, InputWait, SecretText};
use efr_test_support::Wait;
use pretty_assertions::assert_eq;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use super::TEE_CAPACITY;
use crate::testing::{FakeTerminal, Harness, Heard, listener};
use crate::{
    AdoptedRun, CommandResult, Completion, InputModes, RunRequest, ShellError, ShellNotice,
    TeeReader,
};

const ZSH: &str = "/usr/bin/zsh";

/// A getpass-style read: echo off, line mode on.
const HIDDEN: InputModes = InputModes::new(false, true);

fn conversation() -> efr_protocol::ConversationId {
    crate::testing::conversation(1)
}

fn call() -> CallId {
    "01920000-0000-7000-8000-0000000c0de2".parse().unwrap()
}

/// A run of `command` for [`call`] with a timeout that no test reaches.
fn request(command: &str) -> RunRequest {
    RunRequest::new(command, "/home/u").with_call(call()).with_timeout(Duration::from_secs(600))
}

type Run = JoinHandle<Result<CommandResult, ShellError>>;

/// Starts `request` on the `index`-th shell, plays a ready prompt (after `before`),
/// starts the command with `output`, and waits until the run heard `heard_text` of it:
/// then the session holds that output and the run looks for input.
async fn running_on(
    harness: &Harness,
    index: usize,
    request: RunRequest,
    before: &[u8],
    output: &[u8],
    heard_text: &str,
) -> (FakeTerminal, Run, Heard) {
    let (mut listener, mut heard) = listener(true);
    let sessions = harness.sessions.clone();
    let run =
        tokio::spawn(
            async move { sessions.run_command(conversation(), request, &mut listener).await },
        );
    let mut terminal = harness.holder.terminal(index).await;
    terminal.print(before).await;
    terminal.prompt().await;
    terminal.typed_line().await;
    let mut started = b"\r\n\x1b]133;C\x07".to_vec();
    started.extend_from_slice(output);
    terminal.print(&started).await;
    heard.output(heard_text).await;
    (terminal, run, heard)
}

async fn running(
    harness: &Harness,
    command: &str,
    output: &[u8],
    heard: &str,
) -> (FakeTerminal, Run) {
    let (terminal, run, _) = running_on(harness, 0, request(command), b"", output, heard).await;
    (terminal, run)
}

/// Asks to move [`call`] and returns what the job gets.
async fn move_call(harness: &Harness) -> AdoptedRun {
    let (to, adopted) = oneshot::channel();
    harness.sessions.request_move(conversation(), call(), to).await.unwrap();
    adopted.await.unwrap()
}

/// Asks to move [`call`] and returns why it cannot.
async fn refused(harness: &Harness) -> &'static str {
    let (to, adopted) = oneshot::channel();
    let error = harness.sessions.request_move(conversation(), call(), to).await.unwrap_err();
    assert!(adopted.await.is_err(), "a refused move hands nothing over");
    match error {
        ShellError::NotMovable { reason, .. } => reason,
        other => panic!("not a refused move: {other:?}"),
    }
}

/// Reads the output after the move to its end, on a thread of its own as a job does.
async fn read_all(mut reader: TeeReader) -> String {
    tokio::task::spawn_blocking(move || {
        let mut text = String::new();
        reader.read_to_string(&mut text).unwrap();
        text
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn move_answers_the_call_with_moved_and_output_so_far() {
    let harness = Harness::new(ZSH);
    let (terminal, run) =
        running(&harness, "make all", b"cc -c a.c\r\ncc -c b.c\r\n", "cc -c b.c").await;
    let adopted = move_call(&harness).await;
    // The daemon learns at the hand-over that the PTY is the job's (DS56).
    let handed = ShellNotice::Adopted { conversation: conversation(), pty_id: terminal.pty_id };
    assert!(harness.notices.all().contains(&handed), "{:?}", harness.notices.all());

    let result = run.await.unwrap().unwrap();
    assert_eq!(result.completion, Completion::Moved);
    assert_eq!(result.exit_code, None);
    assert_eq!(result.output, "cc -c a.c\ncc -c b.c\n");
    assert_eq!(adopted.head(), result.output);
    assert_eq!(adopted.call_dir(), None);
    assert_eq!(adopted.shell_pid(), 1000);
    // The conversation's slot let go of the shell: nothing of the conversation reaches it.
    let sessions = &harness.sessions;
    assert!(matches!(sessions.state(conversation()).await, Err(ShellError::NoShell { .. })));
    assert!(sessions.busy(conversation()).is_none());
    assert!(matches!(
        sessions.answer(conversation(), call(), &SecretText::new("y"), false).await,
        Err(ShellError::NoShell { .. })
    ));
    assert!(sessions.screen(conversation()).is_none());
    // A second request finds no run to move.
    assert_eq!(refused(&harness).await, "the call does not run");
}

#[tokio::test]
async fn moved_command_keeps_running_and_its_end_reaches_the_adopter() {
    let harness = Harness::new(ZSH);
    let (mut terminal, run) = running(&harness, "make all", b"cc -c a.c\r\n", "cc -c a.c").await;
    let mut adopted = move_call(&harness).await;
    assert_eq!(run.await.unwrap().unwrap().completion, Completion::Moved);
    let output = adopted.take_output().unwrap();
    assert!(adopted.take_output().is_none());

    terminal.print(b"cc -c b.c\r\n\x1b[1;31merror\x1b[0m: none\r\n10%\r100%\r\nlinked").await;
    terminal.print(b"\r\n\x1b]133;D;7\x07").await;
    terminal.prompt().await;
    let end = adopted.ended().await.unwrap();
    assert_eq!((end.exit_code, end.sandbox), (Some(7), None));
    assert_eq!(read_all(output).await, "cc -c b.c\nerror: none\n100%\nlinked\n");
    // The end was told once.
    assert!(matches!(adopted.ended().await, Err(ShellError::Exited { .. })));
    // No notice says that the conversation's command or shell changed.
    assert!(
        harness.notices.all().iter().all(|notice| matches!(
            notice,
            ShellNotice::Started { .. } | ShellNotice::Adopted { .. }
        ))
    );
}

#[tokio::test]
async fn moved_sentinel_or_queued_run_is_refused() {
    // A run that waits for the shell's prompt was never typed.
    let harness = Harness::new(ZSH);
    let sessions = harness.sessions.clone();
    let queued = tokio::spawn(async move {
        sessions.run_command(conversation(), request("make"), &mut crate::NoProgress).await
    });
    let mut terminal = harness.holder.terminal(0).await;
    // A prompt that starts but never takes input: the run waits for it.
    terminal.print(b"\x1b]133;A\x07% ").await;
    let reason = Wait::new("a registered run")
        .until_some_async(async || {
            let (to, _) = oneshot::channel();
            match harness.sessions.request_move(conversation(), call(), to).await {
                Err(ShellError::NotMovable { reason, .. }) if reason != "the call does not run" => {
                    Some(reason)
                }
                _ => None,
            }
        })
        .await
        .unwrap();
    assert_eq!(reason, "the call has not started");
    queued.abort();

    // A sentinel run: the shell gives no marks for its command.
    let harness = Harness::new("/bin/bash");
    let (mut listener, mut heard) = listener(true);
    let sessions = harness.sessions.clone();
    let run = tokio::spawn(async move {
        sessions.run_command(conversation(), request("sleep 100"), &mut listener).await
    });
    let mut terminal = harness.holder.terminal(0).await;
    let line = terminal.typed_line().await;
    let line = String::from_utf8(line).unwrap();
    let token = line.split("_b\\n' ").nth(1).unwrap().split(';').next().unwrap().to_owned();
    // The run holds back as much output as an end marker takes, so more follows.
    let more = "z".repeat(80);
    terminal.print(format!("__efr_{token}_b\r\nzzz\r\n{more}\r\n").as_bytes()).await;
    heard.output("zzz").await;
    assert_eq!(
        refused(&harness).await,
        "the shell gives no marks for this command, so it cannot move"
    );
    run.abort();

    // A line that starts a shell, whose prompt reads command lines.
    let harness = Harness::new(ZSH);
    let (_terminal, run) = running(&harness, "bash", b"bash-5.2$ ", "bash-5.2$").await;
    assert_eq!(refused(&harness).await, "a shell or a REPL cannot move");
    // The run goes on.
    assert!(!run.is_finished());
    run.abort();
}

#[tokio::test]
async fn move_of_an_ended_run_fails() {
    let harness = Harness::new(ZSH);
    let (mut terminal, run) = running(&harness, "make", b"done\r\n", "done").await;
    terminal.print(b"\x1b]133;D;0\x07").await;
    terminal.prompt().await;
    assert_eq!(run.await.unwrap().unwrap().completion, Completion::Finished);
    assert_eq!(refused(&harness).await, "the call does not run");
    // Another conversation's request never reaches this call.
    let (to, _) = oneshot::channel();
    let other = crate::testing::conversation(2);
    assert!(matches!(
        harness.sessions.request_move(other, call(), to).await,
        Err(ShellError::NotMovable { reason: "the call does not run", .. })
    ));
}

#[tokio::test]
async fn move_after_detach_fails() {
    let harness = Harness::new(ZSH);
    let sessions = harness.sessions.clone();
    let request = request("sleep 100").with_timeout(Duration::from_secs(5));
    let run = tokio::spawn(async move {
        sessions.run_command(conversation(), request, &mut crate::NoProgress).await
    });
    let mut terminal = harness.holder.terminal(0).await;
    terminal.prompt().await;
    terminal.typed_line().await;
    terminal.print(b"\r\n\x1b]133;C\x07").await;
    // The deadline, the run's look and the shell's startup timer.
    for _ in 0..5 {
        harness.clock.wait_for_sleeps(3).await;
        harness.clock.advance(Duration::from_secs(1));
    }
    assert_eq!(run.await.unwrap().unwrap().completion, Completion::StillRunning);
    assert_eq!(refused(&harness).await, "the call does not run");
    assert!(harness.sessions.busy(conversation()).is_some(), "the command is left running");
}

#[tokio::test]
async fn next_run_after_move_starts_a_new_shell_in_the_old_cwd() {
    let harness = Harness::new(ZSH);
    let old = harness.dir.path().join("app");
    std::fs::create_dir(&old).unwrap();
    let report = format!("\x1b]7;kitty-shell-cwd://box{}\x07", old.display());
    let (_terminal, run, _) =
        running_on(&harness, 0, request("make"), report.as_bytes(), b"cc\r\n", "cc").await;
    let adopted = move_call(&harness).await;
    assert_eq!(run.await.unwrap().unwrap().cwd_after, old);

    // The next call names another start dir, as for a conversation with no shell.
    let next = RunRequest::new("pwd", "/home/u").with_call(call());
    let sessions = harness.sessions.clone();
    let run = tokio::spawn(async move {
        sessions.run_command(conversation(), next, &mut crate::NoProgress).await
    });
    let mut fresh = harness.holder.terminal(1).await;
    fresh.prompt().await;
    fresh.typed_line().await;
    fresh.run(b"", 0).await;
    assert_eq!(run.await.unwrap().unwrap().exit_code, Some(0));
    let specs = harness.holder.specs();
    assert_eq!(specs[1].cwd, old);
    assert_ne!(specs[1].pty_id, adopted.pty_id());
    // The new shell is the conversation's; the adopted one is not.
    assert_eq!(harness.sessions.state(conversation()).await.unwrap().pty_id, specs[1].pty_id);

    // A dir that is gone falls back to the start dir of the call.
    drop(adopted);
    let harness = Harness::new(ZSH);
    let gone = harness.dir.path().join("gone");
    std::fs::create_dir(&gone).unwrap();
    let report = format!("\x1b]7;kitty-shell-cwd://box{}\x07", gone.display());
    let (_terminal, run, _) =
        running_on(&harness, 0, request("make"), report.as_bytes(), b"cc\r\n", "cc").await;
    let _adopted = move_call(&harness).await;
    run.await.unwrap().unwrap();
    std::fs::remove_dir(&gone).unwrap();
    let next = RunRequest::new("pwd", "/home/u");
    let sessions = harness.sessions.clone();
    tokio::spawn(async move {
        sessions.run_command(conversation(), next, &mut crate::NoProgress).await
    });
    harness.holder.terminal(1).await;
    assert_eq!(harness.holder.specs()[1].cwd, std::path::PathBuf::from("/home/u"));
}

#[tokio::test]
async fn adopted_shell_exit_makes_no_shell_exited_notice() {
    let harness = Harness::new(ZSH);
    let (terminal, run) = running(&harness, "make", b"cc\r\n", "cc").await;
    let mut adopted = move_call(&harness).await;
    run.await.unwrap().unwrap();
    harness.holder.end(terminal.pty_id, ChildStatus::Exited { code: 0 });

    let ended = ShellNotice::AdoptedExited {
        conversation: conversation(),
        pty_id: terminal.pty_id,
        status: Some(ChildStatus::Exited { code: 0 }),
    };
    harness.notices.wait_for(|notice| *notice == ended).await;
    assert!(
        !harness.notices.all().iter().any(|notice| matches!(notice, ShellNotice::Exited { .. })),
        "{:?}",
        harness.notices.all()
    );
    assert!(matches!(adopted.ended().await, Err(ShellError::Exited { .. })));
}

#[tokio::test]
async fn tee_never_blocks_the_actor() {
    let harness = Harness::new(ZSH);
    let (mut terminal, run) = running(&harness, "yes", b"y\r\n", "y").await;
    let mut adopted = move_call(&harness).await;
    run.await.unwrap().unwrap();
    // Nobody reads the output while the command prints more than the queue holds: a
    // chunk is at most one batch of the reader, so this is more chunks than it takes.
    let line = [b'y'; 1023];
    let mut flood = Vec::new();
    while flood.len() <= (TEE_CAPACITY + 8) * crate::reader::BATCH {
        flood.extend_from_slice(&line);
        flood.extend_from_slice(b"\r\n");
    }
    terminal.print(&flood).await;
    terminal.print(b"tail line\r\n\x1b]133;D;0\x07").await;
    terminal.prompt().await;
    // The actor read on: the end came while nothing read the output.
    assert_eq!(adopted.ended().await.unwrap().exit_code, Some(0));

    let text = read_all(adopted.take_output().unwrap()).await;
    assert!(text.starts_with(&format!("{}\n", "y".repeat(1023))), "{}", &text[..64]);
    let marker = text.lines().find(|line| line.starts_with("[efr: ")).unwrap();
    assert!(marker.ends_with(" bytes of output dropped]"), "{marker}");
    let dropped: u64 = marker["[efr: ".len()..].split(' ').next().unwrap().parse().unwrap();
    assert!(dropped > 0);
    // Whole lines of `y`, the marker, and nothing else.
    for line in text.lines() {
        assert!(
            line.chars().all(|c| c == 'y') || line == marker || line == "tail line",
            "{line:?}"
        );
    }
}

#[tokio::test]
async fn adopted_interrupt_and_close() {
    let harness = Harness::new(ZSH);
    let (terminal, run) = running(&harness, "sleep 100", b"z\r\n", "z").await;
    let mut adopted = move_call(&harness).await;
    run.await.unwrap().unwrap();
    let interrupt = (terminal.pty_id, Signal::Interrupt, SignalTarget::ForegroundGroup);

    // The shell itself holds the terminal: the command ended, and nothing is signalled.
    harness.modes.set_foreground(adopted.shell_pid());
    adopted.interrupt().await.unwrap();
    assert!(!harness.holder.signals().contains(&interrupt));
    // A job holds it: SIGINT to the foreground group.
    harness.modes.set_foreground(crate::testing::JOB);
    adopted.interrupt().await.unwrap();
    assert!(harness.holder.signals().contains(&interrupt));

    adopted.close().await.unwrap();
    assert!(harness.holder.signals().contains(&(
        terminal.pty_id,
        Signal::Hangup,
        SignalTarget::Child
    )));
    assert!(matches!(adopted.ended().await, Err(ShellError::Exited { .. })));
    // A closed shell takes an interrupt as done.
    adopted.interrupt().await.unwrap();
}

#[tokio::test]
async fn move_of_a_waiting_run_is_refused() {
    let harness = Harness::new(ZSH);
    let (mut terminal, run, mut heard) =
        running_on(&harness, 0, request("sudo -v"), b"", b"x\r\n", "x").await;
    terminal.print(b"[sudo] password for u: ").await;
    harness.modes.set(Some(HIDDEN));
    // The deadline, the run's look and the shell's startup timer.
    harness.clock.wait_for_sleeps(3).await;
    harness.clock.advance(Duration::from_secs(1));
    heard.inputs(&[InputWait::Hidden]).await;
    assert_eq!(refused(&harness).await, "the command waits for input; answer it first");
    assert!(harness.sessions.state(conversation()).await.is_ok(), "the shell stays");
    assert!(!run.is_finished());
    run.abort();
}

#[tokio::test]
async fn adopted_looks_report_a_hidden_wait() {
    let harness = Harness::new(ZSH);
    let (mut terminal, run) = running(&harness, "./deploy.sh", b"step 1\r\n", "step 1").await;
    let mut adopted = move_call(&harness).await;
    run.await.unwrap().unwrap();
    let mut looks = adopted.looks();
    assert_eq!(*looks.borrow(), (InputWait::None, false));

    terminal.print(b"Password: ").await;
    harness.modes.set(Some(HIDDEN));
    // The watch's look and the shell's startup timer.
    harness.clock.wait_for_sleeps(2).await;
    harness.clock.advance(Duration::from_secs(1));
    looks.wait_for(|look| *look == (InputWait::Hidden, false)).await.unwrap();
    // Nobody follows a job: the wait stops the command at once.
    let interrupt = (terminal.pty_id, Signal::Interrupt, SignalTarget::ForegroundGroup);
    Wait::new("the interrupt of the adopted command")
        .until(|| harness.holder.signals().contains(&interrupt))
        .await
        .unwrap();
    terminal.print(b"^C\r\n\x1b]133;D;130\x07").await;
    terminal.prompt().await;
    assert_eq!(adopted.ended().await.unwrap().exit_code, Some(130));
    // The command ended within the close grace, so the shell stays for the job to close.
    assert!(!harness.holder.signals().contains(&(
        terminal.pty_id,
        Signal::Hangup,
        SignalTarget::Child
    )));
}

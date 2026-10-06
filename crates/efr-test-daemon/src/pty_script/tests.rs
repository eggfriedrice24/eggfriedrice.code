use std::io::{Read as _, Write as _};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

use efr_daemon::{ChildStatus, HolderError, PtyHolder, Signal, SignalTarget, SpawnSpec};
use efr_protocol::{PtyId, Size};
use pretty_assertions::assert_eq;

use super::{FakePtyHolder, PROMPT, PtyScript, PtyStep, command_output, typed_command};
use crate::TestDaemonError;

const SIZE: Size = Size { cols: 80, rows: 24 };

fn pty(n: u128) -> PtyId {
    format!("0192f0c1-7a00-7000-8000-{n:012x}").parse().unwrap()
}

fn spec(n: u128) -> SpawnSpec {
    SpawnSpec::new(pty(n), "/usr/bin/zsh", "/home/u", SIZE)
}

/// The daemon's end of a spawned fake PTY, as a blocking socket.
async fn spawn(holder: &FakePtyHolder, n: u128) -> UnixStream {
    let handle = holder.spawn(spec(n)).await.unwrap();
    assert_eq!(handle.pty_id, pty(n));
    UnixStream::from(handle.master)
}

#[test]
fn a_command_is_typed_as_one_bracketed_paste_and_enter() {
    assert_eq!(typed_command("ls -l"), b"\x1b[efr-clear~\x1b[200~ls -l\x1b[201~\r");
}

#[test]
fn a_command_prints_its_marks_around_the_output_then_the_next_prompt() {
    let mut expected = b"\r\n\x1b]133;C\x07hi\r\n\x1b]133;D;3\x07".to_vec();
    expected.extend_from_slice(PROMPT);

    assert_eq!(command_output(b"hi\r\n", 3), expected);
    assert_eq!(PROMPT, b"\x1b]133;A;cl=line\x07% \x1b]133;B\x07");
}

#[test]
fn a_script_keeps_its_steps_in_order() {
    let script = PtyScript::new().prompt().command("ls", b"a\r\n", 0).print("bye");

    assert_eq!(
        script.steps(),
        [
            PtyStep::Print(PROMPT.to_vec()),
            PtyStep::Expect(typed_command("ls")),
            PtyStep::Print(command_output(b"a\r\n", 0)),
            PtyStep::Print(b"bye".to_vec()),
        ]
    );
}

#[tokio::test]
async fn the_test_reads_what_the_daemon_types_and_the_daemon_reads_what_the_test_prints() {
    let holder = FakePtyHolder::new();
    let mut master = spawn(&holder, 1).await;
    let mut terminal = holder.terminal(0).await.unwrap();
    assert_eq!(terminal.pty_id(), pty(1));

    master.write_all(b"one\rtwo\r").unwrap();
    assert_eq!(terminal.typed_line().await.unwrap(), b"one\r");
    assert_eq!(terminal.typed_exact(2).await.unwrap(), b"tw");
    assert_eq!(terminal.typed_until(b"\r").await.unwrap(), b"o\r");
    terminal.prompt().await.unwrap();
    let mut printed = vec![0; PROMPT.len()];
    master.read_exact(&mut printed).unwrap();
    assert_eq!(printed, PROMPT);
}

#[tokio::test]
async fn the_foreground_is_the_shell_until_the_test_sets_it() {
    let holder = FakePtyHolder::new();
    let handle = holder.spawn(spec(1)).await.unwrap();
    assert_eq!(holder.foreground(pty(1)).await.unwrap(), Some(handle.child_pid));

    assert!(holder.set_foreground(pty(1), Some(77)));
    assert_eq!(holder.foreground(pty(1)).await.unwrap(), Some(77));
    assert!(!holder.set_foreground(pty(2), None));

    holder.end(pty(1), ChildStatus::Exited { code: 0 });
    assert_eq!(holder.foreground(pty(1)).await.unwrap(), None);
}

#[tokio::test]
async fn a_script_plays_and_a_difference_is_a_mismatch() {
    let holder = FakePtyHolder::new();
    let mut master = spawn(&holder, 1).await;
    let mut terminal = holder.terminal(0).await.unwrap();
    master.write_all(&typed_command("ls")).unwrap();
    master.write_all(&typed_command("pwd")).unwrap();

    terminal.play(&PtyScript::new().command("ls", b"a\r\n", 0)).await.unwrap();
    let mismatch = terminal.play(&PtyScript::new().prompt().expect(typed_command("cd"))).await;

    let mut printed = vec![0; command_output(b"a\r\n", 0).len() + PROMPT.len()];
    master.read_exact(&mut printed).unwrap();
    assert!(printed.starts_with(&command_output(b"a\r\n", 0)));
    match mismatch {
        Err(TestDaemonError::Mismatch { line, kind, expected, actual }) => {
            assert_eq!((line, kind), (2, "pty_bytes"));
            assert_eq!(expected, "\u{1b}[efr-clear~\u{1b}[200~cd\u{1b}[201~\r");
            assert_eq!(actual, "\u{1b}[efr-clear~\u{1b}[200~pwd\u{1b}[201~\r");
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn reading_against_an_expectation_stops_at_the_first_difference() {
    let holder = FakePtyHolder::new();
    let mut master = spawn(&holder, 1).await;
    let mut terminal = holder.terminal(0).await.unwrap();
    master.write_all(b"ls\recho\r").unwrap();

    // The session typed a shorter line than expected: the difference shows at once.
    let typed = terminal.typed_against(b"ls -l\r").await.unwrap();
    let matching = terminal.typed_against(b"echo\r").await.unwrap();

    assert_eq!(typed, b"ls\r");
    assert_eq!(matching, b"echo\r");
}

#[tokio::test]
async fn reading_after_the_daemon_closed_the_pty_reports_what_was_typed() {
    let holder = FakePtyHolder::new();
    let mut master = spawn(&holder, 1).await;
    let mut terminal = holder.terminal(0).await.unwrap();
    master.write_all(b"partial").unwrap();
    drop(master);

    match terminal.typed_line().await {
        Err(TestDaemonError::PtyClosed { typed }) => assert_eq!(typed, b"partial"),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn terminals_come_in_the_order_of_spawns_and_each_once() {
    let holder = FakePtyHolder::new();
    let waiting = {
        let holder = holder.clone();
        tokio::spawn(async move { holder.terminal(1).await.map(|terminal| terminal.pty_id()) })
    };
    let _first = spawn(&holder, 1).await;
    let _second = spawn(&holder, 2).await;

    assert_eq!(waiting.await.unwrap().unwrap(), pty(2));
    assert_eq!(holder.terminal(0).await.unwrap().pty_id(), pty(1));
    assert_eq!(holder.spawned(), 2);
    let programs: Vec<PathBuf> = holder.specs().into_iter().map(|spec| spec.program).collect();
    assert_eq!(programs, [PathBuf::from("/usr/bin/zsh"), PathBuf::from("/usr/bin/zsh")]);
}

#[tokio::test]
async fn a_spec_is_checked_and_an_id_is_used_once() {
    let holder = FakePtyHolder::new();
    let relative = SpawnSpec::new(pty(1), "zsh", "/home/u", SIZE);
    let _master = spawn(&holder, 2).await;

    assert!(matches!(holder.spawn(relative).await, Err(HolderError::ProgramNotAbsolute { .. })));
    assert!(matches!(
        holder.spawn(spec(2)).await,
        Err(HolderError::AlreadyExists { pty_id }) if pty_id == pty(2)
    ));
    assert_eq!(holder.spawned(), 1);
}

#[tokio::test]
async fn a_hangup_ends_the_shell_an_interrupt_does_not_and_wait_reports_it() {
    let holder = FakePtyHolder::new();
    let _master = spawn(&holder, 1).await;

    holder.signal(pty(1), Signal::Interrupt, SignalTarget::ForegroundGroup).await.unwrap();
    let listed = holder.list().await.unwrap();
    assert_eq!(listed[0].status, ChildStatus::Running);
    assert_eq!(listed[0].child_pid, 4000);
    holder.signal(pty(1), Signal::Hangup, SignalTarget::Child).await.unwrap();

    assert_eq!(holder.wait(pty(1)).await.unwrap(), ChildStatus::Signaled { signal: 1 });
    assert!(matches!(
        holder.signal(pty(1), Signal::Kill, SignalTarget::Child).await,
        Err(HolderError::Exited { .. })
    ));
    assert_eq!(
        holder.signals(),
        [
            (pty(1), Signal::Interrupt, SignalTarget::ForegroundGroup),
            (pty(1), Signal::Hangup, SignalTarget::Child),
        ]
    );
}

#[tokio::test]
async fn the_test_ends_a_shell_and_a_released_pty_is_gone() {
    let holder = FakePtyHolder::new();
    let _master = spawn(&holder, 1).await;
    let waiting = {
        let holder = holder.clone();
        tokio::spawn(async move { holder.wait(pty(1)).await })
    };

    assert!(holder.end(pty(1), ChildStatus::Exited { code: 3 }));
    assert_eq!(waiting.await.unwrap().unwrap(), ChildStatus::Exited { code: 3 });
    holder.resize(pty(1), Size { cols: 100, rows: 30 }).await.unwrap();
    holder.release(pty(1)).await.unwrap();

    assert_eq!(holder.resizes(), [(pty(1), Size { cols: 100, rows: 30 })]);
    assert_eq!(holder.released(), [pty(1)]);
    assert!(!holder.end(pty(1), ChildStatus::Exited { code: 0 }));
    assert!(matches!(holder.wait(pty(1)).await, Err(HolderError::NotFound { .. })));
    assert!(matches!(holder.release(pty(1)).await, Err(HolderError::NotFound { .. })));
    assert!(holder.list().await.unwrap().is_empty());
}

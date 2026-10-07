//! The holder over real PTYs with `/bin/sh` children. Every child runs in `/` or a temp
//! directory with an environment of only `PATH` and what the test names, so no test
//! reads or writes a real home. Nothing waits on a clock: a test reads the master until
//! the output it expects appears, or until the child's side closes.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{ErrorKind, Read as _, Write as _};
use std::os::fd::{AsRawFd as _, OwnedFd};
use std::path::PathBuf;
use std::pin::pin;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};

use efr_holder::{
    ChildStatus, HolderError, PtyHolder, PtyId, Signal, SignalTarget, Size, SpawnSpec,
};
use pretty_assertions::assert_eq;
use rustix::io::FdFlags;
use rustix::process::{Pid, PidfdFlags, getsid, pidfd_open};
use rustix::termios::{InputModes, tcgetattr, tcgetsid, tcgetwinsize};
use tokio::io::unix::AsyncFd;

use super::{FIRST_NON_STDIO_FD, LocalPtyHolder, open_pty};
use crate::{PtyError, termios};

const SIZE: Size = Size { cols: 80, rows: 24 };

fn pty(n: u8) -> PtyId {
    format!("01920000-0000-7000-8000-0000000000{n:02}").parse().unwrap()
}

/// `/bin/sh -c script` in `/`, with `PATH` as its only variable.
fn sh(pty_id: PtyId, script: &str) -> SpawnSpec {
    SpawnSpec::new(pty_id, "/bin/sh", "/", SIZE).args(["-c", script]).var("PATH", "/usr/bin:/bin")
}

fn pid_of(child_pid: u32) -> Pid {
    Pid::from_raw(i32::try_from(child_pid).unwrap()).unwrap()
}

/// The test's side of a PTY: blocking reads and writes on the caller's master. A read
/// needs nothing from the runtime, so blocking the test's thread is fine.
struct Terminal {
    file: File,
    seen: String,
}

impl Terminal {
    fn new(master: OwnedFd) -> Self {
        Terminal { file: File::from(master), seen: String::new() }
    }

    /// Reads until `needle` has appeared in the output.
    fn expect(&mut self, needle: &str) {
        while !self.seen.contains(needle) {
            assert!(self.read_some(), "the PTY closed before {needle:?}; output: {:?}", self.seen);
        }
    }

    fn write(&mut self, text: &str) {
        self.file.write_all(text.as_bytes()).unwrap();
    }

    /// Reads until no descriptor of the slave is left open, then returns all output.
    fn finish(mut self) -> String {
        while self.read_some() {}
        self.seen
    }

    fn read_some(&mut self) -> bool {
        let mut buf = [0; 4096];
        loop {
            match self.file.read(&mut buf) {
                Ok(0) => return false,
                Ok(n) => {
                    self.seen.push_str(&String::from_utf8_lossy(&buf[..n]));
                    return true;
                }
                // Linux answers EIO on a master once its slave has no open descriptor.
                Err(error) if error.raw_os_error() == Some(libc::EIO) => return false,
                Err(error) if error.kind() == ErrorKind::Interrupted => {}
                Err(error) => panic!("reading the master failed: {error}"),
            }
        }
    }
}

fn assert_not_found<T: std::fmt::Debug>(result: Result<T, HolderError>, id: PtyId) {
    assert!(
        matches!(result, Err(HolderError::NotFound { pty_id }) if pty_id == id),
        "expected NotFound for {id}, got {result:?}"
    );
}

#[tokio::test]
async fn the_programs_output_reaches_the_master() {
    let holder = LocalPtyHolder::new();
    let handle = holder.spawn(sh(pty(1), "echo ok")).await.unwrap();
    assert_eq!(handle.pty_id, pty(1));

    let output = Terminal::new(handle.master).finish();
    assert_eq!(output, "ok\r\n");
    assert_eq!(holder.wait(pty(1)).await.unwrap(), ChildStatus::Exited { code: 0 });
}

#[tokio::test]
async fn the_child_starts_with_the_spec_size() {
    let holder = LocalPtyHolder::new();
    let mut spec = sh(pty(1), "stty size");
    spec.size = Size { cols: 120, rows: 40 };
    let handle = holder.spawn(spec).await.unwrap();
    assert_eq!(Terminal::new(handle.master).finish(), "40 120\r\n");
}

#[tokio::test]
async fn a_resize_reaches_the_child_and_list_reports_it() {
    let holder = LocalPtyHolder::new();
    let handle = holder.spawn(sh(pty(1), "echo ready; read line; stty size")).await.unwrap();
    let mut terminal = Terminal::new(handle.master);
    terminal.expect("ready");

    let resized = Size { cols: 100, rows: 30 };
    holder.resize(pty(1), resized).await.unwrap();
    assert_eq!(holder.list().await.unwrap()[0].size, resized);

    terminal.write("\n");
    assert!(terminal.finish().ends_with("30 100\r\n"));
}

#[tokio::test]
async fn wait_returns_the_exit_status_and_list_agrees() {
    let holder = LocalPtyHolder::new();
    let handle = holder.spawn(sh(pty(1), "exit 3")).await.unwrap();

    assert_eq!(holder.wait(pty(1)).await.unwrap(), ChildStatus::Exited { code: 3 });
    let listed = holder.list().await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].child_pid, handle.child_pid);
    assert_eq!(listed[0].status, ChildStatus::Exited { code: 3 });
    // A second wait answers at once with the same status.
    assert_eq!(holder.wait(pty(1)).await.unwrap(), ChildStatus::Exited { code: 3 });
}

#[tokio::test]
async fn an_exited_child_refuses_signals_but_its_pty_still_resizes() {
    let holder = LocalPtyHolder::new();
    let _handle = holder.spawn(sh(pty(1), "exit 0")).await.unwrap();
    holder.wait(pty(1)).await.unwrap();

    let signalled = holder.signal(pty(1), Signal::Terminate, SignalTarget::Child).await;
    assert!(matches!(signalled, Err(HolderError::Exited { pty_id }) if pty_id == pty(1)));
    let grouped = holder.signal(pty(1), Signal::Interrupt, SignalTarget::ForegroundGroup).await;
    assert!(matches!(grouped, Err(HolderError::Exited { .. })));
    holder.resize(pty(1), Size { cols: 90, rows: 20 }).await.unwrap();
}

#[tokio::test]
async fn a_signal_to_the_child_ends_it() {
    let holder = LocalPtyHolder::new();
    let handle = holder.spawn(sh(pty(1), "echo ready; read line")).await.unwrap();
    let mut terminal = Terminal::new(handle.master);
    terminal.expect("ready");

    holder.signal(pty(1), Signal::Terminate, SignalTarget::Child).await.unwrap();
    assert_eq!(holder.wait(pty(1)).await.unwrap(), ChildStatus::Signaled { signal: libc::SIGTERM });
    assert_eq!(holder.list().await.unwrap()[0].status.exit_code(), None);
}

#[tokio::test]
async fn an_interrupt_to_the_foreground_group_ends_the_command_at_the_terminal() {
    let holder = LocalPtyHolder::new();
    let handle = holder.spawn(sh(pty(1), "echo ready; read line")).await.unwrap();
    let mut terminal = Terminal::new(handle.master);
    terminal.expect("ready");

    holder.signal(pty(1), Signal::Interrupt, SignalTarget::ForegroundGroup).await.unwrap();
    assert_eq!(holder.wait(pty(1)).await.unwrap(), ChildStatus::Signaled { signal: libc::SIGINT });
}

#[tokio::test]
async fn the_child_leads_a_session_whose_controlling_terminal_is_the_pty() {
    let holder = LocalPtyHolder::new();
    let handle = holder.spawn(sh(pty(1), "echo ready; read line")).await.unwrap();
    let pid = pid_of(handle.child_pid);
    let probe = handle.master.try_clone().unwrap();
    let mut terminal = Terminal::new(handle.master);
    terminal.expect("ready");

    assert_eq!(getsid(Some(pid)).unwrap(), pid);
    assert_eq!(tcgetsid(&probe).unwrap(), pid);
    assert_eq!(termios::foreground_group(&probe).unwrap(), pid);

    terminal.write("\n");
    assert_eq!(holder.wait(pty(1)).await.unwrap(), ChildStatus::Exited { code: 0 });
}

#[tokio::test]
async fn foreground_names_the_child_until_it_exits() {
    let holder = LocalPtyHolder::new();
    let handle = holder.spawn(sh(pty(1), "echo ready; read line")).await.unwrap();
    let mut terminal = Terminal::new(handle.master);
    terminal.expect("ready");

    assert_eq!(holder.foreground(pty(1)).await.unwrap(), Some(handle.child_pid));

    terminal.write("\n");
    assert_eq!(holder.wait(pty(1)).await.unwrap(), ChildStatus::Exited { code: 0 });
    assert_eq!(holder.foreground(pty(1)).await.unwrap(), None);
}

/// Starts an intermediate shell that leaves a `sleep` behind and exits, then prints
/// whether the orphaned `sleep` now has the child as its parent.
const ADOPTION: &str = r#"pid=$(sh -c 'sleep 30 >/dev/null 2>&1 & echo $!')
ppid=$(cut -d' ' -f4 "/proc/$pid/stat")
kill "$pid"
if [ "$ppid" = "$$" ]; then echo adopted; else echo not-adopted; fi"#;

#[tokio::test]
async fn a_child_subreaper_adopts_the_orphans_of_its_descendants() {
    let holder = LocalPtyHolder::new();
    let handle = holder.spawn(sh(pty(1), ADOPTION).child_subreaper(true)).await.unwrap();
    assert_eq!(Terminal::new(handle.master).finish(), "adopted\r\n");
}

#[tokio::test]
async fn a_child_is_no_subreaper_unless_the_spec_asks() {
    let holder = LocalPtyHolder::new();
    let handle = holder.spawn(sh(pty(1), ADOPTION)).await.unwrap();
    assert_eq!(Terminal::new(handle.master).finish(), "not-adopted\r\n");
}

#[tokio::test]
async fn the_child_can_open_its_controlling_terminal() {
    let holder = LocalPtyHolder::new();
    let handle = holder.spawn(sh(pty(1), "echo via-tty >/dev/tty")).await.unwrap();
    assert_eq!(Terminal::new(handle.master).finish(), "via-tty\r\n");
    assert_eq!(holder.wait(pty(1)).await.unwrap(), ChildStatus::Exited { code: 0 });
}

#[tokio::test]
async fn the_child_has_only_the_pty_open() {
    // A descriptor without close-on-exec, as a careless library would leave one.
    let leak = OwnedFd::from(File::open("/dev/null").unwrap());
    rustix::io::fcntl_setfd(&leak, FdFlags::empty()).unwrap();

    let holder = LocalPtyHolder::new();
    let handle = holder.spawn(sh(pty(1), "echo ready; read line")).await.unwrap();
    let slave = rustix::pty::ptsname(&handle.master, Vec::new()).unwrap();
    let slave = PathBuf::from(slave.into_string().unwrap());
    let mut terminal = Terminal::new(handle.master);
    terminal.expect("ready");

    let fds: BTreeMap<i32, PathBuf> = std::fs::read_dir(format!("/proc/{}/fd", handle.child_pid))
        .unwrap()
        .map(|entry| {
            let entry = entry.unwrap();
            let fd = entry.file_name().to_str().unwrap().parse().unwrap();
            (fd, std::fs::read_link(entry.path()).unwrap())
        })
        .collect();
    assert!(!fds.contains_key(&leak.as_raw_fd()), "the leaked descriptor reached the child");
    for fd in 0..=2 {
        assert_eq!(fds.get(&fd), Some(&slave), "descriptor {fd} of the child");
    }
    assert!(fds.values().all(|target| *target == slave), "the child holds more: {fds:?}");

    terminal.write("\n");
    holder.wait(pty(1)).await.unwrap();
}

#[tokio::test]
async fn the_child_gets_exactly_the_spec_environment_in_the_spec_directory() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().canonicalize().unwrap();
    // /proc/$$/environ is the environment execve gave the shell, before the shell adds
    // variables of its own; `tr` makes its NUL-separated entries lines.
    let spec = SpawnSpec::new(pty(1), "/bin/sh", &cwd, SIZE)
        .args(["-c", "pwd; tr '\\0' '\\n' </proc/$$/environ"])
        .var("PATH", "/usr/bin:/bin")
        .var("EFR_PTY_TEST", "a value with spaces");

    let holder = LocalPtyHolder::new();
    let handle = holder.spawn(spec).await.unwrap();
    let output = Terminal::new(handle.master).finish();

    let mut lines = output.lines();
    assert_eq!(lines.next(), Some(cwd.to_str().unwrap()));
    let env: BTreeMap<&str, &str> = lines.filter_map(|line| line.split_once('=')).collect();
    assert_eq!(
        env,
        BTreeMap::from([("EFR_PTY_TEST", "a value with spaces"), ("PATH", "/usr/bin:/bin")])
    );
}

#[tokio::test]
async fn a_program_that_cannot_start_fails_the_spawn_and_frees_the_id() {
    let holder = LocalPtyHolder::new();
    let missing = SpawnSpec::new(pty(1), "/nonexistent/efr-pty-test", "/", SIZE);
    match holder.spawn(missing).await {
        Err(HolderError::Spawn { program, source }) => {
            assert_eq!(program, PathBuf::from("/nonexistent/efr-pty-test"));
            assert_eq!(source.kind(), ErrorKind::NotFound);
        }
        other => panic!("expected a spawn error, got {other:?}"),
    }
    assert_eq!(holder.list().await.unwrap(), []);

    let _handle = holder.spawn(sh(pty(1), "exit 0")).await.unwrap();
    assert_eq!(holder.wait(pty(1)).await.unwrap(), ChildStatus::Exited { code: 0 });
}

#[tokio::test]
async fn a_missing_working_directory_fails_the_spawn() {
    let holder = LocalPtyHolder::new();
    let spec = SpawnSpec::new(pty(1), "/bin/sh", "/nonexistent/efr-pty-test", SIZE);
    let spawned = holder.spawn(spec).await;
    assert!(
        matches!(&spawned, Err(HolderError::Spawn { source, .. }) if source.kind() == ErrorKind::NotFound),
        "got {spawned:?}"
    );
    assert_eq!(holder.list().await.unwrap(), []);
}

#[tokio::test]
async fn an_invalid_spec_is_refused_before_anything_is_held() {
    let holder = LocalPtyHolder::new();
    let relative = SpawnSpec::new(pty(1), "sh", "/", SIZE);
    assert!(matches!(holder.spawn(relative).await, Err(HolderError::ProgramNotAbsolute { .. })));
    let empty = SpawnSpec::new(pty(1), "/bin/sh", "/", Size { cols: 0, rows: 24 });
    assert!(matches!(holder.spawn(empty).await, Err(HolderError::EmptySize { .. })));
    assert_eq!(holder.list().await.unwrap(), []);
}

#[tokio::test]
async fn a_second_spawn_with_the_same_id_starts_nothing() {
    let holder = LocalPtyHolder::new();
    let handle = holder.spawn(sh(pty(1), "echo ready; read line")).await.unwrap();
    let mut terminal = Terminal::new(handle.master);
    terminal.expect("ready");

    let retry = holder.spawn(sh(pty(1), "echo second")).await;
    assert!(matches!(retry, Err(HolderError::AlreadyExists { pty_id }) if pty_id == pty(1)));
    let listed = holder.list().await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].child_pid, handle.child_pid);

    terminal.write("\n");
    assert!(!terminal.finish().contains("second"));
}

#[tokio::test]
async fn two_ptys_are_independent() {
    let holder = LocalPtyHolder::new();
    let _one = holder.spawn(sh(pty(1), "exit 1")).await.unwrap();
    let _two = holder.spawn(sh(pty(2), "exit 2")).await.unwrap();

    assert_eq!(holder.wait(pty(2)).await.unwrap(), ChildStatus::Exited { code: 2 });
    assert_eq!(holder.wait(pty(1)).await.unwrap(), ChildStatus::Exited { code: 1 });
    let listed: BTreeMap<PtyId, ChildStatus> =
        holder.list().await.unwrap().into_iter().map(|info| (info.pty_id, info.status)).collect();
    assert_eq!(
        listed,
        BTreeMap::from([
            (pty(1), ChildStatus::Exited { code: 1 }),
            (pty(2), ChildStatus::Exited { code: 2 }),
        ])
    );
}

#[tokio::test]
async fn after_release_every_method_answers_not_found() {
    let holder = LocalPtyHolder::new();
    let _handle = holder.spawn(sh(pty(1), "exit 0")).await.unwrap();
    holder.wait(pty(1)).await.unwrap();
    holder.release(pty(1)).await.unwrap();

    assert_eq!(holder.list().await.unwrap(), []);
    assert_not_found(holder.resize(pty(1), SIZE).await, pty(1));
    assert_not_found(holder.signal(pty(1), Signal::Hangup, SignalTarget::Child).await, pty(1));
    assert_not_found(holder.foreground(pty(1)).await, pty(1));
    assert_not_found(holder.wait(pty(1)).await, pty(1));
    assert_not_found(holder.release(pty(1)).await, pty(1));
}

#[tokio::test]
async fn a_release_ends_a_pending_wait_with_not_found() {
    let holder = Arc::new(LocalPtyHolder::new());
    let handle = holder.spawn(sh(pty(1), "echo ready; read line")).await.unwrap();
    let mut terminal = Terminal::new(handle.master);
    terminal.expect("ready");

    let waiting = tokio::spawn({
        let holder = Arc::clone(&holder);
        async move { holder.wait(pty(1)).await }
    });
    tokio::task::yield_now().await;
    assert!(!waiting.is_finished(), "the child still runs");
    holder.release(pty(1)).await.unwrap();
    assert_not_found(waiting.await.unwrap(), pty(1));

    terminal.write("\n");
    terminal.finish();
}

#[tokio::test]
async fn a_released_child_keeps_running_while_the_caller_holds_the_master() {
    let holder = LocalPtyHolder::new();
    let handle = holder.spawn(sh(pty(1), "echo ready; read line; echo done")).await.unwrap();
    let mut terminal = Terminal::new(handle.master);
    terminal.expect("ready");

    holder.release(pty(1)).await.unwrap();
    terminal.write("\n");
    assert!(terminal.finish().ends_with("done\r\n"));
}

#[tokio::test]
async fn closing_the_last_master_after_release_hangs_up_the_child() {
    let holder = LocalPtyHolder::new();
    let handle = holder.spawn(sh(pty(1), "echo ready; read line")).await.unwrap();
    // A pidfd turns readable when the process exits; the test opens its own, before
    // anything can reap the child, because the holder forgets the PTY on release.
    let pidfd =
        AsyncFd::new(pidfd_open(pid_of(handle.child_pid), PidfdFlags::empty()).unwrap()).unwrap();
    let mut terminal = Terminal::new(handle.master);
    terminal.expect("ready");

    holder.release(pty(1)).await.unwrap();
    // The holder's copy is gone, so closing this one hangs up the terminal and the
    // kernel sends SIGHUP to the session leader. Were the holder's copy still open, the
    // child would block on `read` and this wait would not end.
    drop(terminal);
    drop(pidfd.readable().await.unwrap());
}

#[tokio::test]
async fn a_released_id_can_be_spawned_again() {
    let holder = LocalPtyHolder::new();
    let _first = holder.spawn(sh(pty(1), "exit 1")).await.unwrap();
    holder.wait(pty(1)).await.unwrap();
    holder.release(pty(1)).await.unwrap();

    let _second = holder.spawn(sh(pty(1), "exit 2")).await.unwrap();
    assert_eq!(holder.wait(pty(1)).await.unwrap(), ChildStatus::Exited { code: 2 });
}

#[tokio::test]
async fn the_holder_works_as_a_trait_object() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<LocalPtyHolder>();

    let holder: Arc<dyn PtyHolder> = Arc::new(LocalPtyHolder::new());
    let handle = holder.spawn(sh(pty(1), "echo ok")).await.unwrap();
    assert_eq!(Terminal::new(handle.master).finish(), "ok\r\n");
    assert_eq!(holder.wait(pty(1)).await.unwrap(), ChildStatus::Exited { code: 0 });
}

#[test]
fn a_spawn_outside_a_tokio_runtime_fails_before_it_holds_the_id() {
    let holder = LocalPtyHolder::new();
    let mut context = Context::from_waker(Waker::noop());
    // The spawn never awaits, so one poll finishes it.
    let spawned = pin!(holder.spawn(sh(pty(1), "exit 0"))).poll(&mut context);
    match spawned {
        Poll::Ready(Err(HolderError::Spawn { source, .. })) => {
            let step = source.get_ref().and_then(|inner| inner.downcast_ref::<PtyError>());
            assert!(matches!(step, Some(PtyError::NoRuntime)), "got {source:?}");
        }
        other => panic!("expected a spawn error, got {other:?}"),
    }
    assert!(holder.lock().is_empty());
}

#[test]
fn a_new_pty_has_the_size_and_modes_and_its_slave_is_never_a_standard_stream() {
    let size = Size { cols: 132, rows: 43 };
    let pty = open_pty(size).unwrap();

    assert!(pty.slave.as_raw_fd() >= FIRST_NON_STDIO_FD);
    for fd in [&pty.master, &pty.slave] {
        assert!(rustix::io::fcntl_getfd(fd).unwrap().contains(FdFlags::CLOEXEC));
    }
    assert_eq!(tcgetwinsize(&pty.master).unwrap(), termios::winsize(size));
    assert!(tcgetattr(&pty.slave).unwrap().input_modes.contains(InputModes::IUTF8));
}

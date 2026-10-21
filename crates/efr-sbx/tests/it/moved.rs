//! The end of a call that efrd moved into a job (Ctrl+B in `efr`), on the real bwrap,
//! Landlock and seccomp: with `$CALL/moved` the launcher writes no `apply`, no
//! `state.json`, no `state.zsh` and no config listings, as at the end of a job, but
//! still its `result.json`. It reads the file at the end of the call, because efrd makes
//! it while the call runs.
//!
//! A test never sleeps for a call: the call writes a line to a FIFO and then waits on
//! a FIFO that the test writes.

use std::fs;
use std::io::{BufRead as _, BufReader, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

use efr_sandbox::{CONFIG_LISTINGS_FILE, MOVED_FILE, SpecLaunch};

use crate::support::{Fixture, Run, q, run_with_timeout, sandbox_or_skip};

/// The line of each test: a change of the shell state that a call keeps (a promoted
/// export and a cd), then `rest`.
fn line(fixture: &Fixture, rest: &str) -> String {
    let sub = fixture.project.join("sub");
    fs::create_dir_all(&sub).unwrap();
    format!("export RUST_LOG=debug; cd {}; {rest}", q(&sub))
}

/// Makes `$CALL/moved` as efrd does: empty, mode 0600, and only once.
fn mark_moved(call_dir: &Path) {
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(call_dir.join(MOVED_FILE))
        .unwrap()
        .flush()
        .unwrap();
}

/// Runs `line` from the project in a call dir that `before` may change first.
fn call(fixture: &Fixture, line: &str, before: impl FnOnce(&Path)) -> Run {
    let call_dir = fixture.prepare(line);
    before(&call_dir);
    let command = fixture.command(&call_dir, &fixture.project);
    fixture.collect(&call_dir, run_with_timeout(command))
}

/// The shell state files of the fixture's conversation.
fn state_files(fixture: &Fixture) -> [bool; 3] {
    let shell_dir = &fixture.spec.runtime.shell_dir;
    ["state.json", "state.zsh", CONFIG_LISTINGS_FILE].map(|name| shell_dir.join(name).exists())
}

#[test]
fn moved_call_writes_no_state_or_apply() {
    let ready = sandbox_or_skip!();
    let mut fixture = Fixture::new(&ready);
    fixture.git_init();
    for launch in [SpecLaunch::Contained, SpecLaunch::Unsandboxed] {
        fixture.spec.launch = launch;
        let run = call(&fixture, &line(&fixture, "print -r -- ran"), mark_moved);
        run.expect_status(0);
        assert_eq!(run.stdout.trim(), "ran", "{launch:?}: {run:#?}");
        assert_eq!(run.result().exit_code, Some(0), "{launch:?}: the result is still written");
        assert!(!run.result().state_kept, "{launch:?}: {run:#?}");
        assert!(!run.result().summary.cwd_changed, "{launch:?}: {run:#?}");
        assert_eq!(run.apply, None, "{launch:?}: a moved call writes no apply");
        assert_eq!(state_files(&fixture), [false; 3], "{launch:?}: {run:#?}");
    }
}

#[test]
fn call_without_moved_file_writes_state() {
    let ready = sandbox_or_skip!();
    let mut fixture = Fixture::new(&ready);
    fixture.git_init();
    let sub = fixture.project.join("sub").display().to_string();
    for launch in [SpecLaunch::Contained, SpecLaunch::Unsandboxed] {
        fixture.spec.launch = launch;
        let run = call(&fixture, &line(&fixture, "print -r -- ran"), |_| {});
        run.expect_status(0);
        assert!(run.result().state_kept, "{launch:?}: {run:#?}");
        let fields = run.apply_fields();
        assert!(fields.starts_with(&["cd".to_owned(), sub.clone()]), "{launch:?}: {run:#?}");
    }
    // Only a contained call keeps the sandbox's own state; the last one ran outside.
    assert_eq!(state_files(&fixture)[..2], [true, true]);
}

#[test]
fn moved_file_written_after_start_is_seen_at_the_end() {
    let ready = sandbox_or_skip!();
    let mut fixture = Fixture::new(&ready);
    fixture.git_init();
    for launch in [SpecLaunch::Contained, SpecLaunch::Unsandboxed] {
        fixture.spec.launch = launch;
        let up = fixture.project.join("up");
        let go = fixture.project.join("go");
        // The call writes a line to the FIFO `up` once it runs, then reads the FIFO
        // `go`; the test marks the call moved in that gap, as efrd does when the user
        // presses Ctrl+B. The test holds both FIFOs open for reading and writing, so no
        // open blocks and a line ends the wait of the mover if the call never runs.
        let up_end = fifo(&up);
        let _go_end = fifo(&go);
        let rest = format!(
            "print -r -- up > {up}; read -r _ < {go}; print -r -- ran",
            up = q(&up),
            go = q(&go),
        );
        let line = line(&fixture, &rest);
        let call_dir = fixture.prepare(&line);
        let (run, marked) = std::thread::scope(|scope| {
            let mover = scope.spawn(|| {
                let mut said = String::new();
                let reader = fs::File::open(&up).unwrap();
                BufReader::new(reader).read_line(&mut said).unwrap();
                if said != "up\n" {
                    return false;
                }
                assert!(!call_dir.join(MOVED_FILE).exists());
                mark_moved(&call_dir);
                fs::OpenOptions::new().write(true).open(&go).unwrap().write_all(b"go\n").unwrap();
                true
            });
            let command = fixture.command(&call_dir, &fixture.project);
            let run = fixture.collect(&call_dir, run_with_timeout(command));
            // NOTE: a call that never wrote `up` leaves the mover waiting; this line ends
            // its wait. After a move it stays unread in the FIFO.
            (&up_end).write_all(b"ended\n").unwrap();
            (run, mover.join().unwrap())
        });
        assert!(marked, "{launch:?}: the call never ran: {run:#?}");
        run.expect_status(0);
        assert_eq!(run.stdout.trim(), "ran", "{launch:?}: {run:#?}");
        assert!(!run.result().state_kept, "{launch:?}: {run:#?}");
        assert_eq!(run.apply, None, "{launch:?}: {run:#?}");
        assert_eq!(state_files(&fixture), [false; 3], "{launch:?}: {run:#?}");
    }
}

/// Makes the FIFO `path` anew, mode 0600, and opens it for reading and writing: an open
/// of either end never blocks while the handle lives.
fn fifo(path: &Path) -> fs::File {
    let _ = fs::remove_file(path);
    let mode = rustix::fs::Mode::from_raw_mode(0o600);
    rustix::fs::mknodat(rustix::fs::CWD, path, rustix::fs::FileType::Fifo, mode, 0).unwrap();
    fs::OpenOptions::new().read(true).write(true).open(path).unwrap()
}

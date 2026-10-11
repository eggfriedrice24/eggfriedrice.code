//! The log lines of a snapshot whose `git add` fails: files that git cannot read, once
//! per root, and a fatal git, each time.

use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt as _;
use std::sync::{Arc, Mutex};

use tracing::subscriber::DefaultGuard;

use crate::support::{World, conversation, limits, root, turn, write};

/// Every log line of this thread while its guard lives, at every level, as text.
#[derive(Clone, Default)]
struct Logs(Arc<Mutex<Vec<u8>>>);

impl Logs {
    fn start() -> (Logs, DefaultGuard) {
        let logs = Logs::default();
        let sink = logs.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(move || sink.clone())
            .with_ansi(false)
            .with_max_level(tracing::Level::TRACE)
            .finish();
        (logs, tracing::subscriber::set_default(subscriber))
    }

    fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }
}

impl io::Write for Logs {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Two turns of one call each in `dir`, and each call changes `a.txt`.
async fn two_turns(world: &World, dir: &std::path::Path) {
    for n in 1..=2 {
        let call =
            world.snapshots.before_call(conversation(1), turn(n), vec![root(dir)], limits()).await;
        write(&dir.join("a.txt"), &format!("{n}\n"));
        world.snapshots.after_call(call, limits()).await;
    }
}

#[tokio::test]
async fn an_unreadable_file_is_reported_once_per_root() {
    let world = World::new();
    let dir = world.dir("plain");
    write(&dir.join("a.txt"), "0\n");
    let secret = dir.join("secret");
    write(&secret, "s\n");
    fs::set_permissions(&secret, fs::Permissions::from_mode(0o000)).unwrap();
    if fs::read(&secret).is_ok() {
        // NOTE: root (or a process with CAP_DAC_OVERRIDE) reads the file anyway, so
        // git adds it and nothing fails.
        return;
    }
    let other = world.dir("other");
    let other_secret = other.join("key");
    write(&other_secret, "k\n");
    fs::set_permissions(&other_secret, fs::Permissions::from_mode(0o000)).unwrap();

    let (logs, _guard) = Logs::start();
    two_turns(&world, &dir).await;
    two_turns(&world, &other).await;

    let text = logs.text();
    let lines: Vec<&str> = text
        .lines()
        .filter(|line| line.contains("some files could not be added to a snapshot"))
        .collect();
    assert_eq!(lines.len(), 2, "{text}");
    assert!(lines[0].contains("DEBUG"), "{text}");
    assert!(lines[0].contains("count=1"), "{text}");
    assert!(lines[0].contains("first=\"secret\""), "{text}");
    assert!(lines[0].contains(&format!("root={}", dir.display())), "{text}");
    assert!(lines[1].contains("first=\"key\""), "{text}");
    assert!(!text.contains("WARN"), "{text}");
}

#[tokio::test]
async fn a_fatal_git_add_is_a_warn() {
    let world = World::new();
    let script = world.base.path().join("git-fatal-add");
    let text = "#!/bin/sh\n\
                skip=\n\
                for arg; do\n\
                \x20 if [ -n \"$skip\" ]; then skip=; elif [ \"$arg\" = -c ]; then skip=1; \
                elif [ \"$arg\" = add ]; then \
                echo \"fatal: pathspec 'gone.txt' did not match any files\" >&2; exit 128; \
                else break; fi\n\
                done\n\
                exec git \"$@\"\n";
    let world = world.with_git_script(&script, text);
    let dir = world.dir("plain");
    write(&dir.join("a.txt"), "0\n");

    let (logs, _guard) = Logs::start();
    two_turns(&world, &dir).await;

    let text = logs.text();
    let warnings: Vec<&str> = text
        .lines()
        .filter(|line| line.contains("git stopped and added no file to a snapshot"))
        .collect();
    // Each snapshot that adds files says it: two calls, before and after each.
    assert_eq!(warnings.len(), 4, "{text}");
    for line in warnings {
        assert!(line.contains("WARN"), "{text}");
        assert!(line.contains("fatal: pathspec 'gone.txt' did not match any files"), "{text}");
    }
    assert!(!text.contains("some files could not be added to a snapshot"), "{text}");
}

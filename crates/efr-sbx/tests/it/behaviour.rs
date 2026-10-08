//! The behaviour tests of the spec's section 16.2 that need only the launcher and a call
//! dir: status, signals, the private tmp, the state between calls, overlays, grants,
//! the exit child and setup failures.

use std::fs;
use std::io::{BufRead as _, BufReader, Read};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use efr_protocol::{CacheMode, Grant};
use efr_sandbox::{CacheOverlay, SpecLaunch, WriteRoot, WriteRootKind};

use crate::support::{Fixture, Run, need, q, run_with_timeout, sandbox_or_skip, write};

#[test]
fn exit_status_propagates() {
    let ready = sandbox_or_skip!();
    let fixture = Fixture::new(&ready);
    let run = fixture.run("exit 7");
    run.expect_status(7);
    assert_eq!(run.result().exit_code, Some(7));
    assert_eq!(run.result().setup_error, None);
}

#[test]
fn signal_status_propagates() {
    let ready = sandbox_or_skip!();
    let fixture = Fixture::new(&ready);
    let run = fixture.run("kill -TERM $$");
    run.expect_status(143);
    assert_eq!(run.result().signal, Some(15));
}

#[test]
fn setup_failure_is_not_command_failure() {
    let ready = sandbox_or_skip!();
    let mut fixture = Fixture::new(&ready);
    let run = fixture.run("exit 125");
    run.expect_status(125);
    assert_eq!(run.result().setup_error, None);
    assert_eq!(run.result().exit_code, Some(125));
    // A write root inside a mask refuses the plan: nothing runs.
    let marker = fixture.project.join("ran");
    let project = fixture.project.clone();
    fixture.mask(&project, efr_sandbox::MaskKind::User);
    let run = fixture.run(&format!("touch {}", q(&marker)));
    run.expect_status(125);
    assert!(run.result().setup_error.as_deref().unwrap_or_default().contains("cannot run here"));
    assert!(!marker.exists());
}

/// An executable script at `path`.
fn script(path: &Path, text: &str) {
    fs::write(path, text).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}

#[test]
fn bwrap_setup_error_not_in_output() {
    let ready = sandbox_or_skip!();
    let mut fixture = Fixture::new(&ready);
    // The real bwrap with an option it does not know: it fails before the command.
    let bwrap = fixture.base.join("bwrap");
    let real = fixture.spec.runtime.bwrap.display().to_string();
    script(
        &bwrap,
        &format!("#!/bin/sh\nexec {} --efr-no-such-option \"$@\"\n", q(Path::new(&real))),
    );
    fixture.spec.runtime.bwrap = bwrap;
    let run = fixture.run("print ran");
    run.expect_status(125);
    let error = run.result().setup_error.clone().unwrap_or_default();
    assert!(error.contains("efr-no-such-option"), "{run:#?}");
    assert!(!run.stderr.contains("bwrap"), "{run:#?}");
    assert!(!run.stdout.contains("ran"), "{run:#?}");
}

#[test]
fn two_caches_on_one_layer_dir_refuse_the_call() {
    let ready = sandbox_or_skip!();
    let mut fixture = Fixture::new(&ready);
    let first = fixture.home.join(".cache");
    let second = fixture.home.join(".npm");
    fs::create_dir_all(&first).unwrap();
    fs::create_dir_all(&second).unwrap();
    let shared = CacheOverlay::new(&first, &fixture.spec.runtime.sandbox_dir, &fixture.home);
    fixture.spec.caches.push(shared.clone());
    fixture.spec.caches.push(CacheOverlay { target: second, ..shared });
    let marker = fixture.project.join("ran");
    let run = fixture.run(&format!("touch {}", q(&marker)));
    run.expect_status(125);
    let error = run.result().setup_error.clone().unwrap_or_default();
    assert!(error.contains("cannot run here") && error.contains("layer dir"), "{run:#?}");
    assert!(!marker.exists());
}

#[test]
fn overlay_back_to_back_calls_mount_every_time() {
    // With the kernel's index=on, the upper dir stayed busy for a few milliseconds after
    // a call, and the next call's mount failed with EBUSY. The helper mounts with
    // index=off, and the launcher tears the last call's mounts down before it ends.
    let ready = sandbox_or_skip!();
    let mut fixture = Fixture::new(&ready);
    let cache = fixture.home.join(".cache");
    write(&cache.join("index"), "one\n");
    fixture.cache(&cache);
    for n in 0..30 {
        let run = fixture.run(&format!("print {n} >> {}/calls", q(&cache)));
        run.expect_status(0);
        assert_eq!(run.result().setup_error, None, "{run:#?}");
    }
    let run = fixture.run(&format!("wc -l < {}/calls", q(&cache)));
    assert_eq!(run.stdout.trim(), "30", "{run:#?}");
    assert!(!cache.join("calls").exists(), "the writes reached the user's cache");
}

#[test]
fn a_call_waits_while_another_launch_holds_the_layers_of_its_conversation() {
    // The overlays have no index, so the kernel would let two of them share an upper
    // dir; the launcher holds a lock on the conversation's layer dir instead.
    let ready = sandbox_or_skip!();
    let mut fixture = Fixture::new(&ready);
    let cache = fixture.home.join(".cache");
    write(&cache.join("index"), "one\n");
    fixture.cache(&cache);
    let layers = fixture.spec.runtime.sandbox_dir.join("cache");
    fs::create_dir_all(fixture.spec.caches[0].upper.parent().unwrap()).unwrap();
    let held = fs::File::open(&layers).unwrap();
    rustix::fs::flock(&held, rustix::fs::FlockOperation::LockExclusive).unwrap();
    let marker = fixture.project.join("ran");
    let call_dir = fixture.prepare(&format!("cat {}/index > {}", q(&cache), q(&marker)));
    let child = fixture.command(&call_dir, &fixture.project).spawn().unwrap();
    std::thread::sleep(Duration::from_millis(500));
    assert!(!marker.exists(), "the call ran while another launch held its layers");
    assert!(!call_dir.join("started").exists(), "bwrap started while the layers were held");
    drop(held);
    let output = child.wait_with_output().unwrap();
    let run = fixture
        .collect(&call_dir, (output.status.code().unwrap_or(-1), String::new(), String::new()));
    run.expect_status(0);
    assert_eq!(fs::read_to_string(&marker).unwrap(), "one\n");
}

#[test]
fn a_cache_inside_another_cache_runs_in_either_order() {
    // The inner cache came first and got an overlay of its own, and the helper failed to
    // move it onto the outer overlay (EINVAL): every call failed at setup.
    let ready = sandbox_or_skip!();
    for mode in [CacheMode::Overlay, CacheMode::Tmp] {
        let mut fixture = Fixture::new(&ready);
        fixture.spec.cache_mode = mode;
        let outer = fixture.home.join(".cache");
        let inner = outer.join("pip");
        write(&inner.join("index"), "one\n");
        // ~/.cargo is a link into ~/.cache, and its pins still hold.
        let cargo = outer.join("cargo");
        write(&cargo.join("config.toml"), "[build]\n");
        let link = fixture.home.join(".cargo");
        std::os::unix::fs::symlink(&cargo, &link).unwrap();
        fixture.cache(&link);
        fixture.cache(&inner);
        fixture.cache(&outer);
        let run = fixture.run(&format!(
            "print two >> {0}/index && cat {0}/index; print x >> {1}/config.toml; print $?",
            q(&inner),
            q(&link),
        ));
        run.expect_status(0);
        assert_eq!(run.result().setup_error, None, "{mode:?}: {run:#?}");
        assert!(run.stdout.starts_with("one\ntwo\n"), "{mode:?}: {run:#?}");
        assert!(run.stdout.ends_with("1\n"), "{mode:?}: the pin did not hold: {run:#?}");
        assert_eq!(fs::read_to_string(inner.join("index")).unwrap(), "one\n");
        assert_eq!(fs::read_to_string(cargo.join("config.toml")).unwrap(), "[build]\n");
    }
}

#[test]
fn tmp_cache_writes_go_away_after_the_call() {
    let ready = sandbox_or_skip!();
    let mut fixture = Fixture::new(&ready);
    fixture.spec.cache_mode = CacheMode::Tmp;
    let cache = fixture.home.join(".cache");
    write(&cache.join("index"), "one\n");
    fixture.cache(&cache);
    let run = fixture.run(&format!(
        "print two > {0}/index && print new > {0}/new && cat {0}/index {0}/new",
        q(&cache)
    ));
    run.expect_status(0);
    assert_eq!(run.stdout, "two\nnew\n", "{run:#?}");
    assert_eq!(fs::read_to_string(cache.join("index")).unwrap(), "one\n");
    assert!(!cache.join("new").exists());
    let run = fixture.run(&format!("cat {0}/index; ls {0}", q(&cache)));
    assert_eq!(run.stdout, "one\nindex\n", "{run:#?}");
}

#[test]
fn a_failed_layer_helper_stops_the_call() {
    let ready = sandbox_or_skip!();
    let mut fixture = Fixture::new(&ready);
    let cache = fixture.home.join(".cache");
    fs::create_dir_all(&cache).unwrap();
    fixture.cache(&cache);
    // A launcher whose layer helper fails, and which is the real one otherwise.
    let launcher = fixture.runtime.join("efr-sbx");
    let real = q(&fixture.ready.bin);
    script(
        &launcher,
        &format!(
            "#!/bin/sh\nif [ \"$1\" = layers ]; then echo 'planted helper failure' >&2; \
             exit 125; fi\nexec {real} \"$@\"\n"
        ),
    );
    fixture.spec.runtime.launcher = launcher;
    let marker = fixture.project.join("ran");
    let run = fixture.run(&format!("touch {}", q(&marker)));
    run.expect_status(125);
    let error = run.result().setup_error.clone().unwrap_or_default();
    assert!(error.contains("cache overlays did not mount"), "{run:#?}");
    assert!(error.contains("planted helper failure"), "{run:#?}");
    assert!(!marker.exists(), "the call ran without its overlays");
}

#[test]
fn stdin_reaches_a_sandboxed_read() {
    let ready = sandbox_or_skip!();
    let fixture = Fixture::new(&ready);
    let answer = fixture.base.join("answer");
    fs::write(&answer, "yes\n").unwrap();
    let run = fixture.call("read -r reply; print got-$reply", &fixture.project, |command| {
        command.stdin(Stdio::from(fs::File::open(&answer).unwrap()));
    });
    run.expect_status(0);
    assert!(run.stdout.contains("got-yes"), "{run:#?}");
}

#[test]
fn stderr_reopen_works() {
    let ready = sandbox_or_skip!();
    let fixture = Fixture::new(&ready);
    let flags = rustix::pty::OpenptFlags::RDWR | rustix::pty::OpenptFlags::NOCTTY;
    let master = rustix::pty::openpt(flags).unwrap();
    rustix::pty::grantpt(&master).unwrap();
    rustix::pty::unlockpt(&master).unwrap();
    let name = rustix::pty::ptsname(&master, Vec::new()).unwrap();
    let slave = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(i32::try_from(rustix::fs::OFlags::NOCTTY.bits()).unwrap())
        .open(name.to_string_lossy().as_ref())
        .unwrap();
    let run = fixture.call("print reopen-ok > /dev/stderr && exit 0", &fixture.project, |c| {
        c.stderr(Stdio::from(slave));
    });
    run.expect_status(0);
    rustix::io::ioctl_fionbio(&master, true).unwrap();
    let mut text = Vec::new();
    let _ = fs::File::from(master).read_to_end(&mut text);
    assert!(String::from_utf8_lossy(&text).contains("reopen-ok"));
}

#[test]
fn openpty_works_inside() {
    let ready = sandbox_or_skip!();
    need!("script");
    let fixture = Fixture::new(&ready);
    let run = fixture.run("script -qec 'echo pty-ok' /dev/null");
    run.expect_status(0);
    assert!(run.stdout.contains("pty-ok"), "{run:#?}");
}

#[test]
fn tmp_private_and_kept_in_conversation() {
    let ready = sandbox_or_skip!();
    let mut fixture = Fixture::new(&ready);
    let name = format!("efr-sbx-private-{}", std::process::id());
    fixture.run(&format!("print kept > /tmp/{name}")).expect_status(0);
    assert!(!Path::new("/tmp").join(&name).exists(), "the host's /tmp got the file");
    assert!(fixture.private_tmp().join(&name).exists());
    let run = fixture.run(&format!("cat /tmp/{name}"));
    assert!(run.stdout.contains("kept"), "{run:#?}");
    fixture.switch_conversation();
    let run = fixture.run(&format!("cat /tmp/{name}"));
    assert!(!run.stdout.contains("kept"), "another conversation saw it: {run:#?}");
}

#[test]
fn cwd_under_host_tmp_starts_in_scratch() {
    let ready = sandbox_or_skip!();
    let fixture = Fixture::new(&ready);
    let host_tmp = tempfile::Builder::new().prefix("efr-sbx").tempdir_in("/tmp").unwrap();
    let run = fixture.call("pwd", host_tmp.path(), |_| {});
    run.expect_status(0);
    let scratch = fixture.spec.runtime.scratch.display().to_string();
    assert_eq!(run.stdout.trim(), scratch, "{run:#?}");
    let hidden = run.result().hidden_cwd.clone();
    assert_eq!(hidden.as_deref(), Some(host_tmp.path()));
}

#[test]
fn ps_sees_host_processes() {
    let ready = sandbox_or_skip!();
    need!("ps");
    let fixture = Fixture::new(&ready);
    let run = fixture.run("ps -e -o pid=");
    run.expect_status(0);
    let me = std::process::id().to_string();
    assert!(run.stdout.lines().any(|line| line.trim() == me), "{run:#?}");
}

#[test]
fn git_commit_works_with_pins() {
    let ready = sandbox_or_skip!();
    let fixture = Fixture::new(&ready);
    fixture.git_init();
    let run = fixture.run(
        "print x > a && git add a && git commit -qm a && git switch -qc feat && \
         git stash list && git log --oneline | wc -l",
    );
    run.expect_status(0);
    assert_eq!(run.stdout.trim(), "2", "{run:#?}");
}

#[test]
fn overlay_upper_below_state_mask() {
    let ready = sandbox_or_skip!();
    let mut fixture = Fixture::new(&ready);
    let cache = fixture.home.join(".cache");
    write(&cache.join("go-build/old"), "old\n");
    fixture.cache(&cache);
    let run = fixture.run(&format!(
        "print new > {}/go-build/new && cat {}/go-build/old && ls -A {}",
        q(&cache),
        q(&cache),
        q(&fixture.state)
    ));
    run.expect_status(0);
    assert_eq!(run.stdout, "old\n", "{run:#?}");
    assert!(!cache.join("go-build/new").exists(), "the write reached the user's cache");
    let upper = &fixture.spec.caches[0].upper;
    assert!(upper.join("go-build/new").exists());
}

#[test]
fn overlay_lower_changes_between_calls() {
    let ready = sandbox_or_skip!();
    let mut fixture = Fixture::new(&ready);
    let cache = fixture.home.join(".cache");
    write(&cache.join("index"), "one\n");
    fixture.cache(&cache);
    fixture.run(&format!("cat {}/index", q(&cache))).expect_status(0);
    write(&cache.join("index"), "two\n");
    write(&cache.join("fetched-later"), "later\n");
    let run = fixture.run(&format!("cat {}/index {}/fetched-later", q(&cache), q(&cache)));
    run.expect_status(0);
    // A file the sandbox never copied up shows the user's newer content.
    assert_eq!(run.stdout, "two\nlater\n", "{run:#?}");
}

#[test]
fn overlay_lower_changes_during_call() {
    let ready = sandbox_or_skip!();
    let mut fixture = Fixture::new(&ready);
    let cache = fixture.home.join(".cache");
    write(&cache.join("index"), "one\n");
    fixture.cache(&cache);
    // The project is shared with the test. The call writes `read` after its first read
    // of the index, then waits for `changed`. The test changes the user's cache in that
    // gap, so a slow launch cannot move the change before the first read.
    let read = fixture.project.join("read");
    let changed = fixture.project.join("changed");
    let line = format!(
        "cat {c}/index; : > {read}; until [[ -e {changed} ]]; do sleep 0.01; done; \
         cat {c}/index; ls {c}",
        c = q(&cache),
        read = q(&read),
        changed = q(&changed),
    );
    let index = cache.join("index");
    let ended = AtomicBool::new(false);
    let (run, did_change) = std::thread::scope(|scope| {
        let changer = scope.spawn(|| {
            while !read.exists() {
                if ended.load(Ordering::SeqCst) {
                    return false;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            fs::write(&index, "two\n").unwrap();
            fs::write(index.with_file_name("new"), "new\n").unwrap();
            fs::write(&changed, "").unwrap();
            true
        });
        let run = fixture.run(&line);
        ended.store(true, Ordering::SeqCst);
        (run, changer.join().unwrap())
    });
    assert!(did_change, "the call never read the index: {run:#?}");
    // The kernel calls changes below a mounted overlay undefined; the call must still
    // end normally and see one of the two contents.
    run.expect_status(0);
    let lines: Vec<&str> = run.stdout.lines().collect();
    assert_eq!(lines.first(), Some(&"one"), "{run:#?}");
    assert!(matches!(lines.get(1), Some(&("one" | "two"))), "{run:#?}");
}

#[test]
fn exit_child_survivor_cannot_read_next_input() {
    let ready = sandbox_or_skip!();
    need!("setsid");
    let mut fixture = Fixture::new(&ready);
    fixture.spec.launch = SpecLaunch::Unsandboxed;
    // The line waits until both jobs run sleep: a call that ends first stops a forked
    // zsh, which the summary names `zsh`, not `sleep`. A setsid that forked is gone.
    let line = "setsid sleep 9876.3 & a=$!; sleep 9876.4 &! b=$!; \
                while [[ ( -e /proc/$a && $(</proc/$a/comm) != sleep ) \
                || $(</proc/$b/comm) != sleep ]]; do :; done; print started";
    let run = fixture.run(line);
    run.expect_status(0);
    let alive = |marker: &str| {
        fs::read_dir("/proc").unwrap().flatten().any(|entry| {
            let cmdline = fs::read(entry.path().join("cmdline")).unwrap_or_default();
            String::from_utf8_lossy(&cmdline).replace('\0', " ").contains(marker)
        })
    };
    assert!(!alive("sleep 9876.3") && !alive("sleep 9876.4"), "a process outlived the call");
    let stopped = &run.result().summary.background_stopped;
    assert!(stopped.contains(&"sleep".to_owned()), "{run:#?}");
    assert!(!run.result().summary.confined);
}

#[test]
fn exit_child_does_not_source_state() {
    let ready = sandbox_or_skip!();
    let mut fixture = Fixture::new(&ready);
    fixture.run("export MADE_INSIDE=1; made_inside() { print evil }").expect_status(0);
    let state = fs::read_to_string(fixture.spec.runtime.shell_dir.join("state.zsh")).unwrap();
    assert!(state.contains("MADE_INSIDE"), "{state}");
    fixture.spec.launch = SpecLaunch::Unsandboxed;
    let run = fixture.run("print ${MADE_INSIDE-unset}; whence made_inside || print no-function");
    assert!(run.stdout.contains("unset"), "{run:#?}");
    assert!(run.stdout.contains("no-function"), "{run:#?}");
}

#[test]
fn approved_sudo_runs_in_exit_child_with_relay() {
    // A fake sudo that reads a password: the exit child keeps the terminal's input.
    let ready = sandbox_or_skip!();
    let mut fixture = Fixture::new(&ready);
    let bin = fixture.base.join("bin");
    write(&bin.join("sudo"), "#!/bin/sh\nread -r pw\necho \"fake sudo ran $* with $pw\"\n");
    fs::set_permissions(bin.join("sudo"), PermissionsExt::from_mode(0o755)).unwrap();
    let path = format!("{}:{}", bin.display(), crate::support::env_var("PATH").unwrap_or_default());
    fixture.env.insert("PATH".into(), path.into());
    fixture.spec.launch = SpecLaunch::Unsandboxed;
    let password = fixture.base.join("password");
    fs::write(&password, "hunter2\n").unwrap();
    let run = fixture.call("sudo pacman -Syu", &fixture.project, |command| {
        command.stdin(Stdio::from(fs::File::open(&password).unwrap()));
    });
    run.expect_status(0);
    assert!(run.stdout.contains("fake sudo ran pacman -Syu with hunter2"), "{run:#?}");
}

/// The line that the sandboxed command prints before the line of a test runs.
const RUNNING: &str = "efr-test-running";

/// Starts the launcher in a process group of its own with a command that prints
/// [`RUNNING`] and then runs `line`. Sends SIGINT to the group when that line comes on
/// the launcher's stdout, and returns the run and the time from the signal to the end.
///
/// The signal waits for the command and not for a fixed time: bwrap's setup can take
/// longer than any fixed time on a machine under load, and a SIGINT before the command
/// starts is a setup failure (status 125), not an interrupted job.
fn interrupted(fixture: &Fixture, line: &str) -> (Run, Duration) {
    let call_dir = fixture.prepare(&format!("print {RUNNING}; {line}"));
    let mut command = fixture.command(&call_dir, &fixture.project);
    command.process_group(0);
    let mut child = command.spawn().unwrap();
    let group = rustix::process::Pid::from_raw(i32::try_from(child.id()).unwrap()).unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let errors = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = stderr.read_to_string(&mut text);
        text
    });
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut before = String::new();
    loop {
        let mut row = String::new();
        let read = stdout.read_line(&mut row).unwrap();
        assert_ne!(read, 0, "the command ended before it ran: {before:?}");
        if row.trim_end() == RUNNING {
            break;
        }
        before.push_str(&row);
    }
    let signalled = Instant::now();
    rustix::process::kill_process_group(group, rustix::process::Signal::INT).unwrap();
    let mut rest = String::new();
    stdout.read_to_string(&mut rest).unwrap();
    let status = child.wait().unwrap();
    let spent = signalled.elapsed();
    let status = status.code().unwrap_or(-1);
    let stdout = before + &rest;
    (fixture.collect(&call_dir, (status, stdout, errors.join().unwrap())), spent)
}

#[test]
fn ctrl_c_interrupts_sandboxed_job() {
    let ready = sandbox_or_skip!();
    let fixture = Fixture::new(&ready);
    let (run, spent) = interrupted(&fixture, "sleep 30; print not-reached");
    assert!(spent < Duration::from_secs(10), "the job ran on: {spent:?}");
    assert!(!run.stdout.contains("not-reached"));
    run.expect_status(130);
}

#[test]
fn ctrl_c_reaches_child_while_launcher_catches() {
    let ready = sandbox_or_skip!();
    let fixture = Fixture::new(&ready);
    let (run, _) = interrupted(&fixture, "sleep 30");
    // The launcher lived through SIGINT and wrote the result after the child died.
    let result = run.result();
    assert!(result.started);
    assert_eq!(result.status(), 130, "{run:#?}");
    assert_eq!(result.setup_error, None);
}

#[test]
fn sed_i_with_write_grant() {
    let ready = sandbox_or_skip!();
    need!("sed");
    let mut fixture = Fixture::new(&ready);
    let notes = fixture.home.join("notes");
    write(&notes.join("todo.txt"), "alpha\n");
    let line = format!("sed -i s/alpha/beta/ {}/todo.txt", q(&notes));
    assert_ne!(fixture.run(&line).status, 0);
    fixture.grant(Grant::Write { path: notes.clone() });
    fixture.run(&line).expect_status(0);
    assert_eq!(fs::read_to_string(notes.join("todo.txt")).unwrap(), "beta\n");
}

#[test]
fn mv_to_new_dir_with_write_grant() {
    let ready = sandbox_or_skip!();
    let mut fixture = Fixture::new(&ready);
    write(&fixture.project.join("report.txt"), "report\n");
    // efrd makes the new target first, then grants it for this one call.
    let target = fixture.home.join("archive");
    fs::create_dir(&target).unwrap();
    fixture.grant(Grant::Write { path: target.clone() });
    fixture.run(&format!("mv report.txt {}/", q(&target))).expect_status(0);
    assert!(target.join("report.txt").exists());
    assert!(!fixture.project.join("report.txt").exists());
}

#[test]
fn rm_rf_repo_made_in_earlier_call() {
    let ready = sandbox_or_skip!();
    let fixture = Fixture::new(&ready);
    fixture.git_init();
    fixture.run("git init -q made && print x > made/a").expect_status(0);
    fixture.run("rm -rf made").expect_status(0);
    assert!(!fixture.project.join("made").exists());
}

#[test]
fn cwd_in_private_tmp_persists() {
    let ready = sandbox_or_skip!();
    let fixture = Fixture::new(&ready);
    let run = fixture.run("mkdir -p /tmp/work/sub && cd /tmp/work/sub");
    run.expect_status(0);
    assert_eq!(run.result().cwd.as_deref(), Some(Path::new("/tmp/work/sub")));
    assert!(!run.apply_fields().contains(&"cd".to_owned()), "{run:#?}");
    let run = fixture.run("pwd");
    assert_eq!(run.stdout.trim(), "/tmp/work/sub", "{run:#?}");
}

#[test]
fn git_commit_without_m_shows_editor_stub_message() {
    let ready = sandbox_or_skip!();
    let fixture = Fixture::new(&ready);
    fixture.git_init();
    let run = fixture.run("git commit --allow-empty");
    assert_ne!(run.status, 0);
    assert!(run.stderr.contains("there is no editor in the hidden shell"), "{run:#?}");
}

#[test]
fn apply_carries_the_cd_and_promoted_exports() {
    let ready = sandbox_or_skip!();
    let fixture = Fixture::new(&ready);
    let run = fixture.run("mkdir -p sub && cd sub && export RUST_LOG=debug");
    run.expect_status(0);
    let sub = fixture.project.join("sub").display().to_string();
    assert_eq!(run.apply_fields(), ["cd", sub.as_str(), "export", "RUST_LOG", "debug"]);
    assert!(run.result().summary.cwd_changed);
}

#[test]
fn export_promoted_filtered() {
    let ready = sandbox_or_skip!();
    let fixture = Fixture::new(&ready);
    let run = fixture.run(
        "export RUST_LOG=debug PATH=$PWD/bin:$PATH LD_PRELOAD=/tmp/x.so \
         VIRTUAL_ENV=$PWD/.venv GITHUB_TOKEN=x NODE_ENV=production",
    );
    run.expect_status(0);
    let mut exports: Vec<String> =
        run.apply_fields().chunks(3).map(|chunk| chunk.join(" ")).collect();
    exports.sort();
    assert_eq!(exports, ["export NODE_ENV production", "export RUST_LOG debug"]);
    let summary = &run.result().summary;
    assert_eq!(summary.dropped, ["LD_PRELOAD"]);
    for name in ["PATH", "VIRTUAL_ENV", "GITHUB_TOKEN"] {
        assert!(summary.kept_out.contains(&name.to_owned()), "{name}: {summary:#?}");
    }
}

#[test]
fn function_stays_in_sandbox_state() {
    let ready = sandbox_or_skip!();
    let fixture = Fixture::new(&ready);
    let run = fixture.run("hello() { print hello-from-f }; alias ll='ls -l'");
    run.expect_status(0);
    assert!(run.apply_fields().is_empty(), "{run:#?}");
    let state = fs::read_to_string(fixture.spec.runtime.shell_dir.join("state.zsh")).unwrap();
    assert!(state.contains("hello-from-f"), "{state}");
    let run = fixture.run("hello; alias ll");
    run.expect_status(0);
    assert!(run.stdout.contains("hello-from-f"), "{run:#?}");
    assert!(run.stdout.contains("ls -l"), "{run:#?}");
    let run = fixture.run("unfunction hello");
    run.expect_status(0);
    let run = fixture.run("whence hello || print gone");
    assert!(run.stdout.contains("gone"), "{run:#?}");
}

#[test]
fn venv_activate_survives_calls() {
    let ready = sandbox_or_skip!();
    let fixture = Fixture::new(&ready);
    let venv = fixture.project.join(".venv");
    write(
        &venv.join("bin/activate"),
        "export VIRTUAL_ENV=\"$PWD/.venv\"\nexport PATH=\"$VIRTUAL_ENV/bin:$PATH\"\n\
         deactivate() { unset VIRTUAL_ENV }\n",
    );
    write(&venv.join("bin/venv-tool"), "#!/bin/sh\necho venv-tool-ran\n");
    fs::set_permissions(venv.join("bin/venv-tool"), PermissionsExt::from_mode(0o755)).unwrap();
    let run = fixture.run("source .venv/bin/activate");
    run.expect_status(0);
    assert!(run.apply_fields().is_empty(), "the venv reached the trusted shell: {run:#?}");
    let run = fixture.run(
        "venv-tool; print -r -- $VIRTUAL_ENV; whence deactivate >/dev/null && print has-deactivate",
    );
    run.expect_status(0);
    assert!(run.stdout.contains("venv-tool-ran"), "{run:#?}");
    assert!(run.stdout.contains(&venv.display().to_string()), "{run:#?}");
    assert!(run.stdout.contains("has-deactivate"), "{run:#?}");
}

#[test]
fn nonce_not_visible_inside() {
    // The launcher's side of escape_forged_nonce_mark_ignored: no sandboxed process can
    // learn the nonce, so it cannot print the end mark.
    let ready = sandbox_or_skip!();
    let fixture = Fixture::new(&ready);
    let shell_dir = fixture.spec.runtime.shell_dir.clone();
    let run = fixture.run(&format!("cat {}/*/nonce; env; ps -eo args", q(&shell_dir)));
    assert!(!run.stdout.contains("0123456789abcdef0123456789abcdef"), "{run:#?}");
}

#[test]
fn named_project_is_writable_and_another_is_not() {
    let ready = sandbox_or_skip!();
    let mut fixture = Fixture::new(&ready);
    let named = fixture.home.join("other");
    let unnamed = fixture.home.join("third");
    fs::create_dir(&named).unwrap();
    fs::create_dir(&unnamed).unwrap();
    fixture
        .spec
        .write_roots
        .push(WriteRoot { path: named.clone(), kind: WriteRootKind::NamedProject });
    let run = fixture.run(&format!("print a > {}/f; print b > {}/f", q(&named), q(&unnamed)));
    assert!(named.join("f").exists());
    assert!(!unnamed.join("f").exists());
    assert!(run.stderr.contains("read-only file system"), "{run:#?}");
}

#[test]
fn launcher_refuses_a_call_dir_of_another_user_mode() {
    let ready = sandbox_or_skip!();
    let fixture = Fixture::new(&ready);
    let call_dir = fixture.prepare("print ran");
    fs::set_permissions(&call_dir, PermissionsExt::from_mode(0o755)).unwrap();
    let command = fixture.command(&call_dir, &fixture.project);
    let run = fixture.collect(&call_dir, run_with_timeout(command));
    run.expect_status(125);
    assert!(run.result.is_none(), "{run:#?}");
    assert!(run.stderr.contains("other users"), "{run:#?}");
    assert!(!run.stdout.contains("ran"));
}

//! The launcher's job mode (`efr-sbx run --job`, efr's auto spec, section 3.18) on the
//! real bwrap, Landlock and seccomp: no terminal, a session of its own, the lifeline,
//! the priority and limits that every process of the job inherits, the stop, and the
//! floors that only a job gets.
//!
//! A test never sleeps for a job: it waits for a marker file that the job writes, and
//! a job that must outlive the test's checks waits for a marker that the test writes.

use std::fs;
use std::path::Path;
use std::process::Stdio;

use efr_sandbox::{JobPlan, JobStop, MaskKind, SpecLaunch};
use rustix::process::Signal;

use crate::support::{
    Fixture, Run, alive, need, pty, q, sandbox_or_skip, say, wait_until, which, with_fd_7, write,
};

/// The plan of a test's job: ten minutes, nice 19.
fn plan() -> JobPlan {
    JobPlan::new(600, 19)
}

/// The launcher inside the sandbox, which also runs the self-test.
fn inside_launcher(fixture: &Fixture) -> String {
    q(&fixture.spec.runtime.inside_launcher())
}

/// A shell loop that waits until `path` exists: the job's own wait for the test.
fn wait_for(path: &Path) -> String {
    format!("while [[ ! -e {} ]]; do sleep 0.02; done", q(path))
}

fn stopped_by(run: &Run) -> Option<JobStop> {
    run.result().stop
}

/// The nice value and the IO class that the self-test reports in a job.
fn job_priority(run: &Run) -> (i32, i32) {
    let line = run.stdout.lines().find(|line| line.contains("job_priority")).unwrap_or_default();
    let detail = line.split("\"detail\":\"").nth(1).unwrap_or_default();
    let words: Vec<&str> = detail.trim_end_matches(['"', '}']).split(' ').collect();
    let number = |at: usize| words.get(at).and_then(|word| word.parse().ok()).unwrap_or(-100);
    (number(1), number(3))
}

#[test]
fn job_runs_without_pty() {
    let ready = sandbox_or_skip!();
    let fixture = Fixture::new(&ready);
    // efrd never gives the launcher a terminal. Even when one reaches the launcher, on
    // its stderr and on an inherited descriptor, the job has no controlling terminal
    // and no terminal on any standard stream.
    let (master, slave) = pty();
    let line = "[[ -t 0 || -t 1 || -t 2 ]] && print a-tty; \
                print -r -- tty_nr=$(cut -d' ' -f7 /proc/self/stat); \
                : </dev/tty 2>/dev/null && print devtty-opened; print done";
    let call_dir = fixture.prepare_job(line, plan());
    let slave_file = fs::OpenOptions::new().read(true).write(true).open(&slave).unwrap();
    let mut run = fixture.spawn_job_with(&call_dir, &with_fd_7(&slave), |command| {
        command.stderr(Stdio::from(slave_file));
    });
    run.start();
    let run = run.wait(&fixture);
    run.expect_status(0);
    assert!(run.stdout.contains("tty_nr=0\n"), "{run:#?}");
    assert!(run.stdout.contains("done"), "{run:#?}");
    assert!(!run.stdout.contains("a-tty") && !run.stdout.contains("devtty-opened"), "{run:#?}");
    drop(master);
}

#[test]
fn job_stdin_is_dev_null() {
    let ready = sandbox_or_skip!();
    let fixture = Fixture::new(&ready);
    let call_dir = fixture.prepare_job("cat; readlink /proc/self/fd/0", plan());
    let mut run = fixture.spawn_job(&call_dir);
    // The lifeline carries the start byte, and nothing on it reaches the job.
    run.start().send(b"hunter2\n");
    let run = run.wait(&fixture);
    run.expect_status(0);
    assert!(!run.stdout.contains("hunter2"), "{run:#?}");
    assert_eq!(run.stdout.trim(), "/dev/null", "{run:#?}");
}

#[test]
fn job_gets_the_shell_environment_not_the_launchers() {
    let ready = sandbox_or_skip!();
    let mut fixture = Fixture::new(&ready);
    // NOTE: not EFR_ names, which the environment filter removes from every call.
    fixture.env.insert("JOB_FROM_SHELL".into(), "shell".into());
    let line = "print -r -- ${JOB_FROM_SHELL-} ${LAUNCHER_ONLY-}";
    let run = fixture.run_job(line, plan());
    run.expect_status(0);
    assert_eq!(run.stdout.trim(), "shell", "{run:#?}");
}

#[test]
fn job_output_fsize_limited() {
    let ready = sandbox_or_skip!();
    let fixture = Fixture::new(&ready);
    let job = JobPlan { fsize_bytes: 1024 * 1024, ..plan() };
    let big = fixture.project.join("big");
    let line = format!("head -c 3145728 /dev/zero > {}; print -r -- status=$?", q(&big));
    let run = fixture.run_job(&line, job);
    run.expect_status(0);
    // A write past the limit fails with EFBIG; no SIGXFSZ ends the writer.
    assert!(run.stdout.contains("status=1"), "{run:#?}");
    assert_eq!(fs::metadata(&big).unwrap().len(), 1024 * 1024);
}

#[test]
fn job_sigxfsz_is_ignored_in_child() {
    let ready = sandbox_or_skip!();
    let mut fixture = Fixture::new(&ready);
    for launch in [SpecLaunch::Contained, SpecLaunch::Unsandboxed] {
        fixture.spec.launch = launch;
        let run = fixture.run_job("grep SigIgn /proc/self/status", plan());
        run.expect_status(0);
        let mask = run.stdout.trim().rsplit('\t').next().unwrap_or_default().to_owned();
        let mask = u64::from_str_radix(&mask, 16).unwrap();
        // SIGXFSZ is signal 25.
        assert_ne!(mask & (1 << 24), 0, "{launch:?}: {run:#?}");
    }
}

#[test]
fn job_cannot_raise_fsize_limit() {
    let ready = sandbox_or_skip!();
    let fixture = Fixture::new(&ready);
    let job = JobPlan { fsize_bytes: 1024 * 1024, ..plan() };
    let big = fixture.project.join("big");
    let line = format!(
        "ulimit -f unlimited 2>/dev/null; ulimit -H -f unlimited 2>/dev/null; \
         ulimit -f; ulimit -H -f; head -c 2097152 /dev/zero > {}",
        q(&big)
    );
    let run = fixture.run_job(&line, job);
    // zsh counts in blocks of 512 bytes: both limits stay at 1 MiB, and the write
    // past it still fails.
    assert_eq!(run.stdout.lines().take(2).collect::<Vec<_>>(), ["2048", "2048"], "{run:#?}");
    assert_eq!(fs::metadata(&big).unwrap().len(), 1024 * 1024);
}

#[test]
fn job_runs_at_nice_19_and_idle_io() {
    let ready = sandbox_or_skip!();
    let mut fixture = Fixture::new(&ready);
    let parent = rustix::process::getpriority_process(None).unwrap();
    let wanted = parent.max(19);
    let contained = format!(
        "{} self-test --check job_priority --job-priority; cut -d' ' -f19 /proc/self/stat",
        inside_launcher(&fixture)
    );
    let run = fixture.run_job(&contained, plan());
    run.expect_status(0);
    assert_eq!(job_priority(&run), (wanted, 3), "{run:#?}");
    assert!(run.stdout.trim_end().ends_with(&wanted.to_string()), "{run:#?}");
    assert_eq!(run.result().nice, Some(u8::try_from(wanted).unwrap()));
    fixture.spec.launch = SpecLaunch::Unsandboxed;
    let outside =
        format!("{} self-test --check job_priority --job-priority", q(&fixture.ready.bin));
    let run = fixture.run_job(&outside, plan());
    run.expect_status(0);
    assert_eq!(job_priority(&run), (wanted, 3), "{run:#?}");
    if parent == 0 {
        say("strict: the parent's nice value was 0, and the job ran at 19 in the idle class");
    } else {
        say(&format!("not strict: the parent's nice value was {parent}; the job ran at {wanted}"));
    }
}

#[test]
fn job_nice_never_lowered() {
    let ready = sandbox_or_skip!();
    let fixture = Fixture::new(&ready);
    // A launcher at nice 19 and a spec of 10: the job runs at 19, and it runs.
    let job = JobPlan { nice: 10, ..plan() };
    let call_dir = fixture.prepare_job("cut -d' ' -f19 /proc/self/stat", job);
    let nice = [which("nice").into_os_string(), "-n".into(), "19".into()];
    let mut run = fixture.spawn_job_with(&call_dir, &nice, |_| {});
    run.start();
    let run = run.wait(&fixture);
    run.expect_status(0);
    assert_eq!(run.stdout.trim(), "19", "{run:#?}");
    assert_eq!(run.result().nice, Some(19));
}

#[test]
fn job_child_oom_score_adj_is_1000() {
    let ready = sandbox_or_skip!();
    let mut fixture = Fixture::new(&ready);
    for launch in [SpecLaunch::Contained, SpecLaunch::Unsandboxed] {
        fixture.spec.launch = launch;
        let run = fixture.run_job("cat /proc/self/oom_score_adj", plan());
        run.expect_status(0);
        assert_eq!(run.stdout.trim(), "1000", "{launch:?}: {run:#?}");
    }
}

#[test]
fn job_pid_namespace_ends_descendants_on_stop() {
    let ready = sandbox_or_skip!();
    let fixture = Fixture::new(&ready);
    let started = fixture.project.join("started");
    let trapped = fixture.project.join("trapped");
    let line = format!(
        "trap 'print trapped > {}; exit 3' TERM; sleep 9876.3 & print ok > {}; wait",
        q(&trapped),
        q(&started)
    );
    let call_dir = fixture.prepare_job(&line, plan());
    let mut run = fixture.spawn_job(&call_dir);
    run.start();
    wait_until("the job's start", || started.exists());
    run.signal(Signal::TERM);
    let run = run.wait(&fixture);
    // The job's own TERM trap ran: it got the grace, not bwrap's SIGKILL.
    assert!(trapped.exists(), "{run:#?}");
    assert_eq!(stopped_by(&run), Some(JobStop::Signal), "{run:#?}");
    assert_eq!(run.result().exit_code, Some(3), "{run:#?}");
    assert!(!alive("sleep 9876.3"));
}

#[test]
fn job_result_names_the_stop() {
    let ready = sandbox_or_skip!();
    let fixture = Fixture::new(&ready);
    let started = fixture.project.join("started");
    let line = format!("print ok > {}; sleep 9876.8", q(&started));
    for signal in [Signal::TERM, Signal::HUP] {
        let _ = fs::remove_file(&started);
        let call_dir = fixture.prepare_job(&line, plan());
        let mut run = fixture.spawn_job(&call_dir);
        run.start();
        wait_until("the job's start", || started.exists());
        run.signal(signal);
        let run = run.wait(&fixture);
        assert_eq!(stopped_by(&run), Some(JobStop::Signal), "{signal:?}: {run:#?}");
        assert!(run.result().started);
    }
    assert!(!alive("sleep 9876.8"));
}

#[test]
fn job_ends_when_lifeline_closes() {
    let ready = sandbox_or_skip!();
    let fixture = Fixture::new(&ready);
    let started = fixture.project.join("started");
    let line = format!("print ok > {}; sleep 9876.6", q(&started));
    let call_dir = fixture.prepare_job(&line, plan());
    let mut run = fixture.spawn_job(&call_dir);
    run.start();
    wait_until("the job's start", || started.exists());
    // efrd died: its end of the lifeline closed.
    run.close_lifeline();
    let run = run.wait(&fixture);
    assert_eq!(stopped_by(&run), Some(JobStop::Lifeline), "{run:#?}");
    assert!(!alive("sleep 9876.6"));
}

#[test]
fn job_waits_for_start_byte() {
    let ready = sandbox_or_skip!();
    let fixture = Fixture::new(&ready);
    let ran = fixture.project.join("ran");
    let call_dir = fixture.prepare_job(&format!("print ok > {}", q(&ran)), plan());
    let mut run = fixture.spawn_job(&call_dir);
    // efrd went away before it wrote the start byte: nothing starts.
    run.close_lifeline();
    let run = run.wait(&fixture);
    let error = run.result().setup_error.clone().unwrap_or_default();
    assert!(error.contains("start byte"), "{run:#?}");
    assert!(!run.result().started);
    assert!(!run.call_dir.join("started").exists());
    assert!(!ran.exists());
}

#[test]
fn job_deadline_backstop_in_launcher() {
    let ready = sandbox_or_skip!();
    let fixture = Fixture::new(&ready);
    // efrd's limit of 1 s passed and efrd did not stop the job: the launcher's own
    // deadline, 1 s later, ends it.
    let job = JobPlan { max_seconds: 1, backstop_secs: 1, ..plan() };
    let start = std::time::Instant::now();
    let run = fixture.run_job("sleep 9876.7", job);
    assert_eq!(stopped_by(&run), Some(JobStop::Deadline), "{run:#?}");
    assert!(start.elapsed() >= std::time::Duration::from_secs(2));
    assert!(!alive("sleep 9876.7"));
}

#[test]
fn job_does_not_write_state_zsh() {
    let ready = sandbox_or_skip!();
    let fixture = Fixture::new(&ready);
    let run = fixture.run_job("export EFR_TEST_JOB_STATE=1; cd /", plan());
    run.expect_status(0);
    let shell_dir = &fixture.spec.runtime.shell_dir;
    assert!(!shell_dir.join("state.zsh").exists());
    assert!(!shell_dir.join("state.json").exists());
    assert!(!run.call_dir.join("apply").exists());
    assert!(!run.result().state_kept);
}

#[test]
fn job_contained_same_masks_and_roots_as_call() {
    let ready = sandbox_or_skip!();
    let mut fixture = Fixture::new(&ready);
    write(&fixture.home.join(".ssh/id_ed25519"), "PRIVATE KEY\n");
    let ssh = fixture.home.join(".ssh");
    fixture.mask(&ssh, MaskKind::EngineSecret);
    let line = format!(
        "ls -A ~/.ssh | wc -l; print x > {}/f && print project-writable; \
         {{ print x >> ~/.zshrc }} 2>/dev/null || print zshrc-read-only; \
         {{ print x > {}/escape }} 2>/dev/null || print home-read-only; ls -A {}",
        q(&fixture.project),
        q(&fixture.home),
        q(&fixture.runtime),
    );
    let call = fixture.run(&line);
    let job = fixture.run_job(&line, plan());
    call.expect_status(0);
    job.expect_status(0);
    assert_eq!(job.stdout, call.stdout, "call: {call:#?}\njob: {job:#?}");
    assert!(job.stdout.contains("project-writable") && job.stdout.contains("home-read-only"));
    assert!(job.stdout.contains("zshrc-read-only"));
    assert!(job.result().summary.confined);
}

#[test]
fn escape_job_cannot_read_typed_input() {
    let ready = sandbox_or_skip!();
    let fixture = Fixture::new(&ready);
    // A terminal reaches the launcher on a descriptor it inherited; the job never sees
    // it: the launcher marks every descriptor above 2 close-on-exec, gives the job
    // /dev/null as stdin, and leaves it no controlling terminal to open.
    let (master, slave) = pty();
    let started = fixture.project.join("started");
    let typed = fixture.project.join("typed");
    let line = format!(
        "print ok > {}; {}; read -t 1 got </dev/tty 2>/dev/null && print -r -- tty:$got; \
         read -t 1 got && print -r -- stdin:$got; \
         for fd in /proc/self/fd/*; do print -r -- $fd:$(readlink $fd); done",
        q(&started),
        wait_for(&typed),
    );
    let call_dir = fixture.prepare_job(&line, plan());
    let mut run = fixture.spawn_job_with(&call_dir, &with_fd_7(&slave), |_| {});
    run.start();
    wait_until("the job's start", || started.exists());
    rustix::io::write(&master, b"secret\n").unwrap();
    fs::write(&typed, "").unwrap();
    let run = run.wait(&fixture);
    run.expect_status(0);
    assert!(!run.stdout.contains("secret"), "{run:#?}");
    assert!(!run.stdout.contains("/dev/pts") && !run.stdout.contains("ptmx"), "{run:#?}");
    drop(master);
}

#[test]
fn escape_job_cannot_reach_daemon_sock() {
    let ready = sandbox_or_skip!();
    let fixture = Fixture::new(&ready);
    let socket = fixture.runtime.join("ds");
    let _listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
    let line = format!("[[ -e {} ]] && exit 0; ls -A {}; exit 1", q(&socket), q(&fixture.runtime));
    let run = fixture.run_job(&line, plan());
    run.expect_status(1);
    // Only the editor stub comes back into the masked runtime root.
    assert_eq!(run.stdout.trim(), "zsh", "{run:#?}");
}

#[test]
fn job_layers_do_not_block_foreground_call() {
    let ready = sandbox_or_skip!();
    let mut fixture = Fixture::new(&ready);
    let cargo = fixture.home.join(".cargo");
    let source = cargo.join("registry/src/dep/lib.rs");
    write(&source, "pub fn orig() {}\n");
    fixture.cache(&cargo);
    let started = fixture.project.join("started");
    let done = fixture.project.join("done");
    let line = format!(
        "print 'pub fn job() {{}}' > {}; print ok > {}; {}; cat {}",
        q(&source),
        q(&started),
        wait_for(&done),
        q(&source)
    );
    let call_dir = fixture.prepare_job(&line, plan());
    let mut job = fixture.spawn_job(&call_dir);
    job.start();
    wait_until("the job's start", || started.exists());
    // The job holds its own layers; a call of the conversation mounts the
    // conversation's layers over the same cache meanwhile, and neither waits.
    let call =
        fixture.run(&format!("cat {}; print 'pub fn call() {{}}' > {}", q(&source), q(&source)));
    call.expect_status(0);
    assert!(call.stdout.contains("orig"), "the call saw the job's layer: {call:#?}");
    fs::write(&done, "").unwrap();
    let job = job.wait(&fixture);
    job.expect_status(0);
    assert!(job.stdout.contains("job()"), "the job saw the call's layer: {job:#?}");
    assert_eq!(fs::read_to_string(&source).unwrap(), "pub fn orig() {}\n");
    // A job's cache deletes are never listed.
    assert!(job.result().summary.cache_deletes.is_empty());
}

#[test]
fn job_cannot_write_pinned_git_dir() {
    let ready = sandbox_or_skip!();
    let fixture = Fixture::new(&ready);
    fixture.git_init();
    let run = fixture.run_job(
        "print x > .git/evil 2>/dev/null || print evil-refused; \
         print '[alias] x = !true' >> .git/config 2>/dev/null || print config-refused; \
         print x > work.txt && print work-tree-writable",
        plan(),
    );
    run.expect_status(0);
    assert!(run.stdout.contains("evil-refused"), "{run:#?}");
    assert!(run.stdout.contains("config-refused"), "{run:#?}");
    assert!(run.stdout.contains("work-tree-writable"), "{run:#?}");
    assert!(!fixture.project.join(".git/evil").exists());
}

#[test]
fn job_cannot_write_scanned_nested_git_dir() {
    let ready = sandbox_or_skip!();
    let fixture = Fixture::new(&ready);
    let nested = fixture.project.join("vendor/lib");
    fs::create_dir_all(nested.join(".git/hooks")).unwrap();
    fs::write(nested.join(".git/config"), "[core]\n\tbare = false\n").unwrap();
    let hook = nested.join(".git/hooks/pre-commit");
    let line = format!("print evil > {} 2>/dev/null || print hook-refused", q(&hook));
    let run = fixture.run_job(&line, plan());
    run.expect_status(0);
    assert!(run.stdout.contains("hook-refused"), "{run:#?}");
    assert!(!hook.exists());
}

#[test]
fn job_cannot_write_job_output_dir() {
    let ready = sandbox_or_skip!();
    let fixture = Fixture::new(&ready);
    // efrd made $SCRATCH/jobs and writes each job's output there; no contained call
    // can change it.
    let outputs = fixture.spec.runtime.job_outputs();
    fs::create_dir(&outputs).unwrap();
    fs::write(outputs.join("j1.out"), "real output\n").unwrap();
    let run = fixture.run(&format!(
        "print forged > {0}/j1.out 2>/dev/null || print forge-refused; \
         rm -f {0}/j1.out 2>/dev/null || print remove-refused; \
         print x > {0}/j2.out 2>/dev/null || print new-refused",
        q(&outputs)
    ));
    run.expect_status(0);
    for refused in ["forge-refused", "remove-refused", "new-refused"] {
        assert!(run.stdout.contains(refused), "{refused}: {run:#?}");
    }
    assert_eq!(fs::read_to_string(outputs.join("j1.out")).unwrap(), "real output\n");
    assert!(!outputs.join("j2.out").exists());
}

#[test]
fn run_without_job_flag_refuses_job_spec() {
    let ready = sandbox_or_skip!();
    let fixture = Fixture::new(&ready);
    let ran = fixture.project.join("ran");
    let call_dir = fixture.prepare_job(&format!("print ok > {}", q(&ran)), plan());
    let ended = crate::support::run_with_timeout(fixture.command(&call_dir, &fixture.project));
    let run = fixture.collect(&call_dir, ended);
    let error = run.result().setup_error.clone().unwrap_or_default();
    assert!(error.contains("run --job"), "{run:#?}");
    assert!(!ran.exists());
}

#[test]
fn run_with_job_flag_refuses_call_spec() {
    let ready = sandbox_or_skip!();
    let fixture = Fixture::new(&ready);
    let ran = fixture.project.join("ran");
    let call_dir = fixture.prepare(&format!("print ok > {}", q(&ran)));
    let mut run = fixture.spawn_job(&call_dir);
    run.start();
    let run = run.wait(&fixture);
    let error = run.result().setup_error.clone().unwrap_or_default();
    assert!(error.contains("job's spec"), "{run:#?}");
    assert!(!ran.exists());
}

#[test]
fn job_exit_child_survivors_end_at_stop() {
    let ready = sandbox_or_skip!();
    need!("setsid");
    let mut fixture = Fixture::new(&ready);
    fixture.spec.launch = SpecLaunch::Unsandboxed;
    let started = fixture.project.join("started");
    let line = format!("sleep 9876.4 &!; setsid sleep 9876.5 & print ok > {}; wait", q(&started));
    let call_dir = fixture.prepare_job(&line, plan());
    let mut run = fixture.spawn_job(&call_dir);
    run.start();
    wait_until("the job's start", || started.exists());
    // NOTE: `print` runs as soon as zsh forked the two; a stop before they ran `sleep`
    // would find a zsh or a setsid by that name.
    wait_until("both sleeps", || sleeping("9876.4") && sleeping("9876.5"));
    run.signal(Signal::TERM);
    let run = run.wait(&fixture);
    assert_eq!(stopped_by(&run), Some(JobStop::Signal), "{run:#?}");
    let summary = &run.result().summary;
    assert!(!summary.confined);
    // The stop names what it ended, and nothing survived it.
    assert!(summary.background_stopped.contains(&"sleep".to_owned()), "{run:#?}");
    assert!(summary.survivors.is_empty(), "{run:#?}");
    assert!(!alive("sleep 9876.4") && !alive("sleep 9876.5"));
}

#[test]
fn job_exit_child_holds_no_pty_fd() {
    let ready = sandbox_or_skip!();
    let mut fixture = Fixture::new(&ready);
    fixture.spec.launch = SpecLaunch::Unsandboxed;
    let (master, slave) = pty();
    let line = "for fd in /proc/self/fd/*; do print -r -- $fd:$(readlink $fd); done; \
                : </dev/tty 2>/dev/null && print devtty-opened; print done";
    let call_dir = fixture.prepare_job(line, plan());
    let mut run = fixture.spawn_job_with(&call_dir, &with_fd_7(&slave), |_| {});
    run.start();
    let run = run.wait(&fixture);
    run.expect_status(0);
    assert!(run.stdout.contains("done"), "{run:#?}");
    assert!(!run.stdout.contains("/dev/pts") && !run.stdout.contains("ptmx"), "{run:#?}");
    assert!(!run.stdout.contains("devtty-opened"), "{run:#?}");
    drop(master);
}

/// True while a process named `sleep` runs with `seconds` as its argument.
fn sleeping(seconds: &str) -> bool {
    let Ok(entries) = fs::read_dir("/proc") else { return false };
    entries.flatten().any(|entry| {
        let name = fs::read_to_string(entry.path().join("comm")).unwrap_or_default();
        let cmdline = fs::read(entry.path().join("cmdline")).unwrap_or_default();
        name.trim_end() == "sleep"
            && cmdline.split(|byte| *byte == 0).any(|arg| arg == seconds.as_bytes())
    })
}

/// The job's state dir, `$SBX/jobs/<call>`, for its call dir.
fn state_dir(fixture: &Fixture, call_dir: &Path) -> std::path::PathBuf {
    let call = call_dir.file_name().unwrap().to_string_lossy().parse().unwrap();
    fixture.spec.runtime.job_state_dir(call)
}

/// Runs `efr-sbx guard-check` for the job of `call_dir`: its status and stderr.
fn guard_check(fixture: &Fixture, call_dir: &Path) -> (i32, String) {
    let mut command = crate::support::command(&fixture.ready.bin);
    command
        .args(["guard-check", "--call-dir"])
        .arg(call_dir)
        .arg("--state-dir")
        .arg(state_dir(fixture, call_dir))
        .current_dir("/")
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let (status, _, stderr) = crate::support::run_with_timeout(command);
    (status, stderr)
}

/// A job that makes a new repository `sub` with a hook in the project, then waits for
/// `done`; the started job and its call dir.
fn job_that_plants_a_hook(fixture: &Fixture) -> (crate::support::JobRun, std::path::PathBuf) {
    let started = fixture.project.join("started");
    let line = format!(
        "mkdir -p sub/.git/hooks; print '[core]' > sub/.git/config; \
         print '#!/bin/sh' > sub/.git/hooks/pre-commit; print ok > {}; {}",
        q(&started),
        wait_for(&fixture.project.join("done"))
    );
    let call_dir = fixture.prepare_job(&line, plan());
    let mut run = fixture.spawn_job(&call_dir);
    run.start();
    wait_until("the job's start", || started.exists());
    (run, call_dir)
}

#[test]
fn guard_check_quarantines_new_hook() {
    let ready = sandbox_or_skip!();
    let fixture = Fixture::new(&ready);
    let (run, call_dir) = job_that_plants_a_hook(&fixture);
    let hook = fixture.project.join("sub/.git/hooks/pre-commit");
    assert!(hook.exists());
    // The job still runs: the check moves the hook away now, not at the job's end.
    let (status, stderr) = guard_check(&fixture, &call_dir);
    assert_eq!(status, 3, "{stderr}");
    assert!(!hook.exists());
    let report = fs::read_to_string(state_dir(&fixture, &call_dir).join("guard.json")).unwrap();
    assert!(
        report.contains("hook_planted") && report.contains("\"quarantined\": true"),
        "{report}"
    );
    let call = call_dir.file_name().unwrap().to_string_lossy().parse().unwrap();
    let quarantine = fixture.spec.runtime.quarantine(call);
    let index = fs::read_to_string(quarantine.join("entries.json")).unwrap();
    assert!(index.contains("pre-commit"), "{index}");
    // A second check finds nothing new.
    assert_eq!(guard_check(&fixture, &call_dir).0, 0);
    fs::write(fixture.project.join("done"), "").unwrap();
    let run = run.wait(&fixture);
    run.expect_status(0);
}

#[test]
fn guard_check_and_end_guard_take_one_lock() {
    let ready = sandbox_or_skip!();
    let fixture = Fixture::new(&ready);
    let (run, call_dir) = job_that_plants_a_hook(&fixture);
    let hook = fixture.project.join("sub/.git/hooks/pre-commit");
    // Another holder of the job's guard lock, such as the launcher's end guard, keeps
    // the check from moving anything until it lets go.
    let state = state_dir(&fixture, &call_dir);
    let held = fs::File::open(&state).unwrap();
    rustix::fs::flock(&held, rustix::fs::FlockOperation::NonBlockingLockExclusive).unwrap();
    let (status, stderr) = guard_check(&fixture, &call_dir);
    assert_eq!(status, 1, "{stderr}");
    assert!(stderr.contains("lock"), "{stderr}");
    assert!(hook.exists());
    assert!(!state.join("guard.json").exists());
    drop(held);
    assert_eq!(guard_check(&fixture, &call_dir).0, 3);
    assert!(!hook.exists());
    fs::write(fixture.project.join("done"), "").unwrap();
    run.wait(&fixture).expect_status(0);
}

#[test]
fn guard_check_works_from_the_state_dir_alone() {
    let ready = sandbox_or_skip!();
    let fixture = Fixture::new(&ready);
    let call_dir = fixture.prepare_job("true", plan());
    let mut run = fixture.spawn_job(&call_dir);
    run.start();
    run.wait(&fixture).expect_status(0);
    // After a reboot: the call dir in the runtime root is gone, and a hook planted
    // while efrd was down waits. The record on disk says where to look.
    fs::remove_dir_all(&call_dir).unwrap();
    write(&fixture.project.join("late/.git/config"), "[core]\n");
    write(&fixture.project.join("late/.git/hooks/post-checkout"), "#!/bin/sh\n");
    assert_eq!(guard_check(&fixture, &call_dir).0, 3);
    assert!(!fixture.project.join("late/.git/hooks/post-checkout").exists());
}

#[test]
fn exit_child_job_cannot_gain_privileges() {
    let ready = sandbox_or_skip!();
    let mut fixture = Fixture::new(&ready);
    fixture.spec.launch = SpecLaunch::Unsandboxed;
    // NOTE: no real sudo runs, not even one that fails: no_new_privs is what makes
    // sudo, pkexec and every other setuid program run without new rights.
    let run = fixture.run_job("grep -E '^(NoNewPrivs|CapEff):' /proc/self/status", plan());
    run.expect_status(0);
    assert!(run.stdout.contains("NoNewPrivs:\t1"), "{run:#?}");
    if !rustix::process::geteuid().is_root() {
        assert!(run.stdout.contains("CapEff:\t0000000000000000"), "{run:#?}");
    }
}

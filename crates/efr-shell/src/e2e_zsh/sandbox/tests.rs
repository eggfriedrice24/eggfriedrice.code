//! The zsh tests of efr's auto spec, section 16.2, over a real zsh and the fake
//! launcher.

use std::os::unix::fs::PermissionsExt as _;
use std::os::unix::process::ExitStatusExt as _;
use std::path::{Path, PathBuf};

use efr_sandbox::{RecordLimits, Records, SandboxState, parse_records};
use efr_test_support::Wait;
use pretty_assertions::assert_eq;

use super::read;
use crate::e2e_zsh::Zsh;
use crate::run::WRAPPER_CHECK;
use crate::{Completion, Phase};

/// The fields of an apply file.
fn apply(fields: &[&str]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for field in fields {
        bytes.extend_from_slice(field.as_bytes());
        bytes.push(0);
    }
    bytes
}

/// The zsh on the `PATH` that the tests give the shell.
fn zsh_program() -> PathBuf {
    which::which_in("zsh", Some("/usr/bin:/bin"), "/").unwrap()
}

#[tokio::test]
async fn wrapper_types_fixed_line() {
    let Some(zsh) = Zsh::start_sandboxed("wrapper_types_fixed_line") else {
        return;
    };
    let run = zsh.prepare(1);
    let line = "print -r -- from-the-line; print -r -- ${EFR_HIDDEN_SHELL-unset}";
    let result = zsh.run_sandboxed(&run, line).await;

    assert_eq!(result.completion, Completion::Finished, "{result:?}");
    assert_eq!(result.exit_code, Some(0), "{result:?}");
    assert_eq!(result.output, "from-the-line\n1\n");
    assert!(result.sandbox.as_ref().is_some_and(|sandbox| sandbox.started), "{result:?}");
    assert_eq!(read(&run.dir, "line").as_deref(), Some(line));
    let args = format!("run\n--call-dir\n{}\n", run.dir.display());
    assert_eq!(read(&run.dir, "fake-args"), Some(args));

    // The shell got the fixed line and never the model's. The line editor draws the
    // pasted line again as plain text, so the text without escapes holds it whole.
    let pty_id = zsh.sessions.state(zsh.conversation).await.unwrap().pty_id;
    let recording = crate::capture::clean(&zsh.recorded.stream(pty_id));
    assert!(recording.contains(&format!("{WRAPPER_CHECK}{}", run.call)), "{recording:?}");
    assert!(!recording.contains("print -r -- from-the-line"), "{recording:?}");

    // The child shell reported its state on descriptor 3.
    let records = std::fs::read(run.dir.join("records")).unwrap();
    let records = parse_records(&records, &RecordLimits::default()).unwrap();
    assert_eq!(records.status, Some(0));
    assert_eq!(records.cwd.as_deref(), Some(zsh.start_dir()));
}

#[tokio::test]
async fn wrapper_check_fails_on_redefinition() {
    let Some(zsh) = Zsh::start_sandboxed("wrapper_check_fails_on_redefinition") else {
        return;
    };
    // A line of another mode replaces the apply function.
    zsh.run_plain("_efr_hs_sbx_apply() { : }").await;
    let run = zsh.prepare(1);
    let result = zsh.run_sandboxed(&run, "touch never").await;

    assert_eq!(result.completion, Completion::SandboxFailed, "{result:?}");
    assert_eq!(result.exit_code, Some(1));
    assert_eq!(read(&run.dir, "fake-args"), None, "the launcher ran");
    assert!(!zsh.start_dir().join("never").exists());
}

#[tokio::test]
async fn wrapper_check_fails_on_alias() {
    let Some(zsh) = Zsh::start_sandboxed("wrapper_check_fails_on_alias") else {
        return;
    };
    // zsh expands an ordinary alias named `[[` before the fixed line's check runs.
    let marker = zsh.start_dir().join("alias-ran");
    let alias = format!("alias '[['=': > {}; [['", marker.display());
    zsh.run_plain(&alias).await;
    let run = zsh.prepare(1);
    let result = zsh.run_sandboxed(&run, "true").await;

    assert!(!marker.exists(), "the alias ran in the trusted shell: {result:?}");
    // The clear key removed the alias, so the check ran as written and passed.
    assert_eq!(result.completion, Completion::Finished, "{result:?}");
    let check =
        zsh.run_plain("builtin alias -- '[[' >/dev/null && print present || print absent").await;
    assert_eq!(check.output, "absent\n");
}

#[tokio::test]
async fn wrapper_check_fails_on_builtin_function() {
    let Some(zsh) = Zsh::start_sandboxed("wrapper_check_fails_on_builtin_function") else {
        return;
    };
    // A function named builtin that still runs the command, so the shell's own hooks
    // keep working, and one named command.
    zsh.run_plain("builtin() { \"$@\" }").await;
    let first = zsh.prepare(1);
    let result = zsh.run_sandboxed(&first, "true").await;
    assert_eq!(result.completion, Completion::SandboxFailed, "{result:?}");
    assert_eq!(read(&first.dir, "fake-args"), None, "the launcher ran");

    zsh.run_plain("unfunction builtin; command() { print -r -- shadowed }").await;
    let second = zsh.prepare(2);
    let result = zsh.run_sandboxed(&second, "true").await;
    assert_eq!(result.completion, Completion::SandboxFailed, "{result:?}");
    assert!(!result.output.contains("shadowed"), "{result:?}");
    assert_eq!(read(&second.dir, "fake-args"), None, "the launcher ran");

    // With both gone, the check passes again.
    zsh.run_plain("unfunction command").await;
    let third = zsh.prepare(3);
    assert_eq!(zsh.run_sandboxed(&third, "true").await.completion, Completion::Finished);
}

#[tokio::test]
async fn missing_integration_fails_closed() {
    let Some(zsh) = Zsh::start_in("missing_integration_fails_closed", |config, root| {
        // A shell without the integration: no marks, no wrapper.
        config.program = Some(PathBuf::from("/bin/sh"));
        config.sandbox_dir = Some(root.join("sbx"));
        config.sandbox_launcher = Some(root.join("bin/efr-sbx"));
    }) else {
        return;
    };
    let run = zsh.prepare(1);
    let result = zsh.run_sandboxed(&run, "touch never").await;
    assert_eq!(result.completion, Completion::SandboxFailed, "{result:?}");
    let pty_id = zsh.sessions.state(zsh.conversation).await.unwrap().pty_id;
    let recording = String::from_utf8_lossy(&zsh.recorded.stream(pty_id)).into_owned();
    assert!(!recording.contains("_efr_hs_sbx"), "the wrapper line was typed: {recording:?}");

    // A zsh without the integration runs the fixed line as `command not found`.
    let marker = zsh.start_dir().join("ran");
    let line = format!("{WRAPPER_CHECK}{}", run.call);
    let output = efr_stdx::process::command(zsh_program(), zsh.start_dir())
        .args(["-f", "-c", &line])
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .output()
        .await
        .unwrap();
    assert_eq!(output.status.code(), Some(127));
    assert!(String::from_utf8_lossy(&output.stderr).contains("_efr_hs_sbx"), "{output:?}");
    assert!(!marker.exists());
}

#[tokio::test]
async fn apply_cd_and_export() {
    let Some(zsh) = Zsh::start_sandboxed("apply_cd_and_export") else {
        return;
    };
    let target = zsh.dir("work/sub");
    let marker = zsh.start_dir().join("chpwd-ran");
    // A chpwd function from another mode's line; the apply file's cd runs no hook.
    zsh.run_plain(&format!("export GONE=1; chpwd() {{ : > '{}' }}", marker.display())).await;
    let run = zsh.prepare(1);
    let target_text = target.to_string_lossy().into_owned();
    std::fs::write(
        run.dir.join("fake-apply"),
        apply(&["cd", &target_text, "export", "RUST_LOG", "debug", "unset", "GONE"]),
    )
    .unwrap();
    std::fs::write(run.dir.join("fake-cwd"), &target_text).unwrap();
    let result = zsh.run_sandboxed(&run, "true").await;
    assert_eq!(result.completion, Completion::Finished, "{result:?}");
    assert_eq!(result.cwd_after, target);
    let state = zsh.sessions.state(zsh.conversation).await.unwrap();
    assert_eq!(state.cwd, target);
    assert_eq!(state.sandbox_cwd, None);

    let check = zsh.run_plain("print -r -- \"$PWD ${RUST_LOG-unset} ${GONE-unset}\"").await;
    assert_eq!(check.output, format!("{} debug unset\n", target.display()));
    assert!(!marker.exists(), "the cd ran the chpwd hook");
}

#[tokio::test]
async fn interrupt_after_launcher_still_prints_end_mark() {
    let Some(zsh) = Zsh::start_sandboxed("interrupt_after_launcher_still_prints_end_mark") else {
        return;
    };
    // efr's interrupt reaches the shell itself as the launcher ends: an interactive zsh
    // then aborts the rest of the wrapper.
    let target = zsh.dir("work");
    let target_text = target.to_string_lossy().into_owned();
    let run = zsh.prepare(1);
    std::fs::write(run.dir.join("fake-interrupt"), "").unwrap();
    std::fs::write(run.dir.join("fake-apply"), apply(&["cd", &target_text])).unwrap();
    std::fs::write(run.dir.join("fake-cwd"), &target_text).unwrap();
    let request = crate::RunRequest::new("true", zsh.start_dir())
        .with_call(run.call)
        .with_sandbox(Some(run.clone()))
        .with_timeout(std::time::Duration::from_secs(20));
    let result =
        zsh.sessions.run_command(zsh.conversation, request, &mut crate::NoProgress).await.unwrap();
    assert_eq!(result.completion, Completion::Finished, "{result:?}");
    assert_eq!(result.cwd_after, target);

    // The shell is not left behind a call that never ends.
    let check = zsh.run_plain("print -r -- $PWD").await;
    assert_eq!(check.completion, Completion::Finished, "{check:?}");
    assert_eq!(check.output, format!("{}\n", target.display()));
    let next = zsh.prepare(2);
    assert_eq!(zsh.run_sandboxed(&next, "true").await.completion, Completion::Finished);
}

#[tokio::test]
async fn a_launcher_that_sigint_ends_before_it_starts_reports_an_interrupt() {
    let Some(zsh) =
        Zsh::start_sandboxed("a_launcher_that_sigint_ends_before_it_starts_reports_an_interrupt")
    else {
        return;
    };
    // Ctrl+C or efr's interrupt can come before the launcher catches SIGINT. Then the
    // signal ends it before it writes `started` or `result.json`: the user stopped the
    // call, and the sandbox did not fail.
    let run = zsh.prepare(1);
    std::fs::write(run.dir.join("fake-sigint"), "").unwrap();
    let result = zsh.run_sandboxed(&run, "touch never").await;
    assert_eq!(result.completion, Completion::Finished, "{result:?}");
    assert_eq!(result.exit_code, Some(130), "{result:?}");
    let sandbox = result.sandbox.as_ref().unwrap();
    assert_eq!(sandbox.signal, Some(2), "{result:?}");
    assert!(sandbox.setup_error.is_none() && sandbox.launch_error.is_none(), "{result:?}");
    assert!(!zsh.start_dir().join("never").exists(), "the line ran");

    // The shell takes the next call.
    let next = zsh.prepare(2);
    assert_eq!(zsh.run_sandboxed(&next, "true").await.completion, Completion::Finished);
}

#[tokio::test]
async fn contained_raw_mode_password_prompt_is_refused() {
    let Some(zsh) = Zsh::start_sandboxed("contained_raw_mode_password_prompt_is_refused") else {
        return;
    };
    // Sandboxed code turns line mode and echo off itself and prints a password prompt:
    // a visible wait that looks secret, which efr never answers in the sandbox.
    let run = zsh.prepare(1);
    let line = "stty -icanon -echo; print -n 'password: '; read -k 8";
    let request = crate::RunRequest::new(line, zsh.start_dir())
        .with_call(run.call)
        .with_sandbox(Some(run.clone()))
        .with_timeout(std::time::Duration::from_secs(600));
    // A client that could answer follows the turn.
    let (mut listener, heard) = crate::testing::listener(true);
    let sessions = zsh.sessions.clone();
    let conversation = zsh.conversation;
    let handle =
        tokio::spawn(
            async move { sessions.run_command(conversation, request, &mut listener).await },
        );
    let shown = Wait::new("the password prompt on the screen")
        .until_some_async(async || {
            let screen = zsh.sessions.screen(zsh.conversation)?;
            let capture = screen.snapshot(0).await.ok()?;
            let rows = capture.snapshot.rows.iter().map(efr_screen::row_text);
            rows.into_iter().any(|row| row.contains("password:")).then_some(())
        })
        .await;
    if let Err(error) = shown {
        let pty_id = zsh.sessions.state(zsh.conversation).await.unwrap().pty_id;
        panic!("{error:?}: {}", String::from_utf8_lossy(&zsh.recorded.stream(pty_id)));
    }
    for _ in 0..10 {
        if handle.is_finished() {
            break;
        }
        let before = zsh.clock.pending_sleeps();
        zsh.clock.advance(std::time::Duration::from_secs(1));
        let clock = zsh.clock.clone();
        Wait::new("the next look or the end of the run")
            .until(|| handle.is_finished() || clock.pending_sleeps() >= before)
            .await
            .unwrap();
    }
    let result = handle.await.unwrap().unwrap();
    assert_eq!(result.completion, Completion::Unanswered, "{result:?}");
    assert_eq!(
        *heard.inputs.borrow(),
        [efr_protocol::InputWait::Visible, efr_protocol::InputWait::None]
    );
    assert_eq!(*heard.secrets.borrow(), [true, false]);

    // The interrupted call frees the shell, and its modes come back.
    let next = zsh.prepare(2);
    assert_eq!(zsh.run_sandboxed(&next, "true").await.completion, Completion::Finished);
}

#[tokio::test]
async fn apply_rejects_bad_names() {
    let Some(zsh) = Zsh::start_sandboxed("apply_rejects_bad_names") else {
        return;
    };
    let run = zsh.prepare(1);
    let fields = [
        "export",
        "PATH",
        "/evil",
        "export",
        "LD_PRELOAD",
        "/evil.so",
        "export",
        "BAD-NAME",
        "x",
        "export",
        "GIT_DIR",
        "/evil",
        "unset",
        "HOME",
        "unset",
        "PATH",
        "cd",
        "/tmp",
        "cd",
        "relative",
        "export",
        "OK_NAME",
        "fine",
        "bogus",
        "export",
        "AFTER",
        "x",
    ];
    std::fs::write(run.dir.join("fake-apply"), apply(&fields)).unwrap();
    let result = zsh.run_sandboxed(&run, "true").await;
    assert_eq!(result.completion, Completion::Finished, "{result:?}");

    let check = zsh
        .run_plain(
            "print -r -- \"$PATH|${LD_PRELOAD-unset}|${GIT_DIR-unset}|$HOME|$PWD|${OK_NAME-unset}|${AFTER-unset}\"",
        )
        .await;
    assert_eq!(
        check.output,
        format!(
            "/usr/bin:/bin|unset|unset|{}|{}|fine|unset\n",
            zsh.home().display(),
            zsh.start_dir().display()
        )
    );
    let env = zsh.run_plain("env | grep -c BAD-NAME").await;
    assert_eq!(env.output, "0\n");
}

#[tokio::test]
async fn hooks_stripped_in_hidden_shell() {
    let Some(zsh) = Zsh::start_sandboxed("hooks_stripped_in_hidden_shell") else {
        return;
    };
    let marks = zsh.dir("marks");
    let zshrc = format!(
        r#"efr_test_mark() {{ : > '{marks}'/$1 }}
precmd() {{ efr_test_mark precmd-function }}
efr_test_precmd() {{ efr_test_mark precmd }}
efr_test_preexec() {{ efr_test_mark preexec }}
efr_test_chpwd() {{ efr_test_mark chpwd }}
efr_test_periodic() {{ efr_test_mark periodic }}
efr_test_history() {{ efr_test_mark history }}
precmd_functions+=(efr_test_precmd)
preexec_functions+=(efr_test_preexec)
chpwd_functions+=(efr_test_chpwd)
periodic_functions+=(efr_test_periodic)
PERIOD=1
zshaddhistory_functions+=(efr_test_history)
efr_test_widget() {{ efr_test_mark zle-hook }}
zle -N efr_test_widget
autoload -Uz add-zle-hook-widget
add-zle-hook-widget line-init efr_test_widget
zle-line-finish() {{ efr_test_mark zle-widget }}
zle -N zle-line-finish
setopt prompt_subst
PROMPT='$(efr_test_mark prompt)custom> '
RPROMPT='right'
"#,
        marks = marks.display()
    );
    std::fs::write(zsh.home().join(".zshrc"), zshrc).unwrap();
    // The first prompt comes before the integration can act; whatever ran then is
    // cleared, and from here on nothing of the user's may run.
    zsh.run_plain("true").await;
    for entry in std::fs::read_dir(&marks).unwrap() {
        std::fs::remove_file(entry.unwrap().path()).unwrap();
    }
    let elsewhere = zsh.dir("elsewhere");
    zsh.run_plain(&format!("cd '{}'", elsewhere.display())).await;
    let run = zsh.prepare(1);
    zsh.run_sandboxed(&run, "true").await;
    let state = zsh
        .run_plain(
            "print -r -- \"$precmd_functions|$preexec_functions|$chpwd_functions|$#periodic_functions|$#zshaddhistory_functions|${+functions[precmd]}|$PS1|$RPS1|$options[promptsubst]\"",
        )
        .await;
    assert_eq!(state.output, "_efr_hs_precmd|_efr_hs_preexec|_efr_hs_report_pwd|0|0|0|%# ||off\n");
    let ran: Vec<String> = std::fs::read_dir(&marks)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(ran, Vec::<String>::new(), "user hooks ran");
    // The plain prompt still carries the marks.
    let sessions = zsh.sessions.clone();
    let conversation = zsh.conversation;
    Wait::new("a ready prompt")
        .until_some_async(async || {
            let phase = sessions.state(conversation).await.unwrap().phase;
            (phase == Phase::Ready).then_some(())
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn ttyctl_frozen() {
    let Some(zsh) = Zsh::start_sandboxed("ttyctl_frozen") else {
        return;
    };
    let frozen = zsh.run_plain("ttyctl").await;
    assert_eq!(frozen.output, "tty is frozen\n");
    // A program that leaves echo off does not change how the shell reads the next line.
    zsh.run_plain("stty -echo").await;
    let after = zsh
        .run_plain("if stty -a | grep -qw -- -echo; then print -r -- off; else print -r -- on; fi")
        .await;
    assert_eq!(after.output, "on\n");
}

#[tokio::test]
async fn the_snapshot_carries_the_shells_functions_aliases_and_options() {
    let Some(zsh) =
        Zsh::start_sandboxed("the_snapshot_carries_the_shells_functions_aliases_and_options")
    else {
        return;
    };
    zsh.run_plain(
        "greet() { print -r -- \"hi $1\" }; alias ll='print -r -- listed'; setopt extended_glob",
    )
    .await;
    // A completion function that zsh has not loaded yet stays out; a loaded `_*` helper
    // and another function that is not loaded yet stay in.
    zsh.run_plain("autoload -Uz _mine_stub zmv; _mine_helper() { print -r -- helped }").await;
    // The trap function of a signal that stops a call stays out: in the child, it could
    // catch Ctrl+\ while the child replays the snapshot.
    zsh.run_plain("TRAPQUIT() { return 0 }").await;
    let first = zsh.prepare(1);
    let result = zsh
        .run_sandboxed(&first, "greet you; ll; [[ abc == a(#c1)bc ]] && print -r -- globbed")
        .await;
    assert_eq!(result.output, "hi you\nlisted\nglobbed\n");
    let snapshot = zsh.sandbox_dir().join("snapshot.zsh");
    let text = std::fs::read_to_string(&snapshot).unwrap();
    assert!(!text.contains("_efr_hs_"), "efr's own functions are in the snapshot");
    assert!(!text.contains("_mine_stub"), "{text}");
    assert!(text.contains("_mine_helper") && text.contains("zmv"), "{text}");
    assert!(!text.contains("TRAPQUIT"), "{text}");

    // No other line ran since: the snapshot is not written again.
    std::fs::write(&snapshot, format!("{text}\n# kept\n")).unwrap();
    let second = zsh.prepare(2);
    zsh.run_sandboxed(&second, "true").await;
    assert!(std::fs::read_to_string(&snapshot).unwrap().ends_with("# kept\n"));

    // Another line ran: it is written again, and compiled once it is above 4 KiB.
    zsh.run_plain("eval \"big() { : '$(printf %05000d 0)' }\"").await;
    let third = zsh.prepare(3);
    zsh.run_sandboxed(&third, "_mine_helper; true").await;
    let text = std::fs::read_to_string(&snapshot).unwrap();
    assert!(!text.contains("# kept"));
    assert!(text.len() > 4096);
    assert!(zsh.sandbox_dir().join("snapshot.zsh.zwc").exists());
}

#[tokio::test]
async fn the_snapshot_replays_a_function_named_like_a_default_alias() {
    let Some(zsh) =
        Zsh::start_sandboxed("the_snapshot_replays_a_function_named_like_a_default_alias")
    else {
        return;
    };
    // zsh -f starts with run-help=man and which-command=whence; Ubuntu's zshrc makes
    // run-help a function.
    // Two lines: zsh expands the aliases of a whole line before it runs any of it.
    zsh.run_plain("unalias run-help which-command 2>/dev/null").await;
    zsh.run_plain("run-help() { print -r -- own-help }").await;
    let run = zsh.prepare(1);
    let line = "run-help; (( ${+aliases[which-command]} )) || print -r -- no-which";
    let result = zsh.run_sandboxed(&run, line).await;
    assert_eq!(result.output, "own-help\nno-which\n");
}

#[tokio::test]
async fn the_child_reports_the_state_its_line_leaves() {
    let Some(zsh) = Zsh::start_sandboxed("the_child_reports_the_state_its_line_leaves") else {
        return;
    };
    // The integration files are installed with the first shell.
    zsh.run_plain("true").await;
    let child = zsh.root.path().join("zsh").join(crate::integration::CHILD_FILE);
    let dir = zsh.dir("child");
    std::fs::write(
        dir.join("snapshot.zsh"),
        "greet() { print -r -- hi }\nalias gone='print gone'\n",
    )
    .unwrap();
    let mut state = SandboxState::default();
    state.apply(&Records {
        exports: vec![("FROMSTATE".to_owned(), "it's 1".to_owned())],
        functions: vec![("fromstate".to_owned(), "print -r -- \"from $FROMSTATE\"".to_owned())],
        ..Records::default()
    });
    std::fs::write(dir.join("state.zsh"), state.render()).unwrap();
    std::fs::write(
        dir.join("line"),
        "fromstate; greet; cd /; export NEWVAR='a b'; unset FROMSTATE; newfn() { print new }; \
         unfunction greet; unalias gone; alias newal='ls -l'; exit 3",
    )
    .unwrap();
    let output = run_child(&child, &dir).await;
    assert_eq!(output.status.code(), Some(3), "{output:?}");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "from it's 1\nhi\n",
        "{output:?}\n{}",
        state.render()
    );

    let records = std::fs::read(dir.join("records")).unwrap();
    let records = parse_records(&records, &RecordLimits::default()).unwrap();
    assert_eq!(records.cwd.as_deref(), Some(Path::new("/")));
    assert_eq!(records.exports, [("NEWVAR".to_owned(), "a b".to_owned())]);
    assert_eq!(records.unsets, ["FROMSTATE"]);
    assert_eq!(records.functions.len(), 1, "{records:?}");
    assert_eq!(records.functions[0].0, "newfn");
    assert_eq!(records.removed_functions, ["greet"]);
    assert_eq!(records.aliases, [("newal".to_owned(), "ls -l".to_owned())]);
    assert_eq!(records.removed_aliases, ["gone"]);
    assert_eq!(records.status, Some(3));

    // A line that changes nothing reports nothing but its directory and status.
    std::fs::write(dir.join("line"), "greet >/dev/null; true").unwrap();
    let output = run_child(&child, &dir).await;
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let records = std::fs::read(dir.join("records")).unwrap();
    let records = parse_records(&records, &RecordLimits::default()).unwrap();
    assert_eq!(
        (records.exports, records.unsets, records.functions, records.removed_functions),
        (vec![], vec![], vec![], vec![])
    );
    assert_eq!((records.aliases, records.removed_aliases), (vec![], vec![]));

    // The exit child gets no state.
    std::fs::write(dir.join("line"), "fromstate").unwrap();
    std::fs::rename(dir.join("state.zsh"), dir.join("state.zsh.kept")).unwrap();
    let output = run_child(&child, &dir).await;
    assert_eq!(output.status.code(), Some(127), "fromstate is not defined without the state");

    // A function or alias that an earlier call removed is gone, also one of the
    // snapshot, whatever its name.
    let mut state = SandboxState::default();
    state.apply(&Records {
        removed_functions: vec!["greet".to_owned()],
        removed_aliases: vec!["gone".to_owned()],
        functions: vec![("odd-name.x".to_owned(), "print -r -- odd".to_owned())],
        ..Records::default()
    });
    std::fs::write(dir.join("state.zsh"), state.render()).unwrap();
    std::fs::write(
        dir.join("line"),
        "(( ${+functions[greet]} )) || print -r -- no-greet; \
         (( ${+aliases[gone]} )) || print -r -- no-gone; odd-name.x",
    )
    .unwrap();
    let output = run_child(&child, &dir).await;
    assert_eq!(String::from_utf8_lossy(&output.stdout), "no-greet\nno-gone\nodd\n", "{output:?}");
}

#[tokio::test]
async fn a_signal_before_or_during_the_line_stops_it() {
    let Some(zsh) = Zsh::start_sandboxed("a_signal_before_or_during_the_line_stops_it") else {
        return;
    };
    // The integration files are installed with the first shell.
    zsh.run_plain("true").await;
    let child = zsh.root.path().join("zsh").join(crate::integration::CHILD_FILE);
    let dir = zsh.dir("child");
    // The child replays the snapshot last before the line, so a signal that the snapshot
    // sends to its own shell comes in that window every time. A zsh that is not
    // interactive ignores SIGQUIT: the line ran after Ctrl+\ there.
    std::fs::write(dir.join("line"), "print -r -- ran").unwrap();
    for (signal, status) in [("INT", 130), ("QUIT", 131)] {
        let snapshot = format!("ulimit -c 0\nbuiltin kill -s {signal} $$\n");
        std::fs::write(dir.join("snapshot.zsh"), snapshot).unwrap();
        let output = run_child(&child, &dir).await;
        assert_eq!(output.status.code(), Some(status), "{signal}: {output:?}");
        assert_eq!(String::from_utf8_lossy(&output.stdout), "", "{signal}: the line ran");
        let records = std::fs::read(dir.join("records")).unwrap();
        let records = parse_records(&records, &RecordLimits::default()).unwrap();
        assert_eq!(records.status, Some(status), "{signal}");
    }

    // During the line, either signal ends the shell, also when the snapshot or the state
    // has a trap function that returns 0 and so would let the line go on.
    std::fs::write(dir.join("snapshot.zsh"), "ulimit -c 0\nTRAPINT() { return 0 }\n").unwrap();
    std::fs::write(dir.join("state.zsh"), "TRAPQUIT() { return 0 }\n").unwrap();
    for (signal, number) in [("INT", 2), ("QUIT", 3)] {
        std::fs::write(dir.join("line"), format!("kill -s {signal} $$; print -r -- ran")).unwrap();
        let output = run_child(&child, &dir).await;
        assert_eq!(output.status.signal(), Some(number), "{signal}: {output:?}");
        assert_eq!(String::from_utf8_lossy(&output.stdout), "", "{signal}: the line went on");
    }
}

/// Runs the child script as `efr-sbx` does, with the records on descriptor 3.
async fn run_child(child: &Path, dir: &Path) -> std::process::Output {
    let script = r#"exec zsh -f "$1" "$2" 3>"$3""#;
    efr_stdx::process::command(Path::new("/bin/sh"), dir)
        .arg("-c")
        .arg(script)
        .arg("sh")
        .arg(child)
        .arg(dir)
        .arg(dir.join("records"))
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .output()
        .await
        .unwrap()
}

/// The entries of the shell's `env` file, once one holds `wanted`.
async fn env_entries(zsh: &Zsh, wanted: &str) -> Vec<String> {
    let file = zsh.sandbox_dir().join("env");
    Wait::new("the env file")
        .until_some(|| {
            let bytes = std::fs::read(&file).ok()?;
            let entries: Vec<String> = bytes
                .split(|byte| *byte == 0)
                .filter(|entry| !entry.is_empty())
                .map(|entry| String::from_utf8_lossy(entry).into_owned())
                .collect();
            entries.iter().any(|entry| entry == wanted).then_some(entries)
        })
        .await
        .unwrap()
}

#[tokio::test]
async fn e2e_zsh_integration_writes_env_file() {
    let Some(zsh) = Zsh::start_sandboxed("e2e_zsh_integration_writes_env_file") else {
        return;
    };
    zsh.run_plain("export JOB_VAR='two words'; not_exported=1").await;
    let entries = env_entries(&zsh, "JOB_VAR=two words").await;
    assert!(entries.contains(&"PATH=/usr/bin:/bin".to_owned()), "{entries:?}");
    assert!(!entries.iter().any(|entry| entry.starts_with("not_exported=")), "{entries:?}");
    // efr's own state stays out: the integration took its variables out of the
    // environment.
    assert!(!entries.iter().any(|entry| entry.starts_with("_EFR_HS_")), "{entries:?}");
    let file = zsh.sandbox_dir().join("env");
    let mode = std::fs::metadata(&file).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    // A change is written at the next prompt.
    zsh.run_plain("export JOB_VAR=changed").await;
    let entries = env_entries(&zsh, "JOB_VAR=changed").await;
    assert!(!entries.contains(&"JOB_VAR=two words".to_owned()), "{entries:?}");
}

#[tokio::test]
async fn e2e_zsh_snapshot_written_at_first_prompt() {
    let Some(zsh) = Zsh::start_sandboxed("e2e_zsh_snapshot_written_at_first_prompt") else {
        return;
    };
    std::fs::write(zsh.home().join(".zshrc"), "from_rc() { print -r -- rc }\n").unwrap();
    // No sandboxed call runs: a background job may start before the first one.
    zsh.run_plain("true").await;
    let snapshot = zsh.sandbox_dir().join("snapshot.zsh");
    let holds = |text: &'static str| {
        let snapshot = snapshot.clone();
        move || std::fs::read_to_string(&snapshot).is_ok_and(|held| held.contains(text))
    };
    Wait::new("the snapshot of the first prompt").until(holds("from_rc")).await.unwrap();
    // A line that changes the functions makes it stale; the next prompt writes it again.
    zsh.run_plain("late_fn() { : }").await;
    Wait::new("the snapshot of a later prompt").until(holds("late_fn")).await.unwrap();
    assert!(!zsh.sandbox_dir().join("snapshot.zsh.tmp").exists());
}

#[tokio::test]
async fn e2e_zsh_env_written_when_dir_made_before_spawn() {
    let Some(zsh) = Zsh::start_sandboxed("e2e_zsh_env_written_when_dir_made_before_spawn") else {
        return;
    };
    assert!(!zsh.sandbox_dir().exists());
    zsh.run_plain("true").await;
    // efr made the shell's sandbox dir before the shell started, so its first prompt
    // wrote both files with no sandboxed call.
    let mode = std::fs::metadata(zsh.sandbox_dir()).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o700);
    env_entries(&zsh, "PATH=/usr/bin:/bin").await;
    assert!(zsh.sandbox_dir().join("snapshot.zsh").exists());
}

/// The launcher next to the child script in a call of the mount view: it prints what
/// it was asked, and like the real one it exits 2 for an operand, or for every call
/// when the dir holds `refuse`.
const MOUNTS_LAUNCHER: &str = r#"#!/bin/sh
[ "$1" = mounts ] || exit 9
[ -e "$(dirname "$0")/refuse" ] && exit 2
for arg in "$@"; do
  case $arg in -*|mounts|df|findmnt-df) ;; *) exit 2 ;; esac
done
shift
echo "snapshot $*"
"#;

/// A dir for the child of a mount view call: the snapshot's two files, the launcher,
/// and a `bin` dir with a `df` and a `findmnt` that print their arguments.
fn mounts_dir(dir: &Path) {
    std::fs::write(dir.join("mounts"), "1 0 8:1 / / rw - ext4 /dev/sda1 rw\n").unwrap();
    std::fs::write(dir.join("mounts.json"), r#"{"taken_at":0,"sizes":{}}"#).unwrap();
    let executable = |path: &Path, text: &str| {
        std::fs::write(path, text).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    };
    executable(&dir.join("efr-sbx"), MOUNTS_LAUNCHER);
    std::fs::create_dir_all(dir.join("bin")).unwrap();
    for program in ["df", "findmnt"] {
        executable(
            &dir.join("bin").join(program),
            &format!("#!/bin/sh\necho \"real {program} $*\"\n"),
        );
    }
    std::fs::write(dir.join("snapshot.zsh"), "").unwrap();
}

/// Runs the child script on `dir` with `line`, with the fake programs first on the
/// `PATH`; returns its output and its records.
async fn run_mount_line(child: &Path, dir: &Path, line: &str) -> (String, Records) {
    std::fs::write(dir.join("line"), line).unwrap();
    let script = r#"exec zsh -f "$1" "$2" 3>"$3""#;
    let output = efr_stdx::process::command(Path::new("/bin/sh"), dir)
        .arg("-c")
        .arg(script)
        .arg("sh")
        .arg(child)
        .arg(dir)
        .arg(dir.join("records"))
        .env_clear()
        .env("PATH", format!("{}:/usr/bin:/bin", dir.join("bin").display()))
        .output()
        .await
        .unwrap();
    let records = std::fs::read(dir.join("records")).unwrap();
    let records = parse_records(&records, &RecordLimits::default()).unwrap();
    (String::from_utf8_lossy(&output.stdout).into_owned(), records)
}

/// The child script that the shell installed, and a fresh dir for its calls.
async fn mount_view_child(zsh: &Zsh, name: &str) -> (PathBuf, PathBuf) {
    // The integration files are installed with the first shell.
    zsh.run_plain("true").await;
    let child = zsh.root.path().join("zsh").join(crate::integration::CHILD_FILE);
    let dir = zsh.dir(name);
    mounts_dir(&dir);
    (child, dir)
}

#[tokio::test]
async fn mount_view_df_and_findmnt_route_to_the_snapshot() {
    let Some(zsh) = Zsh::start_sandboxed("mount_view_df_and_findmnt_route_to_the_snapshot") else {
        return;
    };
    let (child, dir) = mount_view_child(&zsh, "mounts").await;
    let table = dir.join("mounts");
    let tab = format!("--tab-file {}", table.display());
    let cases: Vec<(&str, String)> = vec![
        ("df", "snapshot df".to_owned()),
        ("df -hT --total", "snapshot df -hT --total".to_owned()),
        // An operand: the call's own file system, where a write goes.
        ("df -h /tmp", "real df -h /tmp".to_owned()),
        ("df -- -h", "real df -- -h".to_owned()),
        ("findmnt", format!("real findmnt {tab}")),
        ("findmnt -D", "snapshot findmnt-df -D".to_owned()),
        ("findmnt --dfi", "snapshot findmnt-df --dfi".to_owned()),
        ("findmnt -D -t ext4", format!("real findmnt {tab} -D -t ext4")),
        ("findmnt -o SOURCE,TARGET /home", format!("real findmnt {tab} -o SOURCE,TARGET /home")),
        ("findmnt -oFSTYPE", format!("real findmnt {tab} -oFSTYPE")),
        // A table of its own: the real findmnt, as it is.
        ("findmnt -N 1", "real findmnt -N 1".to_owned()),
        ("findmnt --tab-file /etc/fstab", "real findmnt --tab-file /etc/fstab".to_owned()),
        ("findmnt --kern", "real findmnt --kern".to_owned()),
        ("findmnt -ln -s", "real findmnt -ln -s".to_owned()),
        ("findmnt -lp", "real findmnt -lp".to_owned()),
        ("findmnt --poll=mount", "real findmnt --poll=mount".to_owned()),
    ];
    for (line, want) in &cases {
        let (out, _) = run_mount_line(&child, &dir, line).await;
        assert_eq!(out.trim_end(), want, "{line}");
    }
    // The launcher refuses an option: the real commands run.
    std::fs::write(dir.join("refuse"), "").unwrap();
    let (out, _) = run_mount_line(&child, &dir, "df --direct").await;
    assert_eq!(out.trim_end(), "real df --direct");
    let (out, _) = run_mount_line(&child, &dir, "findmnt -D").await;
    assert_eq!(out.trim_end(), format!("real findmnt {tab} -D"));
}

#[tokio::test]
async fn mount_view_missing_snapshot_runs_real_df() {
    let Some(zsh) = Zsh::start_sandboxed("mount_view_missing_snapshot_runs_real_df") else {
        return;
    };
    let (child, dir) = mount_view_child(&zsh, "no-mounts").await;
    std::fs::remove_file(dir.join("mounts")).unwrap();
    let (out, _) = run_mount_line(&child, &dir, "df -h; findmnt -D").await;
    assert_eq!(out, "real df -h\nreal findmnt -D\n");
}

#[tokio::test]
async fn mount_view_user_df_function_wins() {
    let Some(zsh) = Zsh::start_sandboxed("mount_view_user_df_function_wins") else {
        return;
    };
    let (child, dir) = mount_view_child(&zsh, "user-df").await;
    std::fs::write(dir.join("snapshot.zsh"), "df() { print -r -- user df }\n").unwrap();
    let (out, records) = run_mount_line(&child, &dir, "df -h; findmnt -I").await;
    assert_eq!(out, "user df\nsnapshot findmnt-df -I\n");
    assert_eq!(records.functions, []);
    // An alias of the snapshot named df reaches the view's function.
    std::fs::write(dir.join("snapshot.zsh"), "alias df='df -h'\n").unwrap();
    let (out, _) = run_mount_line(&child, &dir, "df").await;
    assert_eq!(out, "snapshot df -h\n");
}

#[tokio::test]
async fn mount_view_functions_not_in_records() {
    let Some(zsh) = Zsh::start_sandboxed("mount_view_functions_not_in_records") else {
        return;
    };
    let (child, dir) = mount_view_child(&zsh, "records").await;
    let line = "df -h; findmnt -D; unfunction df; mine() { :; }";
    let (out, records) = run_mount_line(&child, &dir, line).await;
    assert_eq!(out, "snapshot df -h\nsnapshot findmnt-df -D\n");
    let functions: Vec<&str> = records.functions.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(functions, ["mine"], "the view's own functions are not reported");
    assert_eq!(records.removed_functions, Vec::<String>::new());
    // A function of that name that the line defines is the model's: the next call
    // gets it.
    let line = "df -h; findmnt() { print -r -- mine }; unfunction df; df() { print -r -- my df }";
    let (out, records) = run_mount_line(&child, &dir, line).await;
    assert_eq!(out, "snapshot df -h\n");
    let mut functions: Vec<(&str, &str)> =
        records.functions.iter().map(|(name, body)| (name.as_str(), body.trim())).collect();
    functions.sort_unstable();
    assert_eq!(functions, [("df", "print -r -- my df"), ("findmnt", "print -r -- mine")]);
    assert_eq!(records.removed_functions, Vec::<String>::new());
}

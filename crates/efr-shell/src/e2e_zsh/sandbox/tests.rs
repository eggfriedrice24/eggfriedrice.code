//! The zsh tests of efr's auto spec, section 16.2, over a real zsh and the fake
//! launcher.

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
    let first = zsh.prepare(1);
    let result = zsh
        .run_sandboxed(&first, "greet you; ll; [[ abc == a(#c1)bc ]] && print -r -- globbed")
        .await;
    assert_eq!(result.output, "hi you\nlisted\nglobbed\n");
    let snapshot = zsh.sandbox_dir().join("snapshot.zsh");
    let text = std::fs::read_to_string(&snapshot).unwrap();
    assert!(!text.contains("_efr_hs_"), "efr's own functions are in the snapshot");

    // No other line ran since: the snapshot is not written again.
    std::fs::write(&snapshot, format!("{text}\n# kept\n")).unwrap();
    let second = zsh.prepare(2);
    zsh.run_sandboxed(&second, "true").await;
    assert!(std::fs::read_to_string(&snapshot).unwrap().ends_with("# kept\n"));

    // Another line ran: it is written again, and compiled once it is large.
    zsh.run_plain("eval \"big() { : '$(printf %070000d 0)' }\"").await;
    let third = zsh.prepare(3);
    zsh.run_sandboxed(&third, "true").await;
    let text = std::fs::read_to_string(&snapshot).unwrap();
    assert!(!text.contains("# kept"));
    assert!(text.len() > 65_536);
    assert!(zsh.sandbox_dir().join("snapshot.zsh.zwc").exists());
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

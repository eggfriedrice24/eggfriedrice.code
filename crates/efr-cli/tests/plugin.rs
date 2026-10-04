//! The zsh plugin, `shell/zsh/efr.plugin.zsh`, against a fake `efr` that records its
//! arguments: what the `,` commands pass, the last command line above all.
//!
//! These tests need zsh and run only with `EFR_TEST_ZSH=1`. zsh starts with `-f`, a
//! cleared environment and a temporary `HOME`, so no startup file of the real user runs.
//! The plugin's hooks are called directly, in the order zsh calls them around a line.

// NOTE: an integration test crate is always built with cfg(test); saying so lets
// clippy treat its helper functions as test code, as it does for unit tests.
#![cfg(test)]

use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use assert_cmd::Command;
use pretty_assertions::assert_eq;

/// A fake `efr` that writes each argument on a line of its own to `$EFR_ARGS`, one
/// file per call, and exits with `$FAKE_EXIT` (0 when unset).
const FAKE_EFR: &str = r#"#!/bin/sh
n=0
while [ -e "$EFR_ARGS.$n" ]; do n=$((n + 1)); done
for arg in "$@"; do printf '%s\n' "$arg"; done > "$EFR_ARGS.$n"
exit "${FAKE_EXIT:-0}"
"#;

fn plugin() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../shell/zsh/efr.plugin.zsh")
}

#[expect(clippy::print_stderr, reason = "a skipped test says why")]
fn zsh_tests_enabled() -> bool {
    let enabled = efr_stdx::env::flag(efr_stdx::env::Var::TestZsh).unwrap_or(false);
    if !enabled {
        eprintln!("skipped: set EFR_TEST_ZSH=1 to run the tests that need zsh");
    }
    enabled
}

/// Runs `script` in an interactive zsh with the plugin sourced, and returns the
/// arguments of every `efr` call, one list per call.
fn run(script: &str) -> Vec<Vec<String>> {
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    let efr = bin.join("efr");
    std::fs::write(&efr, FAKE_EFR).unwrap();
    std::fs::set_permissions(&efr, std::fs::Permissions::from_mode(0o755)).unwrap();
    let args = dir.path().join("args");
    let full = format!("source {}\n{script}\n", plugin().display());
    let output = Command::new("zsh")
        .args(["-f", "-i", "-c", &full])
        .env_clear()
        .env("HOME", dir.path())
        .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
        .env("TERM", "dumb")
        .env("XDG_RUNTIME_DIR", dir.path())
        .env("EFR_ARGS", &args)
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(output.status.success(), "zsh failed: {}", String::from_utf8_lossy(&output.stderr));
    (0..)
        .map(|n| PathBuf::from(format!("{}.{n}", args.display())))
        .take_while(|path| path.exists())
        .map(|path| std::fs::read_to_string(path).unwrap().lines().map(str::to_owned).collect())
        .collect()
}

/// `text` as one single-quoted zsh word.
fn quoted(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// The value after `flag` in one call's arguments.
fn value_of<'a>(call: &'a [String], flag: &str) -> Option<&'a str> {
    let at = call.iter().position(|arg| arg == flag)?;
    call.get(at + 1).map(String::as_str)
}

fn last_status(call: &[String]) -> serde_json::Value {
    let context: serde_json::Value =
        serde_json::from_str(value_of(call, "--context-json").unwrap()).unwrap();
    assert!(context.get("last_command").is_none(), "the context never carries it: {context}");
    context["last_status"].clone()
}

#[test]
fn e2e_a_prompt_carries_the_last_command_as_its_own_argument() {
    if !zsh_tests_enabled() {
        return;
    }
    let calls = run(r#"
        _efr_preexec 'make -j8 test'
        (exit 2); _efr_precmd
        , why did it fail
    "#);
    let [call] = calls.as_slice() else { panic!("one call expected: {calls:?}") };
    assert_eq!(call[0], "send");
    assert_eq!(value_of(call, "--last-command"), Some("make -j8 test"));
    assert_eq!(last_status(call), 2);
    let separator = call.iter().position(|arg| arg == "--").unwrap();
    assert_eq!(call[separator + 1..], ["why", "did", "it", "fail"]);
}

#[test]
fn e2e_plugin_lines_are_not_the_last_command() {
    if !zsh_tests_enabled() {
        return;
    }
    let calls = run(r#"
        _efr_preexec 'cargo build'
        (exit 101); _efr_precmd
        _efr_preexec ', why'
        , why
        _efr_precmd
        _efr_preexec '  ,new start over'
        ,new start over
        (exit 0); _efr_precmd
        , and now
    "#);
    assert_eq!(calls.len(), 3, "{calls:?}");
    for call in &calls {
        assert_eq!(value_of(call, "--last-command"), Some("cargo build"), "{call:?}");
        // The status goes with the command, not with the efr call in between.
        assert_eq!(last_status(call), 101, "{call:?}");
    }
    assert_eq!(calls[1][0], "new");
}

#[test]
fn e2e_without_a_shell_command_there_is_no_last_command() {
    if !zsh_tests_enabled() {
        return;
    }
    let calls = run("(exit 3); , hello");
    let [call] = calls.as_slice() else { panic!("one call expected: {calls:?}") };
    assert_eq!(value_of(call, "--last-command"), None);
    assert_eq!(last_status(call), 3);
}

#[test]
fn e2e_a_steer_carries_no_last_command() {
    if !zsh_tests_enabled() {
        return;
    }
    let calls = run(r#"
        _efr_preexec 'ls'
        true; _efr_precmd
        ,! use the other file
    "#);
    let [call] = calls.as_slice() else { panic!("one call expected: {calls:?}") };
    assert_eq!(call[..2], ["send", "--steer"]);
    assert_eq!(value_of(call, "--last-command"), None);
}

#[test]
fn e2e_the_hooks_are_registered_once_even_when_sourced_twice() {
    if !zsh_tests_enabled() {
        return;
    }
    let calls = run(&format!(
        r#"
        _efr_preexec 'uptime'
        true; _efr_precmd
        source {}
        efr hooks ${{#${{(M)preexec_functions:#_efr_preexec}}}} ${{#${{(M)precmd_functions:#_efr_precmd}}}} "$_efr_last_command"
    "#,
        plugin().display()
    ));
    let [call] = calls.as_slice() else { panic!("one call expected: {calls:?}") };
    // One preexec and one precmd hook, and the remembered command survived.
    assert_eq!(call[..], ["hooks", "1", "1", "uptime"]);
}

#[test]
fn e2e_a_prompt_with_shell_syntax_reaches_efr_as_typed() {
    if !zsh_tests_enabled() {
        return;
    }
    let prompts = [
        "what is using port 8080?",
        "list the *.log files",
        "files > 1MB",
        "explain this; rm -rf build",
        "what's this",
        "run !make again",
        "count lines | sort",
        "a  b",
    ];
    // The accept-line widget runs `_efr_rewrite_line` on the typed line; zsh then parses
    // what it returns, as eval does here.
    let script: String = prompts
        .iter()
        .map(|prompt| {
            format!("_efr_rewrite_line {}; eval \"$REPLY\"\n", quoted(&format!(", {prompt}")))
        })
        .collect();
    let calls = run(&script);
    assert_eq!(calls.len(), prompts.len(), "{calls:?}");
    for (call, prompt) in calls.iter().zip(prompts) {
        assert_eq!(call[0], "send");
        let separator = call.iter().position(|arg| arg == "--").unwrap();
        assert_eq!(call[separator + 1..], [prompt], "{call:?}");
    }
}

#[test]
fn e2e_the_rewrite_quotes_only_plugin_lines() {
    if !zsh_tests_enabled() {
        return;
    }
    let calls = run(r#"
        for line in ',new start over?' '  ,! use the *other* file' ',new' 'ls -l *.rs' ',newline'; do
          _efr_rewrite_line "$line"
          efr rewritten "$REPLY"
        done
    "#);
    let rewritten: Vec<&str> = calls.iter().map(|call| call[1].as_str()).collect();
    assert_eq!(
        rewritten,
        [
            r",new start\ over\?",
            r"  ,! use\ the\ \*other\*\ file",
            ",new",
            "ls -l *.rs",
            ",newline",
        ]
    );
}

/// The prompt words of one call: what follows `--`.
fn prompt_of(call: &[String]) -> &[String] {
    let separator = call.iter().position(|arg| arg == "--").unwrap();
    &call[separator + 1..]
}

#[test]
fn e2e_a_bare_new_makes_the_next_line_start_a_conversation() {
    if !zsh_tests_enabled() {
        return;
    }
    let calls = run(r#"
        ,new
        , first prompt
        , second prompt
    "#);
    assert_eq!(calls.len(), 2, "a bare ,new sends nothing: {calls:?}");
    assert_eq!(calls[0][0], "new");
    assert_eq!(prompt_of(&calls[0]), ["first", "prompt"]);
    assert_eq!(calls[1][0], "send");
    assert_eq!(prompt_of(&calls[1]), ["second", "prompt"]);
}

#[test]
fn e2e_a_bare_new_waits_until_a_conversation_could_start() {
    if !zsh_tests_enabled() {
        return;
    }
    // Exit 3: no daemon listened, so no conversation started and the wish stays.
    let calls = run(r#"
        ,new
        FAKE_EXIT=3 , first try
        , second try
        , third try
    "#);
    let commands: Vec<&str> = calls.iter().map(|call| call[0].as_str()).collect();
    assert_eq!(commands, ["new", "new", "send"]);
}

//! The zsh plugin, `shell/zsh/efr.plugin.zsh`, against a fake `efr` that records its
//! command line as `/proc` shows it to every local user, and the variables the plugin
//! hands over: what the `,` commands pass, the last command line above all.
//!
//! These tests need zsh and run only with `EFR_TEST_ZSH=1`. zsh starts with `-f`, a
//! cleared environment and a temporary `HOME`, so no startup file of the real user runs.
//! The plugin's hooks are called directly, in the order zsh calls them around a line.
//! One test puts the built `efr` in the fake's place, in front of a `TestDaemon`.

// NOTE: an integration test crate is always built with cfg(test); saying so lets
// clippy treat its helper functions as test code, as it does for unit tests.
#![cfg(test)]

use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use assert_cmd::Command;
use efr_protocol::{Event, PromptSendResult};
use efr_test_daemon::{ResponsesAnswer, ResponsesServer, TTY, TestDaemon, events_until};
use pretty_assertions::assert_eq;

/// A fake `efr` that records each call under `$EFR_ARGS.<n>`: its command line from
/// `/proc`, NUL-separated, then the three variables the plugin hands over, each
/// prefixed `set:` or `unset:`. The record file is written last, because its existence
/// numbers the calls. It exits with `$FAKE_EXIT` (0 when unset).
const FAKE_EFR: &str = r#"#!/bin/sh
n=0
while [ -e "$EFR_ARGS.$n" ]; do n=$((n + 1)); done
record="$EFR_ARGS.$n"
cat /proc/$$/cmdline > "$record.cmdline"
if [ -n "${EFR_CONTEXT+x}" ]; then printf 'set:%s' "$EFR_CONTEXT"; else printf unset:; fi > "$record.EFR_CONTEXT"
if [ -n "${EFR_LAST_COMMAND+x}" ]; then printf 'set:%s' "$EFR_LAST_COMMAND"; else printf unset:; fi > "$record.EFR_LAST_COMMAND"
if [ -n "${EFR_PROMPT+x}" ]; then printf 'set:%s' "$EFR_PROMPT"; else printf unset:; fi > "$record.EFR_PROMPT"
: > "$record"
exit "${FAKE_EXIT:-0}"
"#;

/// One call of the fake `efr`.
#[derive(Debug, Clone)]
struct Call {
    /// The arguments after the program, as `/proc/<pid>/cmdline` lists them.
    args: Vec<String>,
    /// The whole command line as other users see it.
    cmdline: String,
    context: Option<String>,
    last_command: Option<String>,
    prompt: Option<String>,
}

impl Call {
    fn read(record: &Path) -> Call {
        let cmdline = std::fs::read_to_string(format!("{}.cmdline", record.display())).unwrap();
        // `/bin/sh <path to efr> <args>...`, each ended by a NUL.
        let words: Vec<String> = cmdline.split_terminator('\0').map(str::to_owned).collect();
        assert!(words.len() >= 2 && words[1].ends_with("/efr"), "{words:?}");
        let var = |name: &str| {
            let text = std::fs::read_to_string(format!("{}.{name}", record.display())).unwrap();
            match text.strip_prefix("set:") {
                Some(value) => Some(value.to_owned()),
                None => {
                    assert_eq!(text, "unset:");
                    None
                }
            }
        };
        Call {
            args: words[2..].to_vec(),
            cmdline: cmdline.replace('\0', " "),
            context: var("EFR_CONTEXT"),
            last_command: var("EFR_LAST_COMMAND"),
            prompt: var("EFR_PROMPT"),
        }
    }

    /// The shell context the call carried, which never holds the last command.
    fn context(&self) -> serde_json::Value {
        let context: serde_json::Value =
            serde_json::from_str(self.context.as_deref().unwrap()).unwrap();
        assert!(context.get("last_command").is_none(), "the context never carries it: {context}");
        context
    }

    fn last_status(&self) -> serde_json::Value {
        self.context()["last_status"].clone()
    }

    /// The last command, where an empty variable means none, as `efr` reads it.
    fn last_command(&self) -> Option<&str> {
        self.last_command.as_deref().filter(|line| !line.is_empty())
    }
}

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

/// A temporary home with the fake `efr` in its `bin`.
struct Home {
    dir: tempfile::TempDir,
}

impl Home {
    fn new() -> Home {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let efr = bin.join("efr");
        std::fs::write(&efr, FAKE_EFR).unwrap();
        std::fs::set_permissions(&efr, std::fs::Permissions::from_mode(0o755)).unwrap();
        Home { dir }
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn records(&self) -> PathBuf {
        self.path().join("args")
    }

    /// `zsh -f` with a cleared environment that knows only this home and the fake
    /// `efr`.
    fn zsh(&self) -> Command {
        let mut command = Command::new("zsh");
        command
            .env_clear()
            .env("HOME", self.path())
            .env("PATH", format!("{}:/usr/bin:/bin", self.path().join("bin").display()))
            .env("TERM", "dumb")
            .env("XDG_RUNTIME_DIR", self.path())
            .env("EFR_ARGS", self.records())
            .current_dir(self.path());
        command
    }

    /// Every `efr` call so far, oldest first.
    fn calls(&self) -> Vec<Call> {
        (0..)
            .map(|n| PathBuf::from(format!("{}.{n}", self.records().display())))
            .take_while(|path| path.exists())
            .map(|path| Call::read(&path))
            .collect()
    }
}

/// Runs `script` in `zsh -f -i -c` with the plugin sourced, and returns its stdout.
fn run_in(home: &Home, script: &str) -> String {
    let full = format!("source {}\n{script}\n", plugin().display());
    let output = home.zsh().args(["-f", "-i", "-c", &full]).output().unwrap();
    assert!(output.status.success(), "zsh failed: {}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8(output.stdout).unwrap()
}

/// Runs `script` in an interactive zsh with the plugin sourced, and returns every
/// `efr` call.
fn run(script: &str) -> Vec<Call> {
    let home = Home::new();
    run_in(&home, script);
    home.calls()
}

/// Drives an interactive `zsh -f -i` on a pseudo-terminal from zsh's own zpty
/// module, so ZLE reads every key as it does for a person and the plugin's widgets
/// run for real. `$EFR_KEYS` is typed in one go, with `<C-Space>` standing for the
/// NUL byte that Ctrl+Space sends; the driver prints what the terminal showed.
const DRIVER: &str = r#"
zmodload zsh/zpty || exit 90
zpty user 'TERM=xterm zsh -f -i'
# ZLE turns bracketed paste on once it reads keys in raw mode; keys typed earlier
# would meet a terminal that still edits lines itself.
zpty -r user screen $'*\e\\[\\?2004h*' || exit 91
print -rn -- "$screen"
nul=$'\0'
zpty -w -n user "${EFR_KEYS//'<C-Space>'/$nul}"
while zpty -r user chunk; do print -rn -- "$chunk"; done
"#;

/// Types `lines` into an interactive zsh after a line that sources the plugin, each
/// line ended by Enter and the last followed by `exit`, and returns what the terminal
/// showed.
fn type_lines(home: &Home, lines: &[&str]) -> String {
    let mut keys = format!("source {}\r", plugin().display());
    for line in lines {
        keys.push_str(line);
        keys.push('\r');
    }
    // `!exit` is never read unless sticky mode wrongly stayed on and sent `exit` to the
    // agent; then it ends the shell instead of leaving the test waiting.
    keys.push_str("exit\r!exit\r");
    let output = home.zsh().env("EFR_KEYS", keys).args(["-f", "-c", DRIVER]).output().unwrap();
    let screen = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "the driver failed: {stderr}\n{screen}");
    screen
}

/// `text` as one single-quoted zsh word.
fn quoted(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

#[test]
fn e2e_a_prompt_carries_the_last_command_on_its_own() {
    if !zsh_tests_enabled() {
        return;
    }
    let calls = run(r#"
        _efr_preexec 'make -j8 test'
        (exit 2); _efr_precmd
        , why did it fail
    "#);
    let [call] = calls.as_slice() else { panic!("one call expected: {calls:?}") };
    assert_eq!(call.args, ["send"]);
    assert_eq!(call.last_command(), Some("make -j8 test"));
    assert_eq!(call.last_status(), 2);
    assert_eq!(call.prompt.as_deref(), Some("why did it fail"));
}

#[test]
fn e2e_no_typed_text_reaches_the_command_line_of_efr() {
    if !zsh_tests_enabled() {
        return;
    }
    let home = Home::new();
    run_in(
        &home,
        r#"
        _efr_preexec 'export TOKEN=s3cret-token'
        true; _efr_precmd
        , why is s3cret-prompt slow
        ,new start s3cret-new
        ,! steer s3cret-steer
        efr leftover ${+EFR_CONTEXT} ${+EFR_LAST_COMMAND} ${+EFR_PROMPT}
    "#,
    );
    let calls = home.calls();
    let commands: Vec<&[String]> = calls.iter().map(|call| call.args.as_slice()).collect();
    assert_eq!(commands[..3], [&["send"][..], &["new"][..], &["send", "--steer"][..]]);
    for call in &calls {
        assert!(!call.cmdline.contains("s3cret"), "readable by every user: {call:?}");
    }
    assert_eq!(calls[0].prompt.as_deref(), Some("why is s3cret-prompt slow"));
    assert_eq!(calls[0].last_command(), Some("export TOKEN=s3cret-token"));
    assert_eq!(calls[1].prompt.as_deref(), Some("start s3cret-new"));
    assert_eq!(calls[2].prompt.as_deref(), Some("steer s3cret-steer"));
    assert!(calls[2].context.as_deref().unwrap().contains("\"pwd\""));
    // The variables lived only in the environment of each efr: the shell has none of
    // them, and the next command inherits none.
    let leftover = &calls[3];
    assert_eq!(leftover.args, ["leftover", "0", "0", "0"]);
    assert_eq!(
        (&leftover.context, &leftover.last_command, &leftover.prompt),
        (&None, &None, &None)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_the_built_efr_answers_a_prompt_that_the_plugin_hands_over() {
    if !zsh_tests_enabled() {
        return;
    }
    let server = ResponsesServer::start().await;
    server.push(ResponsesAnswer::text("Hello from **efr**."));
    let daemon = TestDaemon::builder().responses(&server).start().await.unwrap();
    let home = Home::new();
    let efr = home.path().join("bin/efr");
    std::fs::remove_file(&efr).unwrap();
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_efr"), &efr).unwrap();
    let dirs = daemon.dirs().dirs();
    let script = format!("source {}\n, say hello\n", plugin().display());
    let mut zsh = home.zsh();
    zsh.env("EFR_CONFIG_DIR", dirs.config())
        .env("EFR_DATA_DIR", dirs.data())
        .env("EFR_STATE_DIR", dirs.state())
        .env("EFR_RUNTIME_DIR", dirs.runtime())
        .current_dir(daemon.cwd())
        .args(["-f", "-i", "-c", &script]);

    // Off the runtime's workers, which must keep serving the daemon.
    let output = tokio::task::spawn_blocking(move || zsh.output().unwrap()).await.unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stderr}");
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim_end(), "Hello from **efr**.");
    let [request] = server.received().try_into().unwrap();
    let input = request.body["input"].to_string();
    assert!(input.contains("say hello"), "{input}");
    daemon.stop().await.unwrap();
}

#[test]
fn e2e_an_exported_variable_of_the_shell_never_joins_a_prompt() {
    if !zsh_tests_enabled() {
        return;
    }
    let calls = run(r#"
        export EFR_LAST_COMMAND='stale line' EFR_PROMPT='stale prompt'
        , fresh prompt
    "#);
    let [call] = calls.as_slice() else { panic!("one call expected: {calls:?}") };
    assert_eq!(call.prompt.as_deref(), Some("fresh prompt"));
    assert_eq!(call.last_command(), None, "{call:?}");
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
        assert_eq!(call.last_command(), Some("cargo build"), "{call:?}");
        // The status goes with the command, not with the efr call in between.
        assert_eq!(call.last_status(), 101, "{call:?}");
    }
    assert_eq!(calls[1].args, ["new"]);
}

#[test]
fn e2e_without_a_shell_command_there_is_no_last_command() {
    if !zsh_tests_enabled() {
        return;
    }
    let calls = run("(exit 3); , hello");
    let [call] = calls.as_slice() else { panic!("one call expected: {calls:?}") };
    assert_eq!(call.last_command(), None);
    assert_eq!(call.last_status(), 3);
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
    assert_eq!(call.args, ["send", "--steer"]);
    assert_eq!(call.last_command(), None);
    assert_eq!(call.prompt.as_deref(), Some("use the other file"));
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
    assert_eq!(call.args, ["hooks", "1", "1", "uptime"]);
}

/// Prompts that zsh would read as shell syntax if the plugin did not quote them.
const SHELL_SYNTAX: [&str; 10] = [
    "what is using port 8080?",
    "list the *.log files",
    "files > 1MB",
    "explain this; rm -rf build",
    "what's this",
    "run !make again",
    "why does it fail!?",
    "what does !! do",
    "count lines | sort",
    "a  b",
];

#[test]
fn e2e_a_prompt_typed_at_a_terminal_reaches_efr_as_typed() {
    if !zsh_tests_enabled() {
        return;
    }
    // At a terminal zsh also expands history, so a `!` must survive that too.
    let home = Home::new();
    let lines: Vec<String> = SHELL_SYNTAX.iter().map(|prompt| format!(", {prompt}")).collect();
    let lines: Vec<&str> = lines.iter().map(String::as_str).collect();
    type_lines(&home, &lines);
    let prompts: Vec<Option<String>> = home.calls().into_iter().map(|call| call.prompt).collect();
    let expected: Vec<Option<String>> =
        SHELL_SYNTAX.iter().map(|prompt| Some((*prompt).to_owned())).collect();
    assert_eq!(prompts, expected);
}

/// What zsh-autosuggestions does on every prompt: it rebinds each widget of type
/// "builtin" to a wrapper that calls `zle .<name>`. Only real builtins have a dot name,
/// so a widget that merely aliases a builtin breaks under it.
const REBIND_BUILTINS_LIKE_AUTOSUGGESTIONS: &str = r#"for w in ${(k)widgets}; do [[ $widgets[$w] == builtin && $w != .* ]] || continue; eval "_sim_orig_$w() { zle .$w }"; zle -N "$w" "_sim_orig_$w"; done"#;

#[test]
fn e2e_enter_works_after_a_plugin_rebinds_the_builtin_widgets() {
    if !zsh_tests_enabled() {
        return;
    }
    let home = Home::new();
    let screen = type_lines(
        &home,
        &[
            REBIND_BUILTINS_LIKE_AUTOSUGGESTIONS,
            ", what is in this directory?",
            "echo still-typing",
        ],
    );
    assert!(!screen.contains("No such widget"), "{screen}");
    assert!(screen.contains("still-typing"), "{screen}");
    let prompts: Vec<Option<String>> = home.calls().into_iter().map(|call| call.prompt).collect();
    assert_eq!(prompts, [Some("what is in this directory?".to_owned())]);
}

#[test]
fn e2e_a_prompt_with_shell_syntax_reaches_efr_as_typed() {
    if !zsh_tests_enabled() {
        return;
    }
    let prompts = SHELL_SYNTAX;
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
        assert_eq!(call.args, ["send"]);
        assert_eq!(call.prompt.as_deref(), Some(prompt), "{call:?}");
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
    let rewritten: Vec<&str> = calls.iter().map(|call| call.args[1].as_str()).collect();
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
    assert_eq!(calls[0].args, ["new"]);
    assert_eq!(calls[0].prompt.as_deref(), Some("first prompt"));
    assert_eq!(calls[1].args, ["send"]);
    assert_eq!(calls[1].prompt.as_deref(), Some("second prompt"));
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
    let commands: Vec<&str> = calls.iter().map(|call| call.args[0].as_str()).collect();
    assert_eq!(commands, ["new", "new", "send"]);
}

#[test]
fn e2e_a_line_of_just_a_comma_toggles_sticky_mode_as_ctrl_space_does() {
    if !zsh_tests_enabled() {
        return;
    }
    let home = Home::new();
    type_lines(
        &home,
        &[
            ",",
            "what is !! for?",
            // zsh -f has no extended_glob, which the plugin must not need from the user.
            ",new fresh start",
            "!efr probe $_efr_sticky",
            " , ",
            "efr probe $_efr_sticky",
            ", one prompt",
            "efr probe $_efr_sticky",
            "<C-Space>a line after ctrl space",
            ",",
            r#"efr history "${(@f)$(fc -ln 1)}""#,
        ],
    );
    let calls = home.calls();
    let args: Vec<Vec<&str>> =
        calls.iter().map(|call| call.args.iter().map(String::as_str).collect()).collect();
    assert_eq!(
        args[..7],
        [
            vec!["send"],
            vec!["new"],
            vec!["probe", "1"],
            vec!["probe", "0"],
            vec!["send"],
            vec!["probe", "0"],
            vec!["send"],
        ]
    );
    // In sticky mode the line is the prompt as typed, without history expansion.
    assert_eq!(calls[0].prompt.as_deref(), Some("what is !! for?"));
    assert_eq!(calls[1].prompt.as_deref(), Some("fresh start"));
    assert_eq!(calls[4].prompt.as_deref(), Some("one prompt"));
    assert_eq!(calls[6].prompt.as_deref(), Some("a line after ctrl space"));
    // A toggle runs nothing, so it never lands in history.
    let history = &calls[7].args[1..];
    assert!(!history.is_empty() && history.iter().all(|line| line.trim() != ","), "{history:?}");
}

#[test]
fn e2e_without_efr_a_lone_comma_says_what_is_missing() {
    if !zsh_tests_enabled() {
        return;
    }
    let home = Home::new();
    std::fs::remove_file(home.path().join("bin/efr")).unwrap();
    let screen = type_lines(&home, &[",", "print -r -- sticky=$_efr_sticky"]);
    assert!(screen.contains("efr: the efr binary is not on PATH"), "{screen}");
    assert!(screen.contains("sticky=0"), "{screen}");
}

/// Writes `text` as the notice file of `/dev/pts/77` under the runtime root `root`.
fn notice(root: &Path, text: &str) -> PathBuf {
    let dir = root.join("notices");
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("pts-77");
    std::fs::write(&file, format!("{text}\n")).unwrap();
    file
}

/// Shows the notices of `/dev/pts/77` as the prompt would, with `EFR_RUNTIME_DIR` set
/// to `runtime` when given, and returns what was printed.
fn print_notices(home: &Home, runtime: Option<&Path>) -> String {
    let script = format!("source {}\nTTY=/dev/pts/77\n_efr_print_notices\n", plugin().display());
    let mut zsh = home.zsh();
    if let Some(runtime) = runtime {
        zsh.env("EFR_RUNTIME_DIR", runtime);
    }
    let output = zsh.args(["-f", "-i", "-c", &script]).output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn e2e_notices_come_from_the_runtime_root_that_efr_runtime_dir_names() {
    if !zsh_tests_enabled() {
        return;
    }
    let home = Home::new();
    let runtime = home.path().join("just-run-runtime");
    let named = notice(&runtime, "efr: turn finished: from EFR_RUNTIME_DIR");
    // Home::zsh sets XDG_RUNTIME_DIR to the home, whose efr root holds another notice.
    let default = notice(&home.path().join("efr"), "efr: turn finished: from XDG_RUNTIME_DIR");

    let shown = print_notices(&home, Some(&runtime));

    assert_eq!(shown, "efr: turn finished: from EFR_RUNTIME_DIR\n");
    assert!(!named.exists(), "a shown notice is removed");
    assert!(default.exists(), "another daemon's notice stays");
}

#[test]
fn e2e_without_efr_runtime_dir_notices_come_from_xdg_runtime_dir() {
    if !zsh_tests_enabled() {
        return;
    }
    let home = Home::new();
    let default = notice(&home.path().join("efr"), "efr: approval waiting: a conversation");

    assert_eq!(print_notices(&home, None), "efr: approval waiting: a conversation\n");
    assert!(!default.exists());
    assert_eq!(print_notices(&home, None), "", "a notice shows once");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_the_prompt_shows_the_notice_that_the_daemon_wrote() {
    if !zsh_tests_enabled() {
        return;
    }
    let server = ResponsesServer::start().await;
    server.push(ResponsesAnswer::text("Hello."));
    let daemon = TestDaemon::builder().responses(&server).start().await.unwrap();
    // A client without a terminal follows the turn, so the terminal of the
    // conversation does not, and the daemon leaves it a notice.
    let client = daemon.client().await.unwrap();
    let sent: PromptSendResult = client.call(daemon.prompt(1, "say hello", TTY)).await.unwrap();
    let mut follow = daemon.follow(&client, sent.conversation_id).await.unwrap();
    events_until(&mut follow, |event| matches!(event, Event::TurnCompleted { .. })).await.unwrap();
    let runtime = daemon.dirs().dirs().runtime().to_path_buf();
    let file = runtime.join("notices").join(TTY.trim_start_matches("/dev/").replace('/', "-"));
    // The daemon writes the notice from its own task after the commit.
    let mut written = false;
    for _ in 0..100_000 {
        if std::fs::metadata(&file).is_ok_and(|meta| meta.len() > 0) {
            written = true;
            break;
        }
        tokio::task::yield_now().await;
    }
    assert!(written, "no notice at {}", file.display());

    let home = Home::new();
    let script = format!("source {}\nTTY={TTY}\n_efr_print_notices\n", plugin().display());
    let mut zsh = home.zsh();
    zsh.env("EFR_RUNTIME_DIR", &runtime).args(["-f", "-i", "-c", &script]);
    let output = tokio::task::spawn_blocking(move || zsh.output().unwrap()).await.unwrap();

    let shown = String::from_utf8(output.stdout).unwrap();
    assert!(shown.starts_with("efr: turn finished: "), "{shown:?}");
    assert!(!file.exists(), "the plugin removed the notice it showed");
    drop((follow, client));
    daemon.stop().await.unwrap();
}

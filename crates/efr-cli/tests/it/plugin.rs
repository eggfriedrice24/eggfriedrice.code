//! The zsh plugin, `shell/zsh/efr.plugin.zsh`, against a fake `efr` that records its
//! command line as `/proc` shows it to every local user, and the variables the plugin
//! hands over: what the `,` commands pass, the last command line above all.
//!
//! These tests need zsh and run only with `EFR_TEST_ZSH=1`. zsh starts with `-f`, a
//! cleared environment and a temporary `HOME`, so no startup file of the real user runs.
//! The plugin's hooks are called directly, in the order zsh calls them around a line.
//! One test puts the built `efr` in the fake's place, in front of a `TestDaemon`, and
//! one compares the plugin's runtime root with the one the built `efr paths` finds.

use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use assert_cmd::Command;
use efr_protocol::{Event, PromptSendResult};
use efr_test_daemon::{ResponsesAnswer, ResponsesServer, TTY, TestDaemon, events_until};
use efr_test_support::Wait;
use pretty_assertions::assert_eq;

/// A fake `efr` that records each call under `$EFR_ARGS.<n>`: its command line from
/// `/proc`, NUL-separated, then the variables the plugin hands over, each prefixed
/// `set:` or `unset:`. It prints `$EFR_ARGS.out.<first argument>` when that file exists,
/// such as `args.out.settings` for `efr settings`. The record file is written last,
/// because its existence numbers the calls. It exits with 2 when one of its arguments
/// is a line of `$EFR_ARGS.refuse`, as efr refuses a value, and otherwise with
/// `$FAKE_EXIT` (0 when unset).
const FAKE_EFR: &str = r#"#!/bin/sh
n=0
while [ -e "$EFR_ARGS.$n" ]; do n=$((n + 1)); done
record="$EFR_ARGS.$n"
cat /proc/$$/cmdline > "$record.cmdline"
for var in EFR_CONTEXT EFR_LAST_COMMAND EFR_PROMPT EFR_MODE EFR_MODEL EFR_EFFORT; do
  eval "isset=\${$var+x} value=\${$var-}"
  if [ -n "$isset" ]; then printf 'set:%s' "$value"; else printf unset:; fi > "$record.$var"
done
if [ -f "$EFR_ARGS.out.$1" ]; then cat "$EFR_ARGS.out.$1"; fi
: > "$record"
if [ -f "$EFR_ARGS.refuse" ]; then
  for arg in "$@"; do
    if grep -qxF -e "$arg" "$EFR_ARGS.refuse"; then exit 2; fi
  done
fi
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
    /// The terminal's turn settings, as the plugin handed them over.
    mode: Option<String>,
    model: Option<String>,
    effort: Option<String>,
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
            mode: var("EFR_MODE"),
            model: var("EFR_MODEL"),
            effort: var("EFR_EFFORT"),
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

    /// The mode, model and effort as `efr` reads them: an empty variable is none.
    fn settings(&self) -> [Option<&str>; 3] {
        [&self.mode, &self.model, &self.effort]
            .map(|value| value.as_deref().filter(|value| !value.is_empty()))
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
zpty user "TERM=${EFR_TEST_TERM:-xterm} zsh -f -i"
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
    type_lines_with(home, &[], lines)
}

/// The environment of a terminal in a UTF-8 locale, where the sticky indicator is the
/// robot. The other tests run in the C locale.
const UTF8: &[(&str, &str)] = &[("LANG", "C.UTF-8")];

/// As [`type_lines`], with the variables `env` added to the shell's environment.
fn type_lines_with(home: &Home, env: &[(&str, &str)], lines: &[&str]) -> String {
    let mut keys = format!("source {}\r", plugin().display());
    for line in lines {
        keys.push_str(line);
        keys.push('\r');
    }
    // `!exit` is never read unless sticky mode wrongly stayed on and sent `exit` to the
    // agent; then it ends the shell instead of leaving the test waiting.
    keys.push_str("exit\r!exit\r");
    let output = home
        .zsh()
        .envs(env.iter().copied())
        .env("EFR_KEYS", keys)
        .args(["-f", "-c", DRIVER])
        .output()
        .unwrap();
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
fn e2e_a_typed_prompt_stays_as_typed_on_screen_and_in_history() {
    if !zsh_tests_enabled() {
        return;
    }
    let home = Home::new();
    let prompt = "what's in /etc? list it; then | sort $HOME";
    let screen = type_lines(&home, &[&format!(", {prompt}"), "fc -ln -1 > history.txt"]);
    let prompts: Vec<Option<String>> = home.calls().into_iter().map(|call| call.prompt).collect();
    assert_eq!(prompts, [Some(prompt.to_owned())]);
    // No quoting ever reaches the screen or the history.
    assert!(!screen.contains(r"what\'s") && !screen.contains(r"\ "), "{screen}");
    let history = std::fs::read_to_string(home.path().join("history.txt")).unwrap();
    assert_eq!(history.trim_end(), format!(", {prompt}"));
}

#[test]
fn e2e_a_prompt_line_leaves_the_users_own_options_as_they_were() {
    if !zsh_tests_enabled() {
        return;
    }
    let home = Home::new();
    let save = r#"print -r -- "$options[interactivecomments] $options[banghist]" >> options.txt"#;
    type_lines(
        &home,
        &[
            "setopt interactive_comments",
            ", one",
            save,
            "unsetopt interactive_comments",
            ", two",
            save,
        ],
    );
    let saved = std::fs::read_to_string(home.path().join("options.txt")).unwrap();
    assert_eq!(saved.lines().collect::<Vec<_>>(), ["on on", "off on"]);
    assert_eq!(home.calls().len(), 2);
}

#[test]
fn e2e_a_prompt_over_several_lines_never_runs_its_later_lines() {
    if !zsh_tests_enabled() {
        return;
    }
    let home = Home::new();
    // ESC Return inserts a newline into the line being edited. interactive_comments is on,
    // the setting under which a comment would end at the newline and run the next line.
    type_lines(&home, &["setopt interactive_comments", ", first line\u{1b}\rtouch ran-marker"]);
    assert!(!home.path().join("ran-marker").exists(), "the second line ran as a command");
    let prompts: Vec<Option<String>> = home.calls().into_iter().map(|call| call.prompt).collect();
    assert_eq!(prompts, [Some("first line\ntouch ran-marker".to_owned())]);
}

#[test]
fn e2e_sticky_mode_shows_its_indicator_without_changing_the_prompt() {
    if !zsh_tests_enabled() {
        return;
    }
    let home = Home::new();
    let screen = type_lines(
        &home,
        &[
            r#"print -r -- "$PROMPT" > before.txt"#,
            "<C-Space>",
            r#"!print -r -- "$PROMPT" > during.txt"#,
            "<C-Space>",
        ],
    );
    let before = std::fs::read_to_string(home.path().join("before.txt")).unwrap();
    let during = std::fs::read_to_string(home.path().join("during.txt")).unwrap();
    assert_eq!(before, during, "sticky mode must not change PROMPT");
    // The indicator is drawn while sticky mode is on. The raw bytes cannot show it next
    // to the typed text: ZLE writes a key, steps back and writes it again with colour.
    assert!(screen.contains("efr> "), "{screen}");
    assert!(home.calls().is_empty(), "{:?}", home.calls());
}

#[test]
fn e2e_in_sticky_mode_a_prompt_line_starts_with_the_robot_instead_of_a_comma() {
    if !zsh_tests_enabled() {
        return;
    }
    let home = Home::new();
    let prompt = "what's up? *glob* ; | > x";
    let screen = type_lines_with(
        &home,
        UTF8,
        &["<C-Space>", prompt, "!fc -ln -1 > history.txt", "<C-Space>"],
    );
    let prompts: Vec<Option<String>> = home.calls().into_iter().map(|call| call.prompt).collect();
    assert_eq!(prompts, [Some(prompt.to_owned())]);
    assert!(!home.path().join("x").exists(), "the prompt ran as shell syntax");
    // History keeps the line that ran, which the screen showed all along: the robot
    // stood before the typed text and stays there, and no comma is ever drawn.
    let history = std::fs::read_to_string(home.path().join("history.txt")).unwrap();
    assert_eq!(history.trim_end(), format!("🤖 {prompt}"));
    assert!(screen.contains('🤖'), "{screen}");
    assert!(!screen.contains(", what"), "{screen}");
}

#[test]
fn e2e_a_robot_line_from_history_goes_to_the_agent_outside_sticky_mode() {
    if !zsh_tests_enabled() {
        return;
    }
    let home = Home::new();
    type_lines_with(&home, UTF8, &["🤖 sent again; from history"]);
    let prompts: Vec<Option<String>> = home.calls().into_iter().map(|call| call.prompt).collect();
    assert_eq!(prompts, [Some("sent again; from history".to_owned())]);
}

#[test]
fn e2e_the_sticky_word_is_one_plain_word_that_names_nothing_else() {
    if !zsh_tests_enabled() {
        return;
    }
    let calls = run(r#"
        alias taken=true
        for indicator in '🤖 ' 'ai  ' 'efr> ' 'ls ' 'if ' 'taken ' '-x ' 'a b ' ', '; do
          EFR_STICKY_INDICATOR=$indicator
          if _efr_sticky_word; then efr word "$REPLY"; else efr none; fi
        done
    "#);
    let words: Vec<String> = calls.iter().map(|call| call.args.join(" ")).collect();
    assert_eq!(
        words,
        ["word 🤖", "word ai", "none", "none", "none", "none", "none", "none", "none"]
    );
}

#[test]
fn e2e_sticky_mode_leaves_continuation_lines_and_vared_alone() {
    if !zsh_tests_enabled() {
        return;
    }
    let home = Home::new();
    type_lines_with(
        &home,
        UTF8,
        &[
            "<C-Space>",
            "!for x in a b",
            "do print -r -- $x >> loop.txt; done",
            "!vared -c answer",
            "typed into vared",
            r#"!print -r -- "$answer" > answer.txt"#,
            "<C-Space>",
        ],
    );
    assert!(home.calls().is_empty(), "{:?}", home.calls());
    let read = |name: &str| std::fs::read_to_string(home.path().join(name)).unwrap();
    assert_eq!(read("loop.txt"), "a\nb\n");
    assert_eq!(read("answer.txt"), "typed into vared\n");
}

#[test]
fn e2e_a_prompt_with_shell_syntax_reaches_efr_as_typed() {
    if !zsh_tests_enabled() {
        return;
    }
    let prompts = SHELL_SYNTAX;
    // For a prompt over several lines the accept-line widget runs `_efr_rewrite_line` on
    // the typed line; zsh then parses what it returns, as eval does here. Aliases are off
    // because eval always reads `#` as a comment, while the widget turns
    // interactive_comments off for such a line so the alias's `#` stays a word.
    let mut script = String::from("unsetopt aliases\n");
    script.extend(prompts.iter().map(|prompt| {
        format!("_efr_rewrite_line {}; eval \"$REPLY\"\n", quoted(&format!(", {prompt}")))
    }));
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
    Wait::new(&format!("a notice at {}", file.display()))
        .until(|| std::fs::read(&file).is_ok_and(|text| text.ends_with(b"\n")))
        .await
        .unwrap();

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

impl Home {
    /// Makes the fake `efr` print `text` when its first argument is `command`.
    fn output(&self, command: &str, text: &str) {
        std::fs::write(format!("{}.out.{command}", self.records().display()), text).unwrap();
    }
}

/// What the fake `efr settings` prints: the lines of a terminal without settings of
/// its own, as the real one prints them.
const SETTINGS: &str = "\
mode = cautious  # default; choices: manual, cautious, auto
model = gpt-5.5  # the daemon's default; choices: gpt-5.5, gpt-5.4, my-model
effort = medium  # the default of gpt-5.5; choices: low, medium, high
";

/// As [`run_in`], with the variables `env` in the shell's environment when the plugin
/// is sourced; returns stdout and the exit status of the script.
fn run_with(home: &Home, env: &[(&str, &str)], script: &str) -> (String, Option<i32>) {
    let full = format!("source {}\n{script}\n", plugin().display());
    let output =
        home.zsh().envs(env.iter().copied()).args(["-f", "-i", "-c", &full]).output().unwrap();
    (String::from_utf8(output.stdout).unwrap(), output.status.code())
}

/// The arguments of each call, as plain strings.
fn args(calls: &[Call]) -> Vec<Vec<&str>> {
    calls.iter().map(|call| call.args.iter().map(String::as_str).collect()).collect()
}

#[test]
fn e2e_the_terminals_settings_reach_its_prompts_and_new_conversations() {
    if !zsh_tests_enabled() {
        return;
    }
    let home = Home::new();
    // EFR_MODE and EFR_MODEL give the first values when the plugin is sourced.
    let env = [("EFR_MODE", "auto"), ("EFR_MODEL", "gpt-5.4")];
    run_with(
        &home,
        &env,
        r#"
        , one
        ,effort high
        , two
        ,new three
        ,! steer
        ,mode default
        ,model default
        , four
        "#,
    );
    let calls = home.calls();
    assert_eq!(
        args(&calls),
        [
            vec!["send"],
            vec!["settings", "--effort=high"],
            vec!["send"],
            vec!["new"],
            vec!["send", "--steer"],
            vec!["settings"],
            vec!["settings"],
            vec!["send"],
        ]
    );
    let settings: Vec<[Option<&str>; 3]> = calls.iter().map(Call::settings).collect();
    assert_eq!(
        settings,
        [
            [Some("auto"), Some("gpt-5.4"), None],
            // The check of a new value carries the terminal's other values.
            [Some("auto"), Some("gpt-5.4"), None],
            [Some("auto"), Some("gpt-5.4"), Some("high")],
            // A new conversation in this terminal keeps the terminal's choice.
            [Some("auto"), Some("gpt-5.4"), Some("high")],
            // A steer joins a running turn, which keeps its settings.
            [None, None, None],
            [None, Some("gpt-5.4"), Some("high")],
            [None, None, Some("high")],
            [None, None, Some("high")],
        ]
    );
    // An empty variable hides the values that the shell itself exported.
    assert_eq!(calls[4].mode.as_deref(), Some(""));
    assert_eq!(calls[7].model.as_deref(), Some(""));
}

#[test]
fn e2e_a_value_that_efr_settings_refuses_is_not_kept() {
    if !zsh_tests_enabled() {
        return;
    }
    let home = Home::new();
    let (_, status) = run_with(
        &home,
        &[],
        r#"
        ,mode auto
        FAKE_EXIT=2 ,model gpt-9
        efr status $?
        FAKE_EXIT=2 ,effort --help
        , hi
        "#,
    );
    assert_eq!(status, Some(0));
    let calls = home.calls();
    assert_eq!(
        args(&calls),
        [
            vec!["settings", "--mode=auto"],
            vec!["settings", "--model=gpt-9"],
            vec!["status", "2"],
            // A value that looks like a flag still reaches efr as the value to check.
            vec!["settings", "--effort=--help"],
            vec!["send"],
        ]
    );
    assert_eq!(calls[4].settings(), [Some("auto"), None, None]);
}

#[test]
fn e2e_a_bare_setting_shows_its_line_in_the_terminals_words() {
    if !zsh_tests_enabled() {
        return;
    }
    let home = Home::new();
    home.output(
        "settings",
        &SETTINGS
            .replace("mode = cautious  # default", "mode = auto  # EFR_MODE")
            .replace("model = gpt-5.5  # the daemon's default", "model = gpt-5.4  # --model"),
    );
    let (stdout, _) = run_with(
        &home,
        &[("EFR_MODE", "auto")],
        r#"
        ,mode
        ,model gpt-5.4
        ,effort
        "#,
    );
    assert_eq!(
        stdout.lines().collect::<Vec<_>>(),
        [
            "mode = auto  # this terminal (,mode); choices: manual, cautious, auto",
            "model = gpt-5.4  # this terminal (,model); choices: gpt-5.5, gpt-5.4, my-model",
            "effort = medium  # the default of gpt-5.5; choices: low, medium, high",
        ]
    );
}

#[test]
fn e2e_a_setting_takes_one_value() {
    if !zsh_tests_enabled() {
        return;
    }
    let home = Home::new();
    let (_, status) = run_with(&home, &[], ",mode auto cautious || efr status $?");
    assert_eq!(status, Some(0));
    assert_eq!(args(&home.calls()), [vec!["status", "2"]]);
}

#[test]
fn e2e_setting_lines_stay_as_typed_and_are_not_the_last_command() {
    if !zsh_tests_enabled() {
        return;
    }
    let home = Home::new();
    type_lines(
        &home,
        &[
            ",mode auto",
            "fc -ln -1 > history.txt",
            "false",
            ",model default",
            ",effort",
            ", why did it fail",
        ],
    );
    let history = std::fs::read_to_string(home.path().join("history.txt")).unwrap();
    assert_eq!(history.trim_end(), ",mode auto");
    let calls = home.calls();
    let prompt = calls.last().unwrap();
    assert_eq!(prompt.args, ["send"]);
    assert_eq!(prompt.last_command(), Some("false"));
    assert_eq!(prompt.last_status(), 1);
    assert_eq!(prompt.settings(), [Some("auto"), None, None]);
}

#[test]
fn e2e_in_sticky_mode_a_bare_setting_word_runs_as_its_command() {
    if !zsh_tests_enabled() {
        return;
    }
    let home = Home::new();
    std::fs::write(format!("{}.refuse", home.records().display()), "--effort=matters\n").unwrap();
    type_lines_with(
        &home,
        UTF8,
        &[
            "<C-Space>",
            "mode auto",
            "  model  ",
            "effort matters",
            "model the database schema",
            "effort high",
            "what now",
            "mode default",
            "!fc -ln -7 > history.txt",
            "<C-Space>",
        ],
    );
    let calls = home.calls();
    assert_eq!(
        args(&calls),
        [
            // The value is checked before the line becomes the command, which checks it
            // again as it always does.
            vec!["settings", "--mode=auto"],
            vec!["settings", "--mode=auto"],
            vec!["settings"],
            // A value that efr refuses makes the line a prompt.
            vec!["settings", "--effort=matters"],
            vec!["send"],
            vec!["send"],
            vec!["settings", "--effort=high"],
            vec!["settings", "--effort=high"],
            vec!["send"],
            // `default` needs no check.
            vec!["settings"],
        ]
    );
    let prompts: Vec<(Option<&str>, [Option<&str>; 3])> = calls
        .iter()
        .filter(|call| call.args == ["send"])
        .map(|call| (call.prompt.as_deref(), call.settings()))
        .collect();
    assert_eq!(
        prompts,
        [
            (Some("effort matters"), [Some("auto"), None, None]),
            (Some("model the database schema"), [Some("auto"), None, None]),
            (Some("what now"), [Some("auto"), None, Some("high")]),
        ]
    );
    let history = std::fs::read_to_string(home.path().join("history.txt")).unwrap();
    assert_eq!(
        history.lines().map(str::trim).collect::<Vec<_>>(),
        [
            ",mode auto",
            ",model",
            "🤖 effort matters",
            "🤖 model the database schema",
            ",effort high",
            "🤖 what now",
            ",mode default",
        ]
    );
}

/// The fake accepts any value, as a daemon without a model list accepts any model, so
/// only the plugin keeps shell syntax in a value from running.
#[test]
fn e2e_in_sticky_mode_a_setting_value_with_shell_syntax_is_a_prompt() {
    if !zsh_tests_enabled() {
        return;
    }
    let home = Home::new();
    type_lines_with(&home, UTF8, &["<C-Space>", "model a;touch${IFS}x", "<C-Space>"]);
    let calls = home.calls();
    assert_eq!(args(&calls), [vec!["send"]]);
    assert_eq!(calls[0].prompt.as_deref(), Some("model a;touch${IFS}x"));
    assert_eq!(calls[0].settings(), [None, None, None]);
    assert!(!home.path().join("x").exists(), "the value ran as shell code");
}

#[test]
fn e2e_outside_sticky_mode_a_bare_setting_word_is_a_shell_command() {
    if !zsh_tests_enabled() {
        return;
    }
    let home = Home::new();
    type_lines(&home, &["mode() { efr mine \"$@\" }", "mode auto"]);
    assert_eq!(args(&home.calls()), [vec!["mine", "auto"]]);
}

#[test]
fn e2e_an_unknown_comma_word_starts_a_prompt_and_a_users_own_still_runs() {
    if !zsh_tests_enabled() {
        return;
    }
    let home = Home::new();
    let bin = home.path().join("bin/,tool");
    std::fs::write(&bin, "#!/bin/sh\nexec efr tool \"$@\"\n").unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    type_lines_with(
        &home,
        UTF8,
        &[
            ",run sudo pacman -Syu; rm -rf / > x",
            "function ,mine { efr mine \"$@\" }",
            "alias ,al='efr al'",
            ",mine one",
            ",al two",
            ",tool three",
            ",new fresh",
            "<C-Space>",
            ",ask in sticky mode",
            "<C-Space>",
            "fc -ln -10 > history.txt",
        ],
    );
    let calls = home.calls();
    assert_eq!(
        args(&calls),
        [
            vec!["send"],
            vec!["mine", "one"],
            vec!["al", "two"],
            vec!["tool", "three"],
            vec!["new"],
            vec!["send"],
        ]
    );
    let prompts: Vec<Option<&str>> = calls.iter().map(|call| call.prompt.as_deref()).collect();
    assert_eq!(prompts[0], Some("run sudo pacman -Syu; rm -rf / > x"));
    assert_eq!(prompts[4], Some("fresh"));
    assert_eq!(prompts[5], Some("ask in sticky mode"));
    assert!(!home.path().join("x").exists(), "the prompt ran as shell syntax");
    let history = std::fs::read_to_string(home.path().join("history.txt")).unwrap();
    let history: Vec<&str> = history.lines().map(str::trim).collect();
    assert!(history.contains(&", run sudo pacman -Syu; rm -rf / > x"), "{history:?}");
    assert!(history.contains(&", ask in sticky mode"), "{history:?}");
}

#[test]
fn e2e_a_comma_word_with_a_bang_steers_and_a_typo_of_a_command_stays_on_the_line() {
    if !zsh_tests_enabled() {
        return;
    }
    let home = Home::new();
    // Ctrl+U clears the line that the typo left, so the next line starts empty.
    let screen = type_lines(&home, &[",moed auto", "\x15,!stop now", ",now what"]);
    assert!(screen.contains("efr: ,moed is no command; did you mean ,mode?"), "{screen}");
    let calls = home.calls();
    assert_eq!(args(&calls), [vec!["send", "--steer"], vec!["send"]]);
    assert_eq!(calls[0].prompt.as_deref(), Some("stop now"));
    assert_eq!(calls[1].prompt.as_deref(), Some("now what"));
}

/// A widget on Ctrl+X Ctrl+R that records what the line shows while it is typed, with
/// the plugin's highlights after a `|`, and a line-finish hook that records what the
/// line shows once Enter accepted it.
const RECORD_LINES: [&str; 2] = [
    r#"_rec() { print -r -- "$PREDISPLAY$BUFFER|${(j:,:)${(@M)region_highlight:#*memo=efr*}}" >> typed.txt; }; zle -N _rec; bindkey '^X^R' _rec"#,
    r#"_fin() { print -r -- "$PREDISPLAY$BUFFER" >> finished.txt; }; zle -N _fin; add-zle-hook-widget line-finish _fin"#,
];

#[test]
fn e2e_in_sticky_mode_the_settings_stand_before_the_robot_and_stay_on_enter() {
    if !zsh_tests_enabled() {
        return;
    }
    let home = Home::new();
    // A terminal with 256 colours, where zsh draws the grey of the tag.
    let env = [UTF8[0], ("EFR_TEST_TERM", "xterm-256color")];
    type_lines_with(
        &home,
        &env,
        &[
            RECORD_LINES[0],
            RECORD_LINES[1],
            ",mode auto",
            ",model gpt-5.4",
            "<C-Space>",
            "what changed?\u{18}\u{12}",
            ",mode default",
            ",model default",
            "and now\u{18}\u{12}",
            "!fc -ln -4 > history.txt",
            "<C-Space>",
        ],
    );
    let read = |name: &str| std::fs::read_to_string(home.path().join(name)).unwrap();
    // The tag is dim and the robot keeps its own style after it; once the terminal has
    // no settings of its own, only the robot is left. zle gives the end of a highlight
    // back in its own units, so only where each one starts and its style count.
    let typed: Vec<(String, Vec<(String, String)>)> = read("typed.txt")
        .lines()
        .map(|line| {
            let (shown, highlights) = line.rsplit_once('|').unwrap();
            let highlights = highlights
                .split(',')
                .map(|entry| {
                    let words: Vec<&str> = entry.split(' ').collect();
                    assert_eq!(words.len(), 4, "{entry}");
                    assert_eq!(words[3], "memo=efr");
                    (words[0].to_owned(), words[2].to_owned())
                })
                .collect();
            (shown.to_owned(), highlights)
        })
        .collect();
    let entry = |start: &str, style: &str| (start.to_owned(), style.to_owned());
    assert_eq!(
        typed,
        [
            (
                "auto gpt-5.4 🤖 what changed?".to_owned(),
                vec![entry("P0", "fg=8"), entry("P13", "fg=magenta")]
            ),
            ("🤖 and now".to_owned(), vec![entry("P0", "fg=magenta")]),
        ]
    );
    // Enter changed nothing on the screen: the robot became the first word of the line
    // and the tag stayed before it.
    let finished = read("finished.txt");
    assert!(finished.lines().any(|line| line == "auto gpt-5.4 🤖 what changed?"), "{finished}");
    assert!(finished.lines().any(|line| line == "🤖 and now"), "{finished}");
    // History keeps the lines that ran, without the tag.
    assert_eq!(
        read("history.txt").lines().collect::<Vec<_>>(),
        ["🤖 what changed?", ",mode default", ",model default", "🤖 and now"]
    );
    let calls = home.calls();
    let prompts: Vec<(Option<&str>, [Option<&str>; 3])> = calls
        .iter()
        .filter(|call| call.args == ["send"])
        .map(|call| (call.prompt.as_deref(), call.settings()))
        .collect();
    assert_eq!(
        prompts,
        [
            (Some("what changed?"), [Some("auto"), Some("gpt-5.4"), None]),
            (Some("and now"), [None, None, None]),
        ]
    );
}

#[test]
fn the_plugin_completes_every_mode_that_efr_knows() {
    let plugin = std::fs::read_to_string(plugin()).unwrap();
    let names: Vec<&str> = plugin
        .lines()
        .find_map(|line| line.trim().strip_prefix("compadd -- default manual"))
        .map(|rest| std::iter::once("manual").chain(rest.split_whitespace()).collect())
        .unwrap();
    let known: Vec<&str> = efr_protocol::Mode::ALL.iter().map(|mode| mode.as_str()).collect();
    assert_eq!(names, known);
}

#[test]
fn e2e_completion_offers_the_modes_the_models_and_the_models_efforts() {
    if !zsh_tests_enabled() {
        return;
    }
    let home = Home::new();
    home.output("models", "gpt-5.5\ngpt-5.4\nmy-model\n");
    home.output("settings", SETTINGS);
    type_lines(
        &home,
        &[
            // compinit runs after the plugin, as in many a .zshrc: the completions are
            // registered at the next prompt.
            "autoload -Uz compinit; compinit -u -D",
            ",mode au\t",
            ",model my\t",
            ",model gpt-5.4\t",
            ",effort hi\t",
        ],
    );
    let calls = home.calls();
    let args = args(&calls);
    let checks: Vec<&Vec<&str>> =
        args.iter().filter(|args| args.len() == 2 && args[0] == "settings").collect();
    assert_eq!(
        checks,
        [
            &vec!["settings", "--mode=auto"],
            &vec!["settings", "--model=my-model"],
            &vec!["settings", "--model=gpt-5.4"],
            &vec!["settings", "--effort=high"],
        ]
    );
    // The model ids are read once and kept for a minute.
    let lists: Vec<&Vec<&str>> = args.iter().filter(|args| args[0] == "models").collect();
    assert_eq!(lists, [&vec!["models", "--names"]], "{args:?}");
}

/// The uid that owns the temporary home, which is this process's.
fn uid(home: &Home) -> u32 {
    use std::os::unix::fs::MetadataExt as _;
    std::fs::metadata(home.path()).unwrap().uid()
}

/// Shows the notices of `/dev/pts/77` with the variables `env` set and those in
/// `unset` removed, after `script`, and returns what was printed.
fn notices_with(home: &Home, env: &[(&str, &str)], unset: &[&str], script: &str) -> String {
    let full =
        format!("source {}\n{script}\nTTY=/dev/pts/77\n_efr_print_notices\n", plugin().display());
    let mut zsh = home.zsh();
    zsh.envs(env.iter().copied());
    for name in unset {
        zsh.env_remove(name);
    }
    let output = zsh.args(["-f", "-i", "-c", &full]).output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn e2e_notices_follow_the_runtime_rule_with_efr_home() {
    if !zsh_tests_enabled() {
        return;
    }
    let home = Home::new();
    let efr_home = home.path().join("efr-home");
    let below_home = notice(&efr_home.join("runtime"), "from EFR_HOME");
    let named = home.path().join("named");
    notice(&named, "from EFR_RUNTIME_DIR");
    // Home::zsh sets XDG_RUNTIME_DIR to the home, whose efr root holds another notice.
    let xdg = notice(&home.path().join("efr"), "from XDG_RUNTIME_DIR");
    let efr_home_text = efr_home.to_str().unwrap();

    // EFR_HOME/runtime comes before XDG_RUNTIME_DIR.
    assert_eq!(notices_with(&home, &[("EFR_HOME", efr_home_text)], &[], ""), "from EFR_HOME\n");
    assert!(!below_home.exists());
    // EFR_RUNTIME_DIR comes before EFR_HOME.
    let env = [("EFR_HOME", efr_home_text), ("EFR_RUNTIME_DIR", named.to_str().unwrap())];
    assert_eq!(notices_with(&home, &env, &[], ""), "from EFR_RUNTIME_DIR\n");
    // A relative EFR_HOME is an error, as in efr: no root, and no fallback to XDG.
    assert_eq!(notices_with(&home, &[("EFR_HOME", "efr-home")], &[], ""), "");
    assert!(xdg.exists());
    // An empty EFR_HOME counts as unset.
    assert_eq!(notices_with(&home, &[("EFR_HOME", "")], &[], ""), "from XDG_RUNTIME_DIR\n");
}

#[test]
fn e2e_without_xdg_runtime_dir_notices_come_from_a_private_run_user_dir() {
    if !zsh_tests_enabled() {
        return;
    }
    let home = Home::new();
    let run_user = home.path().join("run-user");
    let dir = run_user.join(uid(&home).to_string());
    std::fs::create_dir_all(&dir).unwrap();
    // The plugin's base for /run/user, so the test never reads the real one.
    let script = format!("_efr_run_user={}", run_user.display());

    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    let file = notice(&dir.join("efr"), "from /run/user");
    assert_eq!(notices_with(&home, &[], &["XDG_RUNTIME_DIR"], &script), "", "mode 0755");
    assert!(file.exists());

    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(notices_with(&home, &[], &["XDG_RUNTIME_DIR"], &script), "from /run/user\n");
    assert!(!file.exists());

    // A relative XDG_RUNTIME_DIR counts as unset, as the XDG specification says.
    notice(&dir.join("efr"), "again from /run/user");
    let env = [("XDG_RUNTIME_DIR", "relative")];
    assert_eq!(notices_with(&home, &env, &[], &script), "again from /run/user\n");

    std::fs::remove_dir_all(&dir).unwrap();
    assert_eq!(notices_with(&home, &[], &["XDG_RUNTIME_DIR"], &script), "", "no directory");
}

/// The runtime root that the plugin's rule and the built `efr paths --json` find in
/// the same environment: `HOME` and `XDG_RUNTIME_DIR` at the temporary home, with
/// `env` added and `unset` removed. `None` when there is none: the plugin's rule
/// returns 1, or `efr` refuses the variables.
fn runtime_roots(home: &Home, env: &[(&str, &str)], unset: &[&str]) -> [Option<String>; 2] {
    let mut base = vec![
        ("HOME", home.path().to_str().unwrap()),
        ("PATH", "/usr/bin:/bin"),
        ("XDG_RUNTIME_DIR", home.path().to_str().unwrap()),
    ];
    base.retain(|(name, _)| !unset.contains(name) && !env.iter().any(|(set, _)| set == name));
    base.extend_from_slice(env);

    let script =
        format!("source {}\n_efr_runtime_root && print -r -- $REPLY\n", plugin().display());
    let output = Command::new("zsh")
        .env_clear()
        .envs(base.iter().copied())
        .current_dir(home.path())
        .args(["-f", "-i", "-c", &script])
        .output()
        .unwrap();
    let plugin = String::from_utf8(output.stdout).unwrap();
    let plugin = (!plugin.is_empty()).then(|| plugin.trim_end().to_owned());

    let output = Command::new(env!("CARGO_BIN_EXE_efr"))
        .env_clear()
        .envs(base.iter().copied())
        .current_dir(home.path())
        .args(["paths", "--json"])
        .output()
        .unwrap();
    let efr = output.status.success().then(|| {
        let paths: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        paths["roots"]["runtime"]["path"].as_str().unwrap().to_owned()
    });
    [plugin, efr]
}

/// Variables to set, by name.
type Vars<'a> = &'a [(&'a str, &'a str)];

#[test]
fn e2e_efr_paths_and_the_plugin_find_the_same_runtime_root() {
    if !zsh_tests_enabled() {
        return;
    }
    let home = Home::new();
    let path = |name: &str| home.path().join(name).to_str().unwrap().to_owned();
    let (efr_home, named) = (path("efr-home"), path("named"));
    // NOTE: every environment names a runtime root below the temporary home, or none,
    // so `efr paths` never asks a daemon outside it. The /run/user fallback is left to
    // the unit tests of both sides, which point it at a temporary tree.
    let cases: [(Vars<'_>, &[&str], Option<String>); 7] = [
        (&[], &[], Some(path("efr"))),
        (&[("EFR_HOME", &efr_home)], &[], Some(format!("{efr_home}/runtime"))),
        (&[("EFR_HOME", &efr_home)], &["XDG_RUNTIME_DIR"], Some(format!("{efr_home}/runtime"))),
        (&[("EFR_HOME", &efr_home), ("EFR_RUNTIME_DIR", &named)], &[], Some(named.clone())),
        (
            &[("EFR_HOME", &efr_home), ("EFR_RUNTIME_DIR", "")],
            &[],
            Some(format!("{efr_home}/runtime")),
        ),
        (&[("EFR_HOME", "")], &[], Some(path("efr"))),
        (&[("EFR_HOME", "efr-home")], &[], None),
    ];
    for (env, unset, expected) in cases {
        let [plugin, efr] = runtime_roots(&home, env, unset);
        assert_eq!(plugin, efr, "the plugin and efr differ for {env:?} without {unset:?}");
        assert_eq!(plugin, expected, "for {env:?} without {unset:?}");
    }
    let relative = runtime_roots(&home, &[("EFR_RUNTIME_DIR", "named")], &[]);
    assert_eq!(relative, [None, None], "a relative EFR_RUNTIME_DIR gives no root");
}

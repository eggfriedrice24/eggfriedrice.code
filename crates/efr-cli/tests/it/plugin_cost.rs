//! The bench of the zsh plugin's cost per prompt (`bench_perf_zsh_prompt`), which
//! `just bench-perf` runs in release: `_efr_preexec` and `_efr_precmd` around each of
//! 200 prompts, timed by `$EPOCHREALTIME` inside zsh, the `source` of the plugin, and
//! the redraw hooks of the warm-up (`_efr_warm_line`), of the colours of `@path` words
//! (`_efr_attach_colour`) and of the paste mode (`_efr_paste_line`) for each key of a
//! command line and of a prompt line, which have a budget. The keys go to a real line
//! editor: a second `zsh -f -i` on a pseudo-terminal of zsh/zpty, where `BUFFER` is the
//! special parameter of ZLE.
//!
//! zsh starts as `zsh -f -i` with its stdin on a pseudo-terminal, a cleared
//! environment, a temp `HOME`, an empty runtime root of its own (`EFR_RUNTIME_DIR`)
//! and a fake `efr` on `PATH` that records each call; the bench asserts that no prompt
//! ran it. `TERM=dumb`, so the plugin asks the terminal nothing at load. The ladder:
//! 50, 500 and 5,000 exported variables; 0 and 1,000 draft files of another shell;
//! a `PWD` of 3 and 12 dirs below the home; no projects file and one of 20 projects; a command line
//! of about 60 and of 2,000 characters; the prompt corner (`efr_corner`, with efrd's
//! `prompt.tsv` and its colours in truecolor), whose precmd builds the corner's text
//! and whose `RPROMPT` the bench expands as zsh does for each prompt; the wake FIFO of
//! the terminal, which the first precmd opens and each one empties.

use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use efr_test_support::bench::Report;

/// The prompts of each case.
const PROMPTS: usize = 200;

/// A fake `efr` that leaves a file for each call.
const FAKE_EFR: &str = "#!/bin/sh\n: > \"$EFR_CALLS.$$\"\n";

/// Sources the plugin, then runs `$EFR_BENCH_PROMPTS` prompts; one row each into
/// `$EFR_BENCH_OUT_FILE`, after the time of the `source` and the `TTY` that zsh sees.
/// Then the rows of the redraw hooks in a line editor ([`ZLE_SETUP`]) follow.
const SCRIPT: &str = r#"
zmodload zsh/datetime || exit 90
typeset -F loading=$EPOCHREALTIME
source $EFR_BENCH_PLUGIN
(( EFR_BENCH_CORNER )) && efr_corner
typeset -F loaded=$EPOCHREALTIME
typeset -a out
out+=("load $(( (loaded - loading) * 1e6 ))")
out+=("tty ${TTY:-none}")
cd -- $EFR_BENCH_PWD || exit 91
integer i
typeset -F a b c
for (( i = 1; i <= EFR_BENCH_PROMPTS; i++ )); do
  a=$EPOCHREALTIME
  _efr_preexec "$EFR_BENCH_LINE $i"
  b=$EPOCHREALTIME
  _efr_precmd
  # NOTE: zsh expands RPROMPT for each prompt; with the corner, it is the plugin's.
  (( EFR_BENCH_CORNER )) && : ${(%)${(e)RPROMPT}}
  c=$EPOCHREALTIME
  out+=("prompt $(( (b - a) * 1e6 )) $(( (c - b) * 1e6 ))")
done
print -rl -- $out > $EFR_BENCH_OUT_FILE
# The redraw hooks for each key, in a real line editor: a zsh on a pseudo-terminal of
# zsh/zpty reads the keys as a person types them, and $EFR_BENCH_ZLE times the hooks
# there. Each key waits for its echo, so ZLE redraws the line for each one. The keys:
# the command line (its tabs as blanks, so no completion runs), then a prompt line
# with two @path words that name files of the PWD, whose first key that makes it a
# prompt starts the warm-up (with no socket, no efr runs).
: > shot.png
: > notes.md
zmodload zsh/zpty zsh/zselect || exit 92
zpty zle 'TERM=xterm zsh -f -i' || exit 93
integer fd=$REPLY
# Reads the output of the line editor until it holds the pattern $1, for at most 5
# seconds at each wait.
shown() {
  local more seen
  while [[ $seen != *$~1* ]]; do
    zselect -t 500 -r $fd || exit 94
    zpty -r -t zle more && seen+=$more
  done
}
local ready=$'\e\\[\\?2004h' typed key
shown $ready
zpty -w -n zle "source ${(q)EFR_BENCH_ZLE}"$'\r'
shown $ready
for typed in "${EFR_BENCH_LINE//$'\t'/ }" ", compare the header in @shot.png with @notes.md and @alice"; do
  for (( i = 1; i <= $#typed; i++ )); do
    key=${typed[i]}
    zpty -w -n zle $key
    shown ${(b)key}
  done
  zpty -w -n zle $'\C-u'
done
zpty -w -n zle $'_bench_done\r'
shown $ready
zpty -w -n zle $'exit\r'
zpty -d zle
"#;

/// Sourced in the zsh of the line editor: the plugin, with each of its redraw hooks
/// timed by a hook of the bench, which writes one row per key and hook at
/// `_bench_done`. `EFR_TERMINAL_BG` is set, so the plugin asks the terminal nothing.
const ZLE_SETUP: &str = r#"
EFR_TERMINAL_BG=dark
source $EFR_BENCH_PLUGIN
zmodload zsh/datetime
typeset -ga _bench_rows
_bench_time() {
  local -F a=$EPOCHREALTIME
  $2
  _bench_rows+=("$1 $(( (EPOCHREALTIME - a) * 1e6 ))")
}
_bench_warm() { _bench_time key _efr_warm_line }
_bench_attach() { _bench_time attach _efr_attach_colour }
_bench_paste() { _bench_time paste _efr_paste_line }
for hook in _efr_warm_line _efr_attach_colour _efr_paste_line; do
  add-zle-hook-widget -d line-pre-redraw $hook
done
for hook in _bench_warm _bench_attach _bench_paste; do
  add-zle-hook-widget line-pre-redraw $hook
done
_bench_done() { print -rl -- $_bench_rows >> $EFR_BENCH_OUT_FILE }
"#;

fn plugin() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../shell/zsh/efr.plugin.zsh")
}

/// One case of the ladder.
#[derive(Debug, Clone, Copy)]
struct Case {
    vars: usize,
    drafts: usize,
    depth: usize,
    projects: usize,
    line: usize,
    /// `efr_corner` in `.zshrc`, with efrd's `prompt.tsv`.
    corner: bool,
    /// The wake FIFO of the terminal, as efrd makes it: the first precmd opens it, and
    /// each one empties it.
    wake: bool,
}

impl Case {
    fn label(&self) -> String {
        format!(
            "vars {} drafts {} pwd depth {} projects {} line {} corner {} wake {}",
            self.vars, self.drafts, self.depth, self.projects, self.line, self.corner, self.wake
        )
    }
}

/// What one zsh measured.
struct Run {
    load: Duration,
    preexec: Vec<Duration>,
    precmd: Vec<Duration>,
    /// The redraw hook of the warm-up, for each key of a command line and a prompt.
    keys: Vec<Duration>,
    /// The redraw hook of the colours of `@path` words, for each key of a command line
    /// and a prompt with two of them.
    attach: Vec<Duration>,
    /// The redraw hook of the paste mode, for each key of both lines.
    paste: Vec<Duration>,
    tty: String,
}

fn micros(text: &str) -> Duration {
    Duration::from_secs_f64(text.parse::<f64>().unwrap() / 1e6)
}

/// A command line of about `length` characters, with tabs, quotes and backslashes
/// for the JSON escapes; a usual one for 0.
fn command_line(length: usize) -> String {
    if length == 0 {
        return "cargo nextest run -p efr-cli --release -E 'test(/plugin/)'".to_owned();
    }
    let mut line = String::from("printf '%s\\n' ");
    while line.len() < length {
        line.push_str("word\t\"quoted\" \\back ");
    }
    line.truncate(length);
    line
}

/// Runs `prompts` prompts of `case` in one zsh.
fn run(zsh: &str, case: Case, prompts: usize) -> Run {
    let base = Path::new(env!("CARGO_TARGET_TMPDIR")).join("perf-zsh");
    std::fs::create_dir_all(&base).unwrap();
    let dir = tempfile::tempdir_in(base).unwrap();
    let home = dir.path().join("home");
    let bin = dir.path().join("bin");
    let runtime = dir.path().join("runtime");
    for path in [&home, &bin, &runtime] {
        std::fs::create_dir(path).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    std::fs::write(bin.join("efr"), FAKE_EFR).unwrap();
    std::fs::set_permissions(bin.join("efr"), std::fs::Permissions::from_mode(0o755)).unwrap();
    if case.drafts > 0 {
        // NOTE: drafts of another shell that still runs (this test) in this pid
        // namespace, so the clean at load keeps them and each prompt's glob meets them.
        let ns = std::fs::metadata("/proc/self/ns/pid").unwrap().ino();
        let drafts = runtime.join("drafts");
        std::fs::create_dir(&drafts).unwrap();
        let pid = std::process::id();
        for n in 0..case.drafts {
            std::fs::write(drafts.join(format!("{pid}.1761300000{n:06}.{ns}.1")), "a draft")
                .unwrap();
        }
    }
    // NOTE: the project is the first dir below the home, so every PWD is inside it.
    let project = home.join("dir0");
    let mut pwd = home.clone();
    for n in 0..case.depth {
        pwd.push(format!("dir{n}"));
    }
    std::fs::create_dir_all(&pwd).unwrap();
    if case.projects > 0 {
        let mut table = String::new();
        for n in 1..case.projects {
            table.push_str(&format!("/srv/projects/app{n}\tapp{n}\n"));
        }
        table.push_str(&format!("{}\tp\n", project.display()));
        std::fs::write(runtime.join("projects.tsv"), table).unwrap();
    }
    if case.corner {
        std::fs::write(
            runtime.join("prompt.tsv"),
            "mode\tauto\ncolor.text\t#d8d3c3\ncolor.muted\t#8695b7\ncolor.error\t#e06c75\n",
        )
        .unwrap();
    }
    let out_file = dir.path().join("bench.out");
    let zle_setup = dir.path().join("zle.zsh");
    std::fs::write(&zle_setup, ZLE_SETUP).unwrap();
    let calls = dir.path().join("calls");
    let (master, slave) = super::picker::pty();
    if case.wake {
        use std::os::fd::AsRawFd as _;
        let tty = std::fs::read_link(format!("/proc/self/fd/{}", slave.as_raw_fd())).unwrap();
        let name = tty.to_str().unwrap().trim_start_matches("/dev/").replace('/', "-");
        let notices = runtime.join("notices");
        std::fs::create_dir(&notices).unwrap();
        let fifo = notices.join(format!("{name}.wake"));
        rustix::fs::mkfifoat(rustix::fs::CWD, &fifo, rustix::fs::Mode::from_raw_mode(0o600))
            .unwrap();
    }
    let mut command = efr_stdx::process::command(zsh, &home).into_std();
    command
        .env_clear()
        .env("HOME", &home)
        .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
        .env("TERM", "dumb")
        .env("XDG_RUNTIME_DIR", dir.path())
        .env("EFR_RUNTIME_DIR", &runtime)
        .env("EFR_CALLS", &calls)
        .env("EFR_BENCH_PLUGIN", plugin())
        .env("EFR_BENCH_PWD", &pwd)
        .env("EFR_BENCH_PROMPTS", prompts.to_string())
        .env("EFR_BENCH_LINE", command_line(case.line))
        .env("EFR_BENCH_OUT_FILE", &out_file)
        .env("EFR_BENCH_ZLE", &zle_setup)
        .env("EFR_BENCH_CORNER", u8::from(case.corner).to_string())
        .env("COLORTERM", "truecolor")
        .envs(
            (0..case.vars)
                .map(|n| (format!("BENCH_VAR_{n:05}"), format!("value-{n}-of-the-bench"))),
        )
        .current_dir(&home)
        .args(["-f", "-i", "-c", SCRIPT])
        .stdin(slave.try_clone().unwrap())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let output = command.output().unwrap();
    drop(command);
    drop(slave);
    drop(master);
    assert!(output.status.success(), "zsh failed: {}", String::from_utf8_lossy(&output.stderr));
    let text = std::fs::read_to_string(&out_file).unwrap();
    let mut run = Run {
        load: Duration::ZERO,
        preexec: Vec::new(),
        precmd: Vec::new(),
        keys: Vec::new(),
        attach: Vec::new(),
        paste: Vec::new(),
        tty: String::new(),
    };
    for row in text.lines() {
        let words: Vec<&str> = row.split_whitespace().collect();
        match words[..] {
            ["load", load] => run.load = micros(load),
            ["tty", tty] => tty.clone_into(&mut run.tty),
            ["prompt", pre, post] => {
                run.preexec.push(micros(pre));
                run.precmd.push(micros(post));
            }
            ["key", key] => run.keys.push(micros(key)),
            ["attach", key] => run.attach.push(micros(key)),
            ["paste", key] => run.paste.push(micros(key)),
            _ => panic!("an unknown row {row:?}"),
        }
    }
    assert_eq!(run.precmd.len(), prompts, "{text}");
    let ran_efr = std::fs::read_dir(dir.path())
        .unwrap()
        .filter(|entry| entry.as_ref().unwrap().file_name().to_string_lossy().starts_with("calls."))
        .count();
    assert_eq!(ran_efr, 0, "a prompt runs no efr, and a key no warm-up without a daemon");
    run
}

#[expect(clippy::print_stderr, reason = "a bench says what it skipped")]
#[test]
#[ignore = "bench: run by just bench-perf"]
fn bench_perf_zsh_prompt() {
    let mut report = Report::new("bench_perf_zsh_prompt");
    let Some(zsh) = ["/usr/bin/zsh", "/bin/zsh"].into_iter().find(|path| Path::new(path).exists())
    else {
        eprintln!("bench_perf_zsh_prompt: skipped: no zsh at /usr/bin or /bin");
        report.count("skipped_no_zsh", 1);
        report.finish();
        return;
    };
    let base =
        Case { vars: 50, drafts: 0, depth: 3, projects: 0, line: 0, corner: false, wake: false };
    let cases = [
        base,
        Case { vars: 500, ..base },
        Case { vars: 5_000, ..base },
        Case { drafts: 1_000, ..base },
        Case { depth: 12, ..base },
        Case { projects: 20, ..base },
        Case { projects: 20, depth: 12, ..base },
        Case { line: 2_000, ..base },
        Case { projects: 20, corner: true, ..base },
        Case { wake: true, ..base },
    ];
    let mut var_points = Vec::new();
    let mut tty_set = 0;
    let mut worst_key = Duration::ZERO;
    let mut worst_attach = Duration::ZERO;
    let mut worst_paste = Duration::ZERO;
    for case in cases {
        let run = run(zsh, case, PROMPTS);
        tty_set = u64::from(run.tty != "none");
        let total: Vec<_> = run.preexec.iter().zip(&run.precmd).map(|(a, b)| *a + *b).collect();
        let label = case.label();
        report.time(&format!("{label}: preexec"), &run.preexec);
        report.time(&format!("{label}: precmd"), &run.precmd);
        let stats = report.time(&format!("{label}: per prompt"), &total);
        report.time(&format!("{label}: source"), &[run.load]);
        let keys = report.time(&format!("{label}: warm-up hook per key"), &run.keys);
        worst_key = worst_key.max(keys.p50);
        let attach = report.time(&format!("{label}: @path colour hook per key"), &run.attach);
        worst_attach = worst_attach.max(attach.p50);
        let paste = report.time(&format!("{label}: paste mode hook per key"), &run.paste);
        worst_paste = worst_paste.max(paste.p50);
        if case.drafts == 0
            && case.depth == 3
            && case.projects == 0
            && case.line == 0
            && !case.corner
            && !case.wake
        {
            var_points.push((case.vars as u64, stats.p50));
        }
    }
    report.slope("per prompt over exported vars", &var_points);
    let loads: Vec<_> = (0..10).map(|_| run(zsh, base, 1).load).collect();
    report.time("source the plugin (base, 10 shells)", &loads);
    report.count("TTY is set in zsh", tty_set);
    report.finish();
    // The budget: a redraw hook runs for each key, so it stays a few tests of a
    // variable with no fork (about 10 to 30 µs in zsh on a desktop machine).
    assert!(worst_key < KEY_BUDGET, "warm-up hook per key: p50 {worst_key:?}");
    assert!(worst_attach < KEY_BUDGET, "@path colour hook per key: p50 {worst_attach:?}");
    assert!(worst_paste < KEY_BUDGET, "paste mode hook per key: p50 {worst_paste:?}");
}

/// The budget of each redraw hook of the plugin for one key (p50 of each case).
const KEY_BUDGET: Duration = Duration::from_micros(200);

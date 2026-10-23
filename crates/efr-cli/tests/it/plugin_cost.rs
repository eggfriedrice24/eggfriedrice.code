//! The bench of the zsh plugin's cost per prompt (`bench_perf_zsh_prompt`), which
//! `just bench-perf` runs in release: `_efr_preexec` and `_efr_precmd` around each of
//! 200 prompts, timed by `$EPOCHREALTIME` inside zsh, and the `source` of the plugin.
//!
//! zsh starts as `zsh -f -i` with its stdin on a pseudo-terminal, a cleared
//! environment, a temp `HOME`, an empty runtime root of its own (`EFR_RUNTIME_DIR`)
//! and a fake `efr` on `PATH` that records each call; the bench asserts that no prompt
//! ran it. `TERM=dumb`, so the plugin asks the terminal nothing at load. The ladder:
//! 50, 500 and 5,000 exported variables; 0 and 1,000 draft files of another shell;
//! a `PWD` of 3 and 12 dirs below the home; no projects file and one of 20 projects; a command line
//! of about 60 and of 2,000 characters.

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
const SCRIPT: &str = r#"
zmodload zsh/datetime || exit 90
typeset -F loading=$EPOCHREALTIME
source $EFR_BENCH_PLUGIN
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
  c=$EPOCHREALTIME
  out+=("prompt $(( (b - a) * 1e6 )) $(( (c - b) * 1e6 ))")
done
print -rl -- $out > $EFR_BENCH_OUT_FILE
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
}

impl Case {
    fn label(&self) -> String {
        format!(
            "vars {} drafts {} pwd depth {} projects {} line {}",
            self.vars, self.drafts, self.depth, self.projects, self.line
        )
    }
}

/// What one zsh measured.
struct Run {
    load: Duration,
    preexec: Vec<Duration>,
    precmd: Vec<Duration>,
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
    let out_file = dir.path().join("bench.out");
    let calls = dir.path().join("calls");
    let (master, slave) = super::picker::pty();
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
    let mut run =
        Run { load: Duration::ZERO, preexec: Vec::new(), precmd: Vec::new(), tty: String::new() };
    for row in text.lines() {
        let words: Vec<&str> = row.split_whitespace().collect();
        match words[..] {
            ["load", load] => run.load = micros(load),
            ["tty", tty] => tty.clone_into(&mut run.tty),
            ["prompt", pre, post] => {
                run.preexec.push(micros(pre));
                run.precmd.push(micros(post));
            }
            _ => panic!("an unknown row {row:?}"),
        }
    }
    assert_eq!(run.precmd.len(), prompts, "{text}");
    let ran_efr = std::fs::read_dir(dir.path())
        .unwrap()
        .filter(|entry| entry.as_ref().unwrap().file_name().to_string_lossy().starts_with("calls."))
        .count();
    assert_eq!(ran_efr, 0, "a prompt runs no efr");
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
    let base = Case { vars: 50, drafts: 0, depth: 3, projects: 0, line: 0 };
    let cases = [
        base,
        Case { vars: 500, ..base },
        Case { vars: 5_000, ..base },
        Case { drafts: 1_000, ..base },
        Case { depth: 12, ..base },
        Case { projects: 20, ..base },
        Case { projects: 20, depth: 12, ..base },
        Case { line: 2_000, ..base },
    ];
    let mut var_points = Vec::new();
    let mut tty_set = 0;
    for case in cases {
        let run = run(zsh, case, PROMPTS);
        tty_set = u64::from(run.tty != "none");
        let total: Vec<_> = run.preexec.iter().zip(&run.precmd).map(|(a, b)| *a + *b).collect();
        let label = case.label();
        report.time(&format!("{label}: preexec"), &run.preexec);
        report.time(&format!("{label}: precmd"), &run.precmd);
        let stats = report.time(&format!("{label}: per prompt"), &total);
        report.time(&format!("{label}: source"), &[run.load]);
        if case.drafts == 0 && case.depth == 3 && case.projects == 0 && case.line == 0 {
            var_points.push((case.vars as u64, stats.p50));
        }
    }
    report.slope("per prompt over exported vars", &var_points);
    let loads: Vec<_> = (0..10).map(|_| run(zsh, base, 1).load).collect();
    report.time("source the plugin (base, 10 shells)", &loads);
    report.count("TTY is set in zsh", tty_set);
    report.finish();
}

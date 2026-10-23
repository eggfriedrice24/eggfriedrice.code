use std::io::{Read as _, Write as _};
use std::os::unix::process::ExitStatusExt as _;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use efr_test_support::bench::Report;
use pretty_assertions::assert_eq;
use rstest::rstest;

use super::{Declared, declared};
use crate::shell_tool::words::split;

const HOME: &str = "/home/u";
const CWD: &str = "/home/u/p/app";

fn paths(list: &[&str]) -> Vec<PathBuf> {
    list.iter().map(PathBuf::from).collect()
}

fn declare(line: &str) -> Declared {
    declared(&split(line), Path::new(CWD), Path::new(HOME))
}

#[rstest]
#[case::relative("cat src/main.rs ../x", &["/home/u/p/app/src/main.rs", "/home/u/p/x"], &[], &[])]
#[case::tilde("cat ~/.ssh/id_ed25519 ~", &["/home/u/.ssh/id_ed25519", "/home/u"], &[], &[])]
#[case::absolute("cat /etc/hosts", &["/etc/hosts"], &[], &[])]
#[case::quoted_tilde("cat '~/x'", &["/home/u/p/app/~/x"], &[], &[])]
#[case::once("cat a a", &["/home/u/p/app/a"], &[], &[])]
#[case::implicit_cwd("ls", &["/home/u/p/app"], &[], &[])]
#[case::search_cwd("rg TOKEN", &[], &["/home/u/p/app"], &[])]
#[case::search_home("rg TOKEN ~/.aws", &[], &["/home/u/.aws"], &[])]
#[case::glob("cat ~/.ss*/id*", &[], &["/home/u"], &[])]
#[case::relative_glob("wc -l src/*.rs", &[], &["/home/u/p/app/src"], &[])]
#[case::bare_glob("cat x*", &[], &["/home/u/p/app"], &[])]
#[case::root_glob("ls /*", &[], &["/"], &[])]
#[case::input("wc -c < ~/.netrc", &["/home/u/.netrc"], &[], &[])]
#[case::output("echo x >> ~/.zshrc", &[], &[], &["/home/u/.zshrc"])]
#[case::null("ls > /dev/null", &["/home/u/p/app"], &[], &[])]
#[case::inside_substitution("echo $(cat ~/.ssh/id_rsa)", &["/home/u/.ssh/id_rsa"], &[], &[])]
#[case::every_program("diff ~/.ssh/id_rsa /tmp/k", &[], &["/home/u/.ssh/id_rsa", "/tmp/k"], &[])]
// Writer programs write their operands; cp reads its sources.
#[case::cp("cp ~/.ssh/id_rsa /tmp/k", &[], &["/home/u/.ssh/id_rsa"], &["/tmp/k"])]
#[case::rm_parent("rm -rf ..", &[], &[], &["/home/u/p"])]
#[case::mv_out("mv src ~/x", &[], &[], &["/home/u/p/app/src", "/home/u/x"])]
#[case::glob_write("rm src/*.o", &[], &[], &["/home/u/p/app/src"])]
#[case::write_after_lost_cd("cd - && rm x", &[], &[], &["/"])]
#[case::git_rm("git rm -r src", &[], &[], &["/home/u/p/app/src"])]
// What an earlier cp, ln or mv of the line wrote may be a link that leads anywhere.
#[case::through_a_new_link(
    "ln -s ~ h && cat h/.ssh/id_ed25519",
    &["/home/u", "/home/u/p/app/h/.ssh/id_ed25519"],
    &["/"],
    &["/home/u/p/app/h"]
)]
#[case::write_through_a_new_link(
    "ln -s ~/.config/efr c && cp x c/config.toml",
    &["/home/u/.config/efr"],
    &["/home/u/p/app/x"],
    &["/home/u/p/app/c", "/", "/home/u/p/app/c/config.toml"]
)]
#[case::cd_into_a_new_link(
    "ln -s ~/.ssh k && cd k && cat id_ed25519",
    &["/home/u/.ssh", "/home/u/p/app/k"],
    &["/"],
    &["/home/u/p/app/k"]
)]
#[case::through_a_copy("cp -r a b && cat b/x", &["/home/u/p/app/b/x"], &["/home/u/p/app/a", "/"], &["/home/u/p/app/b"])]
#[case::mkdir_makes_no_link("mkdir -p out && cp a out/", &[], &["/home/u/p/app/a"], &["/home/u/p/app/out"])]
#[case::echo_names_nothing("echo ~/.ssh/id_rsa", &[], &[], &[])]
#[case::home_variable("cat $HOME/.ssh/id_rsa", &["/home/u/.ssh/id_rsa"], &[], &[])]
fn declares(
    #[case] line: &str,
    #[case] reads: &[&str],
    #[case] trees: &[&str],
    #[case] writes: &[&str],
) {
    let expected = Declared { reads: paths(reads), trees: paths(trees), writes: paths(writes) };
    assert_eq!(declare(line), expected, "{line:?}");
}

#[rstest]
// A cd adds its target as a directory the rest of the line may run in.
#[case::cd_relative(
    "cd .. && cat id_ed25519",
    &["/home/u/p", "/home/u/p/app/id_ed25519", "/home/u/p/id_ed25519"],
    &[]
)]
#[case::cd_into_secrets("cd ~/.ssh && cat id_ed25519", &["/home/u/.ssh", "/home/u/p/app/id_ed25519", "/home/u/.ssh/id_ed25519"], &[])]
#[case::cd_home("cd && ls", &["/home/u/p/app", "/home/u"], &[])]
#[case::two_steps(
    "cd ~ && cd .ssh && cat x",
    &[
        "/home/u",
        "/home/u/p/app/.ssh",
        "/home/u/.ssh",
        "/home/u/p/app/x",
        "/home/u/x",
        "/home/u/p/app/.ssh/x",
        "/home/u/.ssh/x",
    ],
    &[]
)]
// A cd the text cannot follow makes every relative path anything below /.
#[case::cd_back("cd - && cat id_ed25519", &[], &["/"])]
#[case::cd_variable("cd $DIR && ls", &[], &["/"])]
#[case::popd("popd; cat ~/x", &["/home/u/x"], &[])]
fn follows_cd(#[case] line: &str, #[case] reads: &[&str], #[case] trees: &[&str]) {
    let declared = declare(line);
    assert_eq!(declared.reads, paths(reads), "{line:?}");
    assert_eq!(declared.trees, paths(trees), "{line:?}");
}

// The bench of `declared` on chains of `cd` (`just bench-perf`, section 2.3 of the perf
// plan). Each `cd` adds its target below every directory so far, so the work doubles
// with each one. A depth can run for hours, and a test cannot stop a thread, so each
// depth runs in a child process: this test binary again, with `CASE` naming the depth,
// which runs `declare_cds_case` only. The parent kills a child that runs too long.

/// The variable that names the case of a child process, such as `cds:15`.
const CASE: &str = "EFR_BENCH_DECLARE_CASE";

/// The full name of the test that a child process runs.
const CHILD_TEST: &str = "shell_tool::declare::tests::declare_cds_case";

/// A depth that takes longer than this stops the ladder.
const LIMIT: Duration = Duration::from_secs(10);

/// How much longer than [`LIMIT`] a child process may run before it is killed.
const KILL_AFTER: Duration = Duration::from_secs(5);

/// The prefix of the lines that a child process reports.
const LINE: &str = "declare-case ";

/// `cd d1 && ... && cd dK && cat x` for `cds`, or `k` groups `(cd gN && make)` joined by
/// `&&` for `groups`.
fn cd_line(kind: &str, k: usize) -> String {
    let parts: Vec<String> = match kind {
        "cds" => (1..=k).map(|n| format!("cd d{n}")).chain(["cat x".to_owned()]).collect(),
        _ => (1..=k).map(|n| format!("(cd g{n} && make)")).collect(),
    };
    parts.join(" && ")
}

/// What a child process gave.
enum Outcome {
    /// `declared` returned: the time of each run, the count of declared paths, and the
    /// count of directories that `cat x` ran in.
    Done { samples: Vec<Duration>, paths: u64, bases: u64 },
    /// The process died.
    Crashed { signal: Option<i32> },
    /// The process ran past [`LIMIT`] and [`KILL_AFTER`] and was killed.
    TimedOut,
}

#[expect(
    clippy::disallowed_methods,
    reason = "the bench runs its own test binary as a blocking child process"
)]
fn spawn(case: &str) -> Child {
    Command::new(std::env::current_exe().unwrap())
        .args([CHILD_TEST, "--exact", "--ignored", "--nocapture", "--test-threads=1"])
        .env_clear()
        .env(CASE, case)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap()
}

/// Runs `case` in a child process and reads what it reported.
fn child(case: &str) -> Outcome {
    let mut child = spawn(case);
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if start.elapsed() > LIMIT + KILL_AFTER {
            let _ = child.kill();
            let _ = child.wait();
            return Outcome::TimedOut;
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    let (mut out, mut err) = (String::new(), String::new());
    child.stdout.take().unwrap().read_to_string(&mut out).unwrap();
    child.stderr.take().unwrap().read_to_string(&mut err).unwrap();
    if !status.success() {
        return Outcome::Crashed { signal: status.signal() };
    }
    let (mut samples, mut paths, mut bases) = (Vec::new(), 0, 0);
    // NOTE: libtest writes the name of the test before the child's first line, on the
    // same line, so a report line may start anywhere in a line.
    for line in out.lines().filter_map(|line| line.find(LINE).map(|at| &line[at + LINE.len()..])) {
        if let Some(ns) = line.strip_prefix("sample_ns=") {
            samples.push(Duration::from_nanos(ns.parse().unwrap()));
        } else if let Some(count) = line.strip_prefix("paths=") {
            paths = count.parse().unwrap();
        } else if let Some(count) = line.strip_prefix("bases=") {
            bases = count.parse().unwrap();
        }
    }
    assert!(!samples.is_empty(), "the child of {case} reported nothing:\n{out}\n{err}");
    Outcome::Done { samples, paths, bases }
}

/// The child process of [`bench_perf_declare_cds`]: declares the line of the case of
/// [`CASE`] and prints the time of each run. Without the variable it does nothing.
#[test]
#[ignore = "bench: the child process of bench_perf_declare_cds"]
fn declare_cds_case() {
    let Some(case) = std::env::var_os(CASE) else {
        return;
    };
    let case = case.into_string().unwrap();
    let (kind, k) = case.split_once(':').unwrap();
    let line = split(&cd_line(kind, k.parse().unwrap()));
    let mut runs = Vec::new();
    let mut found = Declared::default();
    let start = Instant::now();
    while runs.len() < 5 && (runs.is_empty() || start.elapsed() < Duration::from_secs(2)) {
        let one = Instant::now();
        found = declared(&line, Path::new(CWD), Path::new(HOME));
        runs.push(one.elapsed());
    }
    let paths = found.reads.len() + found.trees.len() + found.writes.len();
    let bases = found.reads.iter().filter(|path| path.ends_with("x")).count();
    let mut out = std::io::stdout().lock();
    for run in runs {
        writeln!(out, "{LINE}sample_ns={}", run.as_nanos()).unwrap();
    }
    writeln!(out, "{LINE}paths={paths}").unwrap();
    writeln!(out, "{LINE}bases={bases}").unwrap();
}

#[test]
#[ignore = "bench: run by just bench-perf"]
fn bench_perf_declare_cds() {
    let mut report = Report::new("bench_perf_declare_cds");
    for kind in ["cds", "groups"] {
        let what =
            if kind == "cds" { "cd d1 && .. && cd dK && cat x" } else { "(cd gN && make) x K" };
        let mut points = Vec::new();
        for k in [5_u64, 10, 15, 20, 25] {
            match child(&format!("{kind}:{k}")) {
                Outcome::Done { samples, paths, bases } => {
                    let stats = report.time(&format!("{what}, K={k}"), &samples);
                    report.count(&format!("{kind} K={k}: declared paths"), paths);
                    if kind == "cds" {
                        report.count(&format!("{kind} K={k}: bases of cat x"), bases);
                    }
                    points.push((k, stats.p50));
                    if stats.p50 > LIMIT {
                        report.count(&format!("{kind} stopped_at_n"), k);
                        break;
                    }
                }
                Outcome::Crashed { signal } => {
                    let signal = signal.map_or(0, |signal| signal as u64);
                    report.count(&format!("{kind} K={k}: crashed, signal"), signal);
                    report.count(&format!("{kind} stopped_at_n"), k);
                    break;
                }
                Outcome::TimedOut => {
                    report.count(&format!("{kind} K={k}: killed after 15 s"), 1);
                    report.count(&format!("{kind} stopped_at_n"), k);
                    break;
                }
            }
        }
        report.slope(&format!("{kind} slope over K"), &points);
    }
    report.finish();
}

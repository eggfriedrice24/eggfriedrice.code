//! The benches of the engine (`just bench-perf`, section 2.3 of the perf plan): the
//! decision of long lines under the user's ask rules and with many declared paths, the
//! fact requests of long lines, and the base cost of one decision. A cost that grows
//! with the line has a budget on its slope: [`MAX_SLOPE`]. Each bench writes its
//! report first, then checks its budget.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use efr_permissions::exits::fact_requests;
use efr_permissions::{
    Action, AutoSupport, CallFacts, CommandPattern, ConversationPolicy, DecisionInput, Effect,
    Engine, Locations, Policy, Requirements, Resource, Rule, TargetKind,
};
use efr_protocol::{Mode, Origin, ProjectId, Scope};
use efr_test_support::bench::{self, Memory, Report};

const HOME: &str = "/home/u";
const PROJECT: &str = "/home/u/p/efr";
const SCRATCH: &str = "/home/u/.local/share/efr/scratch/2026-10-23-perf-0a1b2c3d";

/// A size of a ladder that takes longer than this stops the ladder.
const LIMIT: Duration = Duration::from_secs(10);

/// The largest slope of a cost over the size of the line: a linear cost has about 1,
/// and the copies and scans that each word repeated had 2 (v0.0.4).
const MAX_SLOPE: f64 = 1.3;

/// Fails when a slope of `slopes` is above [`MAX_SLOPE`] or a ladder stopped early.
fn check_budget(slopes: &[(String, f64)], stopped: &[String]) {
    assert!(stopped.is_empty(), "stopped early: {stopped:?}");
    for (label, slope) in slopes {
        assert!(*slope < MAX_SLOPE, "{label}: slope {slope:.2}, the budget is {MAX_SLOPE}");
    }
}

/// The programs of the user's ask rules: 20 of them, as a long config has.
const ASKED: [&str; 20] = [
    "curl",
    "wget",
    "docker",
    "kubectl",
    "ssh",
    "scp",
    "rsync",
    "npm",
    "pip",
    "make",
    "python",
    "node",
    "terraform",
    "aws",
    "gcloud",
    "helm",
    "psql",
    "mysql",
    "rg",
    "rm",
];

fn project() -> ProjectId {
    "0192f0c1-7a00-7000-8000-000000000001".parse().unwrap()
}

fn locations() -> Locations {
    let mut locations = Locations::new(HOME).unwrap().with_project(project(), PROJECT).unwrap();
    for root in ["/tmp", "/var/tmp", "/dev/shm"] {
        locations = locations.with_private_root(root).unwrap();
    }
    locations
}

/// The engine of a machine with undo and the home store, and the user's 20 ask rules.
fn engine() -> Engine {
    let rules = ASKED
        .iter()
        .map(|program| {
            Rule::new(
                Action::Execute,
                Resource::Command(CommandPattern::new(*program)),
                Effect::Ask,
            )
        })
        .collect();
    Engine::with_rules(locations(), Policy::new(rules).unwrap()).with_support(AutoSupport {
        floor_backup: true,
        undo: true,
        ..AutoSupport::default()
    })
}

/// The decision input of `line` in an `auto` turn of the project, with the facts the
/// daemon collects for its programs.
fn input(line: &str) -> DecisionInput {
    let request = fact_requests(line, Some(PROJECT.as_ref()), &locations());
    let programs = request
        .programs
        .iter()
        .map(|word| {
            let resolved = (!request.builtins.contains(word))
                .then(|| PathBuf::from("/usr/bin").join(word.rsplit('/').next().unwrap()));
            (word.clone(), resolved, false)
        })
        .collect();
    let facts = CallFacts { programs, ..CallFacts::default() };
    DecisionInput {
        requirements: Requirements::none()
            .with_command(line)
            .with_command_dir(PROJECT)
            .with_facts(facts),
        scope: Scope::Project(project()),
        origin: Origin::Shell,
        mode: Mode::Auto,
        conversation_policy: ConversationPolicy::new(SCRATCH),
    }
}

/// `program` and then the operands `f1` to `fN`.
fn operands(program: &str, n: usize) -> String {
    let mut line = program.to_owned();
    for at in 1..=n {
        line.push_str(&format!(" f{at}"));
    }
    line
}

/// The measured runs of `f`: one run first, then up to four more while they fit in
/// about three seconds. The first run alone when it took longer than [`LIMIT`].
fn samples(mut f: impl FnMut()) -> Vec<Duration> {
    let start = Instant::now();
    f();
    let first = start.elapsed();
    let more = if first > LIMIT {
        0
    } else {
        (Duration::from_secs(3).as_nanos() / first.as_nanos().max(1)).min(4) as usize
    };
    let mut samples = vec![first];
    samples.extend(bench::run(0, more, f));
    samples
}

#[test]
#[ignore = "bench: run by just bench-perf"]
fn bench_perf_run_forms() {
    let engine = engine();
    let mut report = Report::new("bench_perf_run_forms");
    let (mut slopes, mut stopped) = (Vec::new(), Vec::new());
    // NOTE: `rm` first: the peak of `time rg` stays resident after it, and would hide
    // the growth of the next line.
    for (name, program) in [("rm", "rm"), ("time rg x", "time rg x")] {
        let mut points = Vec::new();
        for n in [100_u64, 1_000, 5_000, 10_000] {
            let input = input(&operands(program, n as usize));
            let reset = bench::reset_peak();
            let before = Memory::of_self();
            let mut effect = None;
            let runs = samples(|| effect = Some(engine.decide(&input).effect()));
            // Both programs have an ask rule, so the line must ask.
            assert_eq!(effect, Some(Effect::Ask), "{name} with {n} operands");
            let stats = report.time(&format!("{name} f1..f{n}"), &runs);
            if let (true, Some(before), Some(after)) = (reset, before, Memory::of_self()) {
                report.bytes(
                    &format!("{name} f1..f{n}: peak rss growth"),
                    after.peak.saturating_sub(before.rss),
                );
            }
            points.push((n, stats.p50));
            if stats.p50 > LIMIT {
                report.count(&format!("{name} stopped_at_n"), n);
                stopped.push(name.to_owned());
                break;
            }
        }
        let label = format!("{name} slope over operands");
        slopes.push((label.clone(), report.slope(&label, &points)));
    }
    report.finish();
    check_budget(&slopes, &stopped);
}

#[test]
#[ignore = "bench: run by just bench-perf"]
fn bench_perf_git_forms() {
    // NOTE: a git rule with `under` reads the views of every git form, and each view
    // reads the rest of the line.
    let git = CommandPattern::new("git").with_args(["push"]).with_under("~/work");
    let rules = vec![Rule::new(Action::Execute, Resource::Command(git), Effect::Ask)];
    let engine = Engine::with_rules(locations(), Policy::new(rules).unwrap());
    let mut report = Report::new("bench_perf_git_forms");
    let (mut points, mut stopped) = (Vec::new(), Vec::new());
    for n in [100_u64, 1_000, 5_000, 10_000] {
        let mut line = String::from("env");
        for _ in 0..n {
            line.push_str(" git");
        }
        let input = input(&line);
        let mut effect = None;
        let runs = samples(|| effect = Some(engine.decide(&input).effect()));
        // NOTE: past the forms that a rule reads, the rule matches: the line asks.
        assert_eq!(effect, Some(Effect::Ask), "env and git {n} times");
        let stats = report.time(&format!("env git x {n}"), &runs);
        points.push((n, stats.p50));
        if stats.p50 > LIMIT {
            report.count("stopped_at_n", n);
            stopped.push("env git".to_owned());
            break;
        }
    }
    let label = "slope over git words".to_owned();
    let slope = report.slope(&label, &points);
    report.finish();
    check_budget(&[(label, slope)], &stopped);
}

/// The lines of [`bench_perf_rule_readings`] with `n` words: the program's own options
/// before the subcommand (OQL4), `cd`s before the command (OQL9), and a substitution
/// in a line that cannot be split (OQL9).
fn reading_lines(n: usize) -> [(&'static str, String); 3] {
    let mut options = String::from("systemctl");
    let mut cds = String::new();
    let mut opaque = String::from("git push $(echo");
    for at in 1..=n {
        options.push_str(" --zz");
        cds.push_str(&format!("cd /d{at}; "));
        opaque.push_str(&format!(" w{at}"));
    }
    options.push_str(" restart nginx");
    cds.push_str("git push");
    opaque.push(')');
    [("options", options), ("cds", cds), ("substitution", opaque)]
}

#[test]
#[ignore = "bench: run by just bench-perf"]
fn bench_perf_rule_readings() {
    // NOTE: v0.0.5's readings of an ask rule (OQL4, OQL9) in `cautious`, after a rule
    // that allows every command line. v0.0.4 allowed all three lines: it compared the
    // words right after `systemctl`, matched nothing with `under` after a `cd`, and
    // read no rule but `any` in a line that cannot be split. The line of `cd`s runs in
    // `auto` and in full mode too, with the exits: each segment of the lenient split
    // copied the targets of the `cd`s before it, so 10k `cd`s took 10.7 s in `auto` (a
    // slope of 2.2), and the guard of full mode read each list again for each segment,
    // so 2k `cd`s took 41 s there (a slope of 3.0).
    let rules = vec![
        Rule::new(Action::Execute, Resource::Any, Effect::Allow),
        Rule::new(
            Action::Execute,
            Resource::Command(CommandPattern::new("systemctl").with_args(["restart"])),
            Effect::Ask,
        ),
        Rule::new(
            Action::Execute,
            Resource::Command(CommandPattern::new("git").with_args(["push"]).with_under("~/work")),
            Effect::Ask,
        ),
    ];
    let engine = Engine::with_rules(locations(), Policy::new(rules).unwrap());
    let mut report = Report::new("bench_perf_rule_readings");
    let (mut slopes, mut stopped) = (Vec::new(), Vec::new());
    let mut cds_10k: Vec<(String, Duration)> = Vec::new();
    let small: &[u64] = &[100, 1_000, 5_000, 10_000];
    let ladders = [
        (0, Mode::Cautious, small),
        (1, Mode::Cautious, small),
        (2, Mode::Cautious, small),
        (1, Mode::Auto, &[1_000, 10_000, 50_000]),
        (1, Mode::Full, &[1_000, 10_000, 50_000]),
    ];
    for (at, mode, sizes) in ladders {
        let mut points = Vec::new();
        let mut name = String::new();
        for &n in sizes {
            let (label, line) = reading_lines(n as usize)[at].clone();
            name = match mode {
                Mode::Auto => format!("{label} in auto"),
                Mode::Full => format!("{label} in full"),
                _ => label.to_owned(),
            };
            let mut input = input(&line);
            input.mode = mode;
            let mut effect = None;
            let runs = samples(|| effect = Some(engine.decide(&input).effect()));
            assert_eq!(effect, Some(Effect::Ask), "{name} with {n} words");
            let stats = report.time(&format!("{name} x {n}"), &runs);
            if mode != Mode::Cautious && n == 10_000 {
                cds_10k.push((name.clone(), stats.p50));
            }
            points.push((n, stats.p50));
            if stats.p50 > LIMIT {
                report.count(&format!("{name} stopped_at_n"), n);
                stopped.push(name.clone());
                break;
            }
        }
        let label = format!("{name} slope over words");
        slopes.push((label.clone(), report.slope(&label, &points)));
    }
    report.finish();
    check_budget(&slopes, &stopped);
    // NOTE: about 10 ms in `auto` and 15 ms in full mode on a busy machine.
    for (name, p50) in cds_10k {
        assert!(p50 < Duration::from_millis(100), "{name} x 10000: {p50:?}, the budget is 100 ms");
    }
}

#[test]
#[ignore = "bench: run by just bench-perf"]
fn bench_perf_fact_requests() {
    let locations = locations();
    let mut report = Report::new("bench_perf_fact_requests");
    let (mut slopes, mut stopped) = (Vec::new(), Vec::new());
    for program in ["rm -rf", "touch"] {
        let mut points = Vec::new();
        for n in [100_u64, 1_000, 5_000] {
            let line = operands(program, n as usize);
            let mut targets = 0;
            let runs = samples(|| {
                targets = fact_requests(&line, Some(PROJECT.as_ref()), &locations).targets.len();
            });
            assert!(targets >= n as usize, "{targets} targets for {n} operands");
            let stats = report.time(&format!("{program} f1..f{n}"), &runs);
            report.count(&format!("{program} f1..f{n}: targets"), targets as u64);
            points.push((n, stats.p50));
            if stats.p50 > LIMIT {
                report.count(&format!("{program} stopped_at_n"), n);
                stopped.push(program.to_owned());
                break;
            }
        }
        let label = format!("{program} slope over operands");
        slopes.push((label.clone(), report.slope(&label, &points)));
    }
    report.finish();
    check_budget(&slopes, &stopped);
}

/// The decision input of `rm -f` of `n` files that do not exist in `dir`, which the
/// line names as `{prefix}1` to `{prefix}N`, as efrd gives it: the line, each file as a
/// declared write, and the facts that efrd collects for them (each file is missing,
/// `dir` is a dir).
fn rm_input(n: usize, dir: &str, prefix: &str) -> DecisionInput {
    let mut line = String::from("rm -f");
    for at in 1..=n {
        line.push_str(&format!(" {prefix}{at}"));
    }
    let mut input = input(&line);
    let files: Vec<PathBuf> = (1..=n).map(|at| PathBuf::from(dir).join(format!("f{at}"))).collect();
    let mut requirements = input.requirements.clone();
    let mut facts = requirements.facts.take().unwrap_or_default();
    facts.targets = files.iter().map(|file| (file.clone(), None)).collect();
    facts.targets.push((PathBuf::from(dir), Some(TargetKind::Dir)));
    for file in files {
        requirements = requirements.with_write(file);
    }
    input.requirements = requirements.with_destructive().with_facts(facts);
    input
}

#[test]
#[ignore = "bench: run by just bench-perf"]
fn bench_perf_decide_paths() {
    let engine = engine();
    let mut report = Report::new("bench_perf_decide_paths");
    let (mut slopes, mut stopped) = (Vec::new(), Vec::new());
    let mut over: Vec<String> = Vec::new();
    // NOTE: in the project the sandbox writes each file; outside it each file is an
    // exit of its own.
    for (place, dir, prefix) in
        [("in the project", PROJECT, "f"), ("outside the project", "/home/u/notes", "~/notes/f")]
    {
        let mut points = Vec::new();
        for n in [100_u64, 1_000, 6_400] {
            let input = rm_input(n as usize, dir, prefix);
            let reset = bench::reset_peak();
            let before = Memory::of_self();
            let mut effect = None;
            let runs = samples(|| effect = Some(engine.decide(&input).effect()));
            // NOTE: the user's rule for `rm` asks.
            assert_eq!(effect, Some(Effect::Ask), "rm -f of {n} files {place}");
            let stats = report.time(&format!("rm -f of {n} files {place}"), &runs);
            if let (true, Some(before), Some(after)) = (reset, before, Memory::of_self()) {
                let growth = after.peak.saturating_sub(before.rss);
                report.bytes(&format!("rm -f of {n} files {place}: peak rss growth"), growth);
                if growth > DECIDE_PATHS_GROWTH {
                    over.push(format!("{n} files {place}: peak rss growth {growth} bytes"));
                }
            }
            points.push((n, stats.p50));
            if stats.p50 > LIMIT {
                report.count(&format!("{place} stopped_at_n"), n);
                stopped.push(place.to_owned());
                break;
            }
        }
        let label = format!("{place} slope over files");
        // NOTE: outside the project each file is an exit, which looks up the facts of
        // its path. v0.0.4 scanned every fact for each one: 6,400 files took 2.8 s, a
        // slope of 1.8. Then each exit copied the text of the whole `rm`: 6,400 files
        // took about 100 ms, a slope of 0.93 to 1.22.
        let slope = report.slope(&label, &points);
        if slope >= DECIDE_PATHS_SLOPE {
            over.push(format!("{label}: slope {slope:.2}"));
        }
        slopes.push((label, slope));
    }
    report.finish();
    check_budget(&slopes, &stopped);
    assert!(
        over.is_empty(),
        "over the budget of slope {DECIDE_PATHS_SLOPE} or {DECIDE_PATHS_GROWTH} bytes: {over:?}"
    );
}

/// The largest slope of the decision over the files of an `rm -f`. Each exit of the
/// line now shares the text of its part, and the slope is about 1.0.
const DECIDE_PATHS_SLOPE: f64 = 1.15;

/// The largest growth of the peak resident set while the engine decides an `rm -f` of
/// files. The exits of 6,400 files outside the project held a copy of the whole line
/// each, about 530 MB; they now share one.
const DECIDE_PATHS_GROWTH: u64 = 64 * 1024 * 1024;

/// `runs` runs of `f` after 20 to warm up, which give the base cost of a short call; or,
/// for `runs` 0, the [`samples`] of a long call.
fn measure(runs: usize, f: &mut dyn FnMut()) -> Vec<Duration> {
    if runs > 0 { bench::run(20, runs, f) } else { samples(f) }
}

#[test]
#[ignore = "bench: run by just bench-perf"]
fn bench_perf_decide_line() {
    let engine = engine();
    let mut report = Report::new("bench_perf_decide_line");
    // About 200 bytes, as a model writes a build and test line.
    let typical = "cd crates/efr-cli && cargo build --release 2>&1 | tail -n 20 && \
                   cargo test -p efr-cli -- --nocapture > /tmp/test.log; grep -n 'FAILED' \
                   /tmp/test.log | head -n 5";
    let mut long = String::from("printf '%s\\n'");
    while long.len() < 100 * 1024 {
        long.push_str(" word");
    }
    let locations = locations();
    for (name, line, runs) in [("typical line", typical, 2_000), ("100 KB line", long.as_str(), 0)]
    {
        let input = input(line);
        report.count(&format!("{name}: bytes"), line.len() as u64);
        let decide = measure(runs, &mut || {
            std::hint::black_box(engine.decide(&input));
        });
        report.time(&format!("{name}: decide"), &decide);
        let facts = measure(runs, &mut || {
            std::hint::black_box(fact_requests(line, Some(PROJECT.as_ref()), &locations));
        });
        report.time(&format!("{name}: fact_requests"), &facts);
    }
    report.finish();
}

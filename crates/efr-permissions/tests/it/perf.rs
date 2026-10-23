//! The benches of the engine (`just bench-perf`, section 2.3 of the perf plan): the
//! decision of long lines under the user's ask rules, the fact requests of long lines,
//! and the base cost of one decision. They report; they have no time budget yet.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use efr_permissions::exits::fact_requests;
use efr_permissions::{
    Action, AutoSupport, CallFacts, CommandPattern, ConversationPolicy, DecisionInput, Effect,
    Engine, Locations, Policy, Requirements, Resource, Rule,
};
use efr_protocol::{Mode, Origin, ProjectId, Scope};
use efr_test_support::bench::{self, Memory, Report};

const HOME: &str = "/home/u";
const PROJECT: &str = "/home/u/p/efr";
const SCRATCH: &str = "/home/u/.local/share/efr/scratch/2026-10-23-perf-0a1b2c3d";

/// A size of a ladder that takes longer than this stops the ladder.
const LIMIT: Duration = Duration::from_secs(10);

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
                break;
            }
        }
        report.slope(&format!("{name} slope over operands"), &points);
    }
    report.finish();
}

#[test]
#[ignore = "bench: run by just bench-perf"]
fn bench_perf_fact_requests() {
    let locations = locations();
    let mut report = Report::new("bench_perf_fact_requests");
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
                break;
            }
        }
        report.slope(&format!("{program} slope over operands"), &points);
    }
    report.finish();
}

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

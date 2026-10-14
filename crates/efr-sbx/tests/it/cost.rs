//! The launch cost and the setup failures of the spec's section 16.2 gate: back-to-back
//! calls with the full set of cache overlays, in the `overlay` and the `tmp` cache
//! mode. The test prints the cost per call of the whole launcher and of the child shell
//! alone, and how many calls failed to start (the launcher tries a busy overlay again).
//! With `EFR_TEST_SBX_GATE=1` it runs 1000 calls per mode, reports the gate of each mode
//! (p95 at most 10 ms, no setup failure; the default becomes `tmp` when `overlay` fails
//! it), and fails when the `tmp` mode cannot start every call. A second test reports
//! the cost of a job's start. `launch cost proxy` reports the cost of a call with a
//! proxy plan, whose bridge takes two more spawns and a ready byte per call.

use std::fs;
use std::process::Stdio;
use std::time::{Duration, Instant};

use efr_protocol::CacheMode;
use efr_sandbox::JobPlan;

use crate::support::{
    Fixture, ProxyAnswer, command, env_var, run_with_timeout, sandbox_or_skip, say, wait_until,
};

/// The caches of the spec's section 5.2, made in the fake home.
const CACHES: &[&str] = &[
    ".cargo",
    ".rustup",
    ".cache",
    "go/pkg/mod",
    ".npm",
    ".bun/install/cache",
    ".local/share/pnpm/store",
    ".m2/repository",
    ".gradle/caches",
];

/// The gate's limit on the p95 launch cost.
const GATE: Duration = Duration::from_millis(10);

fn percentile(sorted: &[Duration], share: f64) -> Duration {
    let last = sorted.len().saturating_sub(1);
    let at = (last as f64 * share).round() as usize;
    sorted.get(at.min(last)).copied().unwrap_or_default()
}

/// Prints the cost table of one series and returns its p50 and p95.
fn report(name: &str, mut costs: Vec<Duration>) -> (Duration, Duration) {
    costs.sort_unstable();
    let (p50, p95) = (percentile(&costs, 0.5), percentile(&costs, 0.95));
    say(&format!(
        "{name}: {} calls, p50 {:.2} ms, p95 {:.2} ms, max {:.2} ms",
        costs.len(),
        p50.as_secs_f64() * 1000.0,
        p95.as_secs_f64() * 1000.0,
        costs.last().copied().unwrap_or_default().as_secs_f64() * 1000.0
    ));
    (p50, p95)
}

/// The wall time of each of `calls` calls of `true` that started, and the setup
/// failures.
fn launches(fixture: &Fixture, calls: usize) -> (Vec<Duration>, Vec<String>) {
    let mut costs = Vec::with_capacity(calls);
    let mut failures = Vec::new();
    for _ in 0..calls {
        let call_dir = fixture.prepare("true");
        let command = fixture.command(&call_dir, &fixture.project);
        let start = Instant::now();
        let ended = run_with_timeout(command);
        let spent = start.elapsed();
        let run = fixture.collect(&call_dir, ended);
        match &run.result().setup_error {
            Some(error) => failures.push(error.clone()),
            None => {
                run.expect_status(0);
                costs.push(spent);
            }
        }
    }
    (costs, failures)
}

/// The cost of a job's start (efr's auto spec, section 16.5): from the spawn of the
/// launcher to the first byte of the job's output, for jobs that print one line. A
/// report, not a gate: its time depends on the load of the machine. With
/// `EFR_TEST_SBX_GATE=1` it starts 1000 jobs, else 100.
#[test]
fn job_start_cost() {
    let ready = sandbox_or_skip!();
    let gate = env_var("EFR_TEST_SBX_GATE").is_some_and(|value| value == "1");
    let starts = if gate { 1000 } else { 100 };
    let fixture = Fixture::new(&ready);
    let mut costs = Vec::with_capacity(starts);
    for _ in 0..starts {
        let call_dir = fixture.prepare_job("print started", JobPlan::new(600, 19));
        let spawned = Instant::now();
        let mut run = fixture.spawn_job(&call_dir);
        run.start();
        wait_until("the job's first output", || run.first_output().is_some());
        let first = run.first_output().unwrap_or(spawned);
        costs.push(first.saturating_duration_since(spawned));
        run.wait(&fixture).expect_status(0);
    }
    report("job start cost (spawn to first output byte)", costs);
}

#[test]
fn launch_cost_with_every_cache_overlay() {
    let ready = sandbox_or_skip!();
    let gate = env_var("EFR_TEST_SBX_GATE").is_some_and(|value| value == "1");
    let calls = if gate { 1000 } else { 100 };
    let (bare, bare_failures) = launches(&Fixture::new(&ready), 100);
    assert!(bare_failures.is_empty(), "{bare_failures:?}");
    let mut fixture = Fixture::new(&ready);
    for cache in CACHES {
        let path = fixture.home.join(cache);
        fs::create_dir_all(&path).unwrap();
        fixture.cache(&path);
    }
    let (overlay, overlay_failures) = launches(&fixture, calls);
    fixture.spec.cache_mode = CacheMode::Tmp;
    let (tmp, tmp_failures) = launches(&fixture, calls);
    let children = child_shell(&fixture);
    report("efr-sbx run without cache overlays", bare);
    let (_, overlay_p95) = report("efr-sbx run with 9 cache overlays", overlay);
    let (_, tmp_p95) = report("efr-sbx run with 9 tmp overlays (cache_mode = tmp)", tmp);
    let (child_p50, _) = report("the child shell alone", children);
    let overlay_cost = overlay_p95.saturating_sub(child_p50);
    let tmp_cost = tmp_p95.saturating_sub(child_p50);
    say(&format!(
        "launch cost at p95 without the child shell: overlay {:.2} ms, tmp {:.2} ms",
        overlay_cost.as_secs_f64() * 1000.0,
        tmp_cost.as_secs_f64() * 1000.0
    ));
    say(&format!(
        "setup failures in {calls} back-to-back calls: overlay {}, tmp {}{}",
        overlay_failures.len(),
        tmp_failures.len(),
        overlay_failures.first().map(|error| format!(" (first: {error})")).unwrap_or_default()
    ));
    if gate {
        // The gate decides the default cache mode and goes into the phase notes. Its time
        // part depends on the load of the machine, so the test reports it; it fails only
        // when the mode the gate falls back to cannot start every call.
        let passes = |cost: Duration, failures: &[String]| cost <= GATE && failures.is_empty();
        let verdict = |passed: bool| if passed { "passes" } else { "fails" };
        say(&format!(
            "gate: overlay {}, tmp {}",
            verdict(passes(overlay_cost, &overlay_failures)),
            verdict(passes(tmp_cost, &tmp_failures))
        ));
        assert!(tmp_failures.is_empty(), "the tmp cache mode failed to start: {tmp_failures:?}");
    }
}

/// The wall time of 100 runs of the child shell alone, outside any sandbox; its records
/// go nowhere.
fn child_shell(fixture: &Fixture) -> Vec<Duration> {
    let child_dir = fixture.base.join("child");
    fs::create_dir_all(&child_dir).unwrap();
    fs::write(child_dir.join("line"), "true").unwrap();
    let script = fixture.spec.runtime.child_script.clone();
    let mut children = Vec::with_capacity(100);
    for _ in 0..100 {
        let mut child = command("zsh");
        child.arg("-f").arg(&script).arg(&child_dir).stdout(Stdio::null()).stderr(Stdio::null());
        let start = Instant::now();
        let (status, _, _) = run_with_timeout(child);
        children.push(start.elapsed());
        assert_eq!(status, 0);
    }
    children
}

/// The cost of a call with a proxy plan (DP34): the bridge's two spawns, its own
/// Landlock domain and seccomp profile and its ready byte, before the child runs. A
/// report of four series: with and without the plan, with no cache overlay and with the
/// 9 caches in the `tmp` mode, the default. The calls of the four series take turns, so
/// the load of the machine weighs on each the same. 100 calls each, 1000 with
/// `EFR_TEST_SBX_GATE=1`. The gate line compares the p95 of the call with the caches and
/// the plan, without the child shell, with the launch gate; a fail there is a stop for
/// the integrator, not a failed test.
#[test]
fn launch_cost_proxy() {
    let ready = sandbox_or_skip!();
    let gate = env_var("EFR_TEST_SBX_GATE").is_some_and(|value| value == "1");
    let calls = if gate { 1000 } else { 100 };
    let with_caches = |fixture: &mut Fixture| {
        for cache in CACHES {
            let path = fixture.home.join(cache);
            fs::create_dir_all(&path).unwrap();
            fixture.cache(&path);
        }
        fixture.spec.cache_mode = CacheMode::Tmp;
    };
    let mut fixtures = Vec::new();
    for (caches, proxy) in [(false, false), (false, true), (true, false), (true, true)] {
        let mut fixture = Fixture::new(&ready);
        if caches {
            with_caches(&mut fixture);
        }
        if proxy {
            fixture.proxy(ProxyAnswer::Tunnel);
        }
        fixtures.push(fixture);
    }
    let mut series: Vec<Vec<Duration>> = vec![Vec::with_capacity(calls); fixtures.len()];
    for _ in 0..calls {
        for (fixture, costs) in fixtures.iter().zip(&mut series) {
            let (cost, failures) = launches(fixture, 1);
            assert!(failures.is_empty(), "{failures:?}");
            costs.extend(cost);
        }
    }
    let children = child_shell(&Fixture::new(&ready));
    let mut p95s = Vec::new();
    for (name, costs) in [
        "launch cost without proxy",
        "launch cost proxy",
        "launch cost with 9 tmp overlays without proxy",
        "launch cost proxy with 9 tmp overlays",
    ]
    .into_iter()
    .zip(series)
    {
        p95s.push(report(name, costs).1);
    }
    let (child_p50, _) = report("the child shell alone", children);
    let p95 = |at: usize| p95s.get(at).copied().unwrap_or_default();
    let ms = |cost: Duration| cost.as_secs_f64() * 1000.0;
    let cost = p95(3).saturating_sub(child_p50);
    say(&format!(
        "launch cost proxy at p95 without the child shell: {:.2} ms with 9 tmp overlays, \
         {:.2} ms without; the bridge adds {:.2} ms and {:.2} ms",
        ms(cost),
        ms(p95(1).saturating_sub(child_p50)),
        ms(p95(3).saturating_sub(p95(2))),
        ms(p95(1).saturating_sub(p95(0)))
    ));
    say(&format!("gate: proxy {}", if cost <= GATE { "passes" } else { "fails" }));
}

/// The cost of a call with the network view: no network namespace of its own, the
/// network view's seccomp profile and Landlock's TCP rights. A report of 100 calls,
/// not a gate.
#[test]
fn launch_cost_netview() {
    let ready = sandbox_or_skip!();
    let mut fixture = Fixture::new(&ready);
    fixture.net_view();
    let (costs, failures) = launches(&fixture, 100);
    assert!(failures.is_empty(), "{failures:?}");
    report("launch cost netview", costs);
}

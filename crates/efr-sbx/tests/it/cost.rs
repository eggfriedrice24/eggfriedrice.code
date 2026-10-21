//! The launch cost and the setup failures of the spec's section 16.2 gate: back-to-back
//! calls with the full set of cache overlays, in the `overlay` and the `tmp` cache
//! mode. The test prints the cost per call of the whole launcher and of the child shell
//! alone, and how many calls failed to start (the launcher tries a busy overlay again).
//! With `EFR_TEST_SBX_GATE=1` it runs 1000 calls per mode, reports the gate of each mode
//! (p95 at most 10 ms, no setup failure; the default becomes `tmp` when `overlay` fails
//! it), and fails when the `tmp` mode cannot start every call. A second test reports
//! the cost of a job's start, and a third the cost that the forwarder of port forwards
//! adds to it. `launch cost proxy` reports the cost of a call with a
//! proxy plan, whose bridge takes two more spawns and a ready byte per call.
//! `launch cost downloads` reports the cost of the `tmp` mode when the download caches
//! keep their layer for the conversation (`sandbox.keep_downloads`, DP27 of the phase
//! D2 plan). `bus proxy wrapper start` reports the time of `efr-sbx dbus-proxy` to the
//! ready byte of the program behind it (phase E).

use std::collections::BTreeMap;
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

/// The narrow set of kept download caches of the spec's section 8.7, at the unit of
/// the cache table: the cargo and npm caches.
const DOWNLOADS_NARROW: &[&str] = &[".cargo", ".npm"];

/// The wide set of kept download caches: `efr_config::DOWNLOAD_CACHES` at most.
const DOWNLOADS_WIDE: &[&str] =
    &[".cargo", "go/pkg/mod", ".npm", ".bun/install/cache", ".local/share/pnpm/store"];

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

/// The cost of the forwarder at a job's start (DL18 of the live items plan): its two
/// spawns, its setup and its ready byte, which the inner stage waits for before the
/// job's child runs. Two series of jobs that print one line, without and with a forward
/// plan (a test listener stands for efrd), whose jobs take turns, so the load of the
/// machine weighs on each the same. A report, not a gate: the plan's check is at most
/// 2 ms more at the median. With `EFR_TEST_SBX_GATE=1` 1000 jobs each, else 100.
#[test]
fn job_start_cost_forward() {
    let ready = sandbox_or_skip!();
    let gate = env_var("EFR_TEST_SBX_GATE").is_some_and(|value| value == "1");
    let starts = if gate { 1000 } else { 100 };
    let fixture = Fixture::new(&ready);
    let mut series: [Vec<Duration>; 2] = [Vec::with_capacity(starts), Vec::with_capacity(starts)];
    for _ in 0..starts {
        for (forward, costs) in [false, true].into_iter().zip(&mut series) {
            let (call_dir, listener) = if forward {
                crate::forward::prepare(&fixture, "print started", true)
            } else {
                (fixture.prepare_job("print started", JobPlan::new(600, 19)), None)
            };
            let spawned = Instant::now();
            let mut run = fixture.spawn_job(&call_dir);
            run.start();
            wait_until("the job's first output", || run.first_output().is_some());
            let first = run.first_output().unwrap_or(spawned);
            costs.push(first.saturating_duration_since(spawned));
            run.wait(&fixture).expect_status(0);
            drop(listener);
        }
    }
    let [plain, forward] = series;
    let (plain_p50, _) = report("job start cost without forwarder", plain);
    let (forward_p50, _) = report("job start cost with forwarder", forward);
    let added = forward_p50.as_secs_f64() - plain_p50.as_secs_f64();
    say(&format!("the forwarder adds {:.2} ms to a job's start at p50", added * 1000.0));
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

/// The cost of the `tmp` mode with kept download caches (DP27): the 9 caches of the
/// spec, of which none, the two of [`DOWNLOADS_NARROW`] or the five of
/// [`DOWNLOADS_WIDE`] keep their layer for the conversation, as `overlay` mode layers
/// with the layer lock; the rest stay `tmp`. A report of three series, `launch cost
/// tmp`, `launch cost downloads narrow` and `launch cost downloads`, whose calls take
/// turns. 100 calls each, 1000 with `EFR_TEST_SBX_GATE=1`. Each gate line compares the
/// p95 without the child shell with the launch gate and counts the setup failures; the
/// default of `sandbox.keep_downloads` follows from them, so a fail is a report, not a
/// failed test.
#[test]
fn launch_cost_downloads() {
    let ready = sandbox_or_skip!();
    let gate = env_var("EFR_TEST_SBX_GATE").is_some_and(|value| value == "1");
    let calls = if gate { 1000 } else { 100 };
    let mut fixtures = Vec::new();
    for kept in [&[][..], DOWNLOADS_NARROW, DOWNLOADS_WIDE] {
        let mut fixture = Fixture::new(&ready);
        for cache in CACHES {
            let path = fixture.home.join(cache);
            fs::create_dir_all(&path).unwrap();
            fixture.cache(&path);
        }
        fixture.spec.cache_mode = CacheMode::Tmp;
        for cache in &mut fixture.spec.caches {
            cache.keep = kept.iter().any(|name| cache.target == fixture.home.join(name));
        }
        let count = fixture.spec.caches.iter().filter(|cache| cache.keep).count();
        assert_eq!(count, kept.len());
        fixtures.push(fixture);
    }
    let mut series: Vec<Vec<Duration>> = vec![Vec::with_capacity(calls); fixtures.len()];
    let mut failures: Vec<Vec<String>> = vec![Vec::new(); fixtures.len()];
    let mut phases: Vec<BTreeMap<String, Vec<u64>>> = vec![BTreeMap::new(); fixtures.len()];
    for _ in 0..calls {
        let each = fixtures.iter().zip(&mut series).zip(&mut failures).zip(&mut phases);
        for (((fixture, costs), failed), steps) in each {
            let call_dir = fixture.prepare("true");
            let command = fixture.command(&call_dir, &fixture.project);
            let start = Instant::now();
            let ended = run_with_timeout(command);
            let spent = start.elapsed();
            let run = fixture.collect(&call_dir, ended);
            if let Some(error) = &run.result().setup_error {
                failed.push(error.clone());
                continue;
            }
            run.expect_status(0);
            costs.push(spent);
            for timing in &run.result().timings {
                steps.entry(timing.phase.clone()).or_default().push(timing.us);
            }
        }
    }
    let children = child_shell(&Fixture::new(&ready));
    let names = [
        "launch cost tmp (9 tmp overlays)",
        "launch cost downloads narrow (2 kept, 7 tmp)",
        "launch cost downloads (5 kept, 4 tmp)",
    ];
    let mut p95s = Vec::new();
    for ((name, costs), steps) in names.into_iter().zip(series).zip(&mut phases) {
        p95s.push(report(name, costs).1);
        // Where the time goes: the launcher's steps at their median.
        let medians: Vec<String> = steps
            .iter_mut()
            .map(|(phase, us)| {
                us.sort_unstable();
                let at = us.len() / 2;
                format!("{phase} {:.2}", us.get(at).copied().unwrap_or_default() as f64 / 1000.0)
            })
            .collect();
        say(&format!("  steps at p50 in ms: {}", medians.join(", ")));
    }
    let (child_p50, _) = report("the child shell alone", children);
    let ms = |cost: Duration| cost.as_secs_f64() * 1000.0;
    for ((name, p95), failed) in
        ["tmp", "downloads narrow", "downloads"].iter().zip(&p95s).zip(&failures)
    {
        let cost = p95.saturating_sub(child_p50);
        let passes = cost <= GATE && failed.is_empty();
        say(&format!(
            "launch cost {name} at p95 without the child shell: {:.2} ms, {} setup failures in \
             {calls} calls{}; gate: {}",
            ms(cost),
            failed.len(),
            failed.first().map(|error| format!(" (first: {error})")).unwrap_or_default(),
            if passes { "passes" } else { "fails" }
        ));
    }
    let tmp = p95s.first().copied().unwrap_or_default();
    say(&format!(
        "the kept layers add at p95: {:.2} ms (narrow), {:.2} ms (five)",
        ms(p95s.get(1).copied().unwrap_or_default().saturating_sub(tmp)),
        ms(p95s.get(2).copied().unwrap_or_default().saturating_sub(tmp))
    ));
    let tmp_failures = failures.first().cloned().unwrap_or_default();
    assert!(tmp_failures.is_empty(), "the tmp cache mode failed to start: {tmp_failures:?}");
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

/// A report, no gate: the time from the spawn of `efr-sbx dbus-proxy` to the ready
/// byte of the program behind it, with the launcher's bus proxy checks standing for
/// `xdg-dbus-proxy` (which adds about 1.5 ms of its own, the phase E probe).
#[test]
fn bus_proxy_wrapper_start() {
    use std::io::Read;
    let ready = sandbox_or_skip!();
    let dir =
        tempfile::Builder::new().prefix("w").tempdir_in(crate::support::target_tmp()).unwrap();
    let mut costs = Vec::with_capacity(100);
    for _ in 0..100 {
        // `--check none` names no check: the program writes its ready byte and exits.
        let checks = ["--check".to_owned(), "none".to_owned()];
        let mut command = crate::dbus::wrapper(&ready.bin, dir.path(), &[], &checks);
        command.stderr(Stdio::null());
        let start = Instant::now();
        let mut child = command.spawn().unwrap();
        let mut byte = [0_u8; 1];
        let read = child.stdout.take().map(|mut pipe| pipe.read(&mut byte));
        costs.push(start.elapsed());
        assert!(matches!(read, Some(Ok(1))) && byte == *b"x", "{read:?}");
        assert!(child.wait().unwrap().success());
    }
    report("bus proxy wrapper start", costs);
}

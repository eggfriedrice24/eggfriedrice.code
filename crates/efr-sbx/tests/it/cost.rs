//! The launch cost and the setup failures of the spec's section 16.2 gate: back-to-back
//! calls with the full set of cache overlays, in the `overlay` and the `tmp` cache
//! mode. The test prints the cost per call of the whole launcher and of the child shell
//! alone, and how many calls failed to start (the launcher tries a busy overlay again).
//! With `EFR_TEST_SBX_GATE=1` it runs 1000 calls per mode and holds the gate for the
//! mode that the gate's rule picks as the default: `overlay` when it meets both parts,
//! else `tmp`, which then must meet them.

use std::fs;
use std::process::Stdio;
use std::time::{Duration, Instant};

use efr_protocol::CacheMode;

use crate::support::{Fixture, command, env_var, run_with_timeout, sandbox_or_skip, say};

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
    // The child shell alone, outside any sandbox; its records go nowhere.
    let child_dir = fixture.base.join("child");
    fs::create_dir(&child_dir).unwrap();
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
        let overlay_passes = overlay_cost <= GATE && overlay_failures.is_empty();
        let mode = if overlay_passes { "overlay" } else { "tmp" };
        say(&format!("gate: the default cache mode that passes here is {mode}"));
        if !overlay_passes {
            assert!(tmp_cost <= GATE, "the tmp cache mode passes 10 ms too");
            assert!(tmp_failures.is_empty(), "{tmp_failures:?}");
        }
    }
}

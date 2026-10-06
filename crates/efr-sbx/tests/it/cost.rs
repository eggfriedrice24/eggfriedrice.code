//! The launch cost of the spec's section 16.2 gate: back-to-back calls with the full set
//! of cache overlays, none of which may fail to start (the launcher tries a busy overlay
//! again). The test prints the cost per call of the whole launcher and of the child
//! shell alone; `EFR_TEST_SBX_GATE=1` runs 1000 calls and holds the 10 ms p95 gate.

use std::fs;
use std::process::Stdio;
use std::time::{Duration, Instant};

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

fn percentile(sorted: &[Duration], share: f64) -> Duration {
    let at = ((sorted.len() as f64 - 1.0) * share).round() as usize;
    sorted.get(at).copied().unwrap_or_default()
}

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

/// The wall time of `calls` calls of `true`, each checked for a setup failure.
fn launches(fixture: &Fixture, calls: usize) -> Vec<Duration> {
    let mut costs = Vec::with_capacity(calls);
    for _ in 0..calls {
        let call_dir = fixture.prepare("true");
        let command = fixture.command(&call_dir, &fixture.project);
        let start = Instant::now();
        let ended = run_with_timeout(command);
        costs.push(start.elapsed());
        let run = fixture.collect(&call_dir, ended);
        assert_eq!(run.result().setup_error, None, "a setup failure: {run:#?}");
        run.expect_status(0);
    }
    costs
}

#[test]
fn launch_cost_with_every_cache_overlay() {
    let ready = sandbox_or_skip!();
    let gate = env_var("EFR_TEST_SBX_GATE").is_some_and(|value| value == "1");
    let calls = if gate { 1000 } else { 100 };
    let fixture = Fixture::new(&ready);
    let bare = launches(&fixture, calls.min(100));
    let mut fixture = Fixture::new(&ready);
    for cache in CACHES {
        let path = fixture.home.join(cache);
        fs::create_dir_all(&path).unwrap();
        fixture.cache(&path);
    }
    let full = launches(&fixture, calls);
    fixture.spec.cache_mode = efr_protocol::CacheMode::Tmp;
    let tmp = launches(&fixture, calls.min(100));
    // The child shell alone, outside any sandbox; its records go nowhere.
    let child_dir = fixture.base.join("child");
    fs::create_dir(&child_dir).unwrap();
    fs::write(child_dir.join("line"), "true").unwrap();
    let script = fixture.spec.runtime.child_script.clone();
    let mut children = Vec::with_capacity(calls);
    for _ in 0..calls {
        let mut child = command("zsh");
        child.arg("-f").arg(&script).arg(&child_dir).stdout(Stdio::null()).stderr(Stdio::null());
        let start = Instant::now();
        let (status, _, _) = run_with_timeout(child);
        children.push(start.elapsed());
        assert_eq!(status, 0);
    }
    report("efr-sbx run without cache overlays", bare);
    let (_, overlay_p95) = report("efr-sbx run with 9 cache overlays", full);
    let (_, tmp_p95) = report("efr-sbx run with 9 tmp overlays (cache_mode = tmp)", tmp);
    let (child_p50, _) = report("the child shell alone", children);
    let overlay = overlay_p95.saturating_sub(child_p50);
    let tmp = tmp_p95.saturating_sub(child_p50);
    say(&format!(
        "launch cost at p95 without the child shell: overlay {:.2} ms, tmp {:.2} ms",
        overlay.as_secs_f64() * 1000.0,
        tmp.as_secs_f64() * 1000.0
    ));
    if gate {
        // The spec's gate: when the overlay mode passes 10 ms, the default cache mode
        // becomes tmp, which must then meet it.
        let mode = if overlay <= Duration::from_millis(10) { "overlay" } else { "tmp" };
        say(&format!("gate: the cache mode that meets 10 ms here is {mode}"));
        assert!(tmp <= Duration::from_millis(10), "even the tmp cache mode passes 10 ms");
    }
}

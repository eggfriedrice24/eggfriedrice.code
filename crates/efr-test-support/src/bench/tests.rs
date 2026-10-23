use std::time::Duration;

use pretty_assertions::assert_eq;

use super::{Machine, Memory, Report, Stats, slope};

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

#[test]
fn stats_of_no_sample_are_zero() {
    let stats = Stats::of(&[]);
    assert_eq!(stats.n, 0);
    assert_eq!(stats.max, Duration::ZERO);
}

#[test]
fn stats_give_the_percentiles_of_the_sorted_samples() {
    let samples: Vec<Duration> = (1..=100).rev().map(ms).collect();
    let stats = Stats::of(&samples);
    assert_eq!(stats.n, 100);
    assert_eq!(stats.min, ms(1));
    assert_eq!(stats.p50, ms(51));
    assert_eq!(stats.p95, ms(95));
    assert_eq!(stats.max, ms(100));
    assert_eq!(stats.mean, Duration::from_micros(50_500));
}

#[test]
fn slope_of_a_linear_cost_is_one_and_of_a_quadratic_cost_is_two() {
    let linear: Vec<(u64, Duration)> = [10, 100, 1000].iter().map(|n| (*n, ms(*n))).collect();
    let quadratic: Vec<(u64, Duration)> =
        [10, 100, 1000].iter().map(|n| (*n, Duration::from_micros(n * n))).collect();
    assert!((slope(&linear) - 1.0).abs() < 1e-9);
    assert!((slope(&quadratic) - 2.0).abs() < 1e-9);
}

#[test]
fn slope_needs_two_points_that_are_not_zero() {
    assert!(slope(&[(10, ms(1))]).is_nan());
    assert!(slope(&[(10, ms(1)), (100, Duration::ZERO)]).is_nan());
    assert!(slope(&[(10, ms(1)), (10, ms(2))]).is_nan());
}

#[test]
fn memory_of_this_process_reads_its_resident_set() {
    let memory = Memory::of_self().unwrap();
    assert!(memory.rss > 0);
    assert!(memory.peak >= memory.rss);
}

#[test]
fn a_report_gives_one_json_object_per_line_with_the_machine() {
    let machine = Machine {
        load: "0.10 0.20 0.30".into(),
        governor: "performance".into(),
        cpus: 4,
        kernel: "7.1.0".into(),
    };
    let mut report = Report::new("bench_perf_example");
    report.time("send", &[ms(1), ms(3)]);
    report.bytes("rss", 2048);
    report.count("spawns", 4);
    let json = report.json(&machine);
    assert_eq!(json.len(), 3);
    assert_eq!(json[0]["bench"], "bench_perf_example");
    assert_eq!(json[0]["label"], "send");
    assert_eq!(json[0]["measure"]["kind"], "time");
    assert_eq!(json[0]["measure"]["max_ns"], 3_000_000);
    assert_eq!(json[1]["measure"]["bytes"], 2048);
    assert_eq!(json[2]["measure"]["count"], 4);
    assert_eq!(json[2]["machine"]["governor"], "performance");
    let text = report.text(&machine);
    assert_eq!(text.len(), 4);
    assert!(text[0].contains("governor performance"), "{}", text[0]);
}

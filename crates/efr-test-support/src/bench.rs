//! The shared parts of the `bench_perf_*` benches: samples and their percentiles, the
//! report lines, the facts of the machine, the slope of a cost over its input size, and
//! the resident memory of a process.
//!
//! A bench is an ignored test that `just bench-perf` runs in release. It prints its
//! lines to stderr after the load average and the CPU governor, because the numbers of
//! a busy machine are not valid. When `EFR_BENCH_OUT` names a directory, each line also
//! goes to `<dir>/<bench>.jsonl` as one JSON object, so two runs can be compared.
//!
//! Memory comes from `/proc`, not from a counting allocator: the peak resident set of
//! a process (`VmHWM`), which [`reset_peak`] sets back to the current resident set
//! through `/proc/self/clear_refs`. That needs no allocator hook and no dependency.

use std::fmt::Write as _;
use std::fs;
use std::future::Future;
use std::io::Write as _;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// The variable that names the directory of the JSON lines.
pub const BENCH_OUT: &str = "EFR_BENCH_OUT";

/// The percentiles of a set of samples.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stats {
    /// The number of samples.
    pub n: usize,
    /// The shortest sample.
    pub min: Duration,
    /// The median.
    pub p50: Duration,
    /// The 95th percentile.
    pub p95: Duration,
    /// The longest sample.
    pub max: Duration,
    /// The mean.
    pub mean: Duration,
}

impl Stats {
    /// The percentiles of `samples`; all zero when there is no sample.
    pub fn of(samples: &[Duration]) -> Stats {
        if samples.is_empty() {
            return Stats {
                n: 0,
                min: Duration::ZERO,
                p50: Duration::ZERO,
                p95: Duration::ZERO,
                max: Duration::ZERO,
                mean: Duration::ZERO,
            };
        }
        let mut sorted = samples.to_vec();
        sorted.sort();
        let at = |q: f64| sorted[((sorted.len() - 1) as f64 * q).round() as usize];
        let total: Duration = sorted.iter().sum();
        Stats {
            n: sorted.len(),
            min: sorted[0],
            p50: at(0.5),
            p95: at(0.95),
            max: sorted[sorted.len() - 1],
            mean: total / u32::try_from(sorted.len()).unwrap_or(u32::MAX),
        }
    }
}

/// Runs `f` `warmup` times without a measure, then `runs` times, and gives the time of
/// each measured run.
pub fn run(warmup: usize, runs: usize, mut f: impl FnMut()) -> Vec<Duration> {
    for _ in 0..warmup {
        f();
    }
    (0..runs)
        .map(|_| {
            let start = Instant::now();
            f();
            start.elapsed()
        })
        .collect()
}

/// [`run`] for an async body.
pub async fn run_async<F, Fut>(warmup: usize, runs: usize, mut f: F) -> Vec<Duration>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = ()>,
{
    for _ in 0..warmup {
        f().await;
    }
    let mut samples = Vec::with_capacity(runs);
    for _ in 0..runs {
        let start = Instant::now();
        f().await;
        samples.push(start.elapsed());
    }
    samples
}

/// The exponent `k` of a cost that grows as `n^k`: the slope of a least squares line
/// through `(ln n, ln time)`. A linear cost gives about 1, a quadratic one about 2. It
/// gives `f64::NAN` for fewer than two points or a point that is zero.
pub fn slope(points: &[(u64, Duration)]) -> f64 {
    let logs: Vec<(f64, f64)> = points
        .iter()
        .filter(|(n, time)| *n > 0 && !time.is_zero())
        .map(|(n, time)| ((*n as f64).ln(), time.as_secs_f64().ln()))
        .collect();
    if logs.len() < 2 || logs.len() != points.len() {
        return f64::NAN;
    }
    let count = logs.len() as f64;
    let mean_x = logs.iter().map(|(x, _)| x).sum::<f64>() / count;
    let mean_y = logs.iter().map(|(_, y)| y).sum::<f64>() / count;
    let covariance: f64 = logs.iter().map(|(x, y)| (x - mean_x) * (y - mean_y)).sum();
    let variance: f64 = logs.iter().map(|(x, _)| (x - mean_x).powi(2)).sum();
    if variance == 0.0 {
        return f64::NAN;
    }
    covariance / variance
}

/// The resident set of a process from `/proc/<pid>/status`, in bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Memory {
    /// `VmRSS`: the resident set now.
    pub rss: u64,
    /// `VmHWM`: the peak resident set since the start or the last [`reset_peak`].
    pub peak: u64,
}

impl Memory {
    /// The memory of this process.
    pub fn of_self() -> Option<Memory> {
        Memory::read("/proc/self/status")
    }

    /// The memory of the process `pid`.
    pub fn of(pid: u32) -> Option<Memory> {
        Memory::read(&format!("/proc/{pid}/status"))
    }

    fn read(path: &str) -> Option<Memory> {
        let status = fs::read_to_string(path).ok()?;
        let field = |name: &str| {
            status
                .lines()
                .find_map(|line| line.strip_prefix(name))
                .and_then(|rest| rest.trim().strip_suffix("kB"))
                .and_then(|kib| kib.trim().parse::<u64>().ok())
                .map(|kib| kib * 1024)
        };
        Some(Memory { rss: field("VmRSS:")?, peak: field("VmHWM:")? })
    }
}

/// Sets the peak resident set of this process back to its resident set now, so a
/// later [`Memory::of_self`] gives the peak of the measured part only.
pub fn reset_peak() -> bool {
    fs::write("/proc/self/clear_refs", "5").is_ok()
}

/// The facts of the machine that change what a bench measures.
#[derive(Debug, Clone, PartialEq)]
pub struct Machine {
    /// The 1, 5 and 15 minute load averages.
    pub load: String,
    /// The CPU frequency governor of cpu0, such as `powersave`.
    pub governor: String,
    /// The CPUs that the process can use.
    pub cpus: usize,
    /// The kernel release.
    pub kernel: String,
}

impl Machine {
    /// The facts of this machine now; a fact that cannot be read is `?`.
    pub fn now() -> Machine {
        let read = |path: &str| {
            fs::read_to_string(path).map(|text| text.trim().to_owned()).unwrap_or("?".into())
        };
        let load = read("/proc/loadavg");
        Machine {
            load: load.split_whitespace().take(3).collect::<Vec<_>>().join(" "),
            governor: read("/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor"),
            cpus: std::thread::available_parallelism().map_or(0, usize::from),
            kernel: read("/proc/sys/kernel/osrelease"),
        }
    }

    fn json(&self) -> Value {
        json!({
            "load": self.load,
            "governor": self.governor,
            "cpus": self.cpus,
            "kernel": self.kernel,
        })
    }
}

/// One measured line of a bench.
#[derive(Debug, Clone, PartialEq)]
enum Measure {
    Time(Stats),
    Bytes(u64),
    Count(u64),
    Slope(f64),
}

/// The lines of one bench. [`Report::finish`] prints them and writes the JSON lines.
#[derive(Debug)]
pub struct Report {
    bench: String,
    lines: Vec<(String, Measure)>,
}

impl Report {
    /// A report of the bench `bench`, such as `bench_perf_ttft`.
    pub fn new(bench: &str) -> Report {
        Report { bench: bench.to_owned(), lines: Vec::new() }
    }

    /// Adds the percentiles of `samples` under `label`, and gives them.
    pub fn time(&mut self, label: &str, samples: &[Duration]) -> Stats {
        let stats = Stats::of(samples);
        self.lines.push((label.to_owned(), Measure::Time(stats)));
        stats
    }

    /// Adds a size in bytes under `label`.
    pub fn bytes(&mut self, label: &str, bytes: u64) {
        self.lines.push((label.to_owned(), Measure::Bytes(bytes)));
    }

    /// Adds a count, such as the processes that a call started, under `label`.
    pub fn count(&mut self, label: &str, count: u64) {
        self.lines.push((label.to_owned(), Measure::Count(count)));
    }

    /// Adds the [`slope`] of `points` under `label`, and gives it.
    pub fn slope(&mut self, label: &str, points: &[(u64, Duration)]) -> f64 {
        let k = slope(points);
        self.lines.push((label.to_owned(), Measure::Slope(k)));
        k
    }

    /// The lines as text, one per measure, after a line with the machine's facts.
    pub fn text(&self, machine: &Machine) -> Vec<String> {
        let ms = |time: Duration| time.as_secs_f64() * 1000.0;
        let mut out = vec![format!(
            "{}: load average {}, governor {}, {} cpus",
            self.bench, machine.load, machine.governor, machine.cpus
        )];
        for (label, measure) in &self.lines {
            let mut line = format!("{}: {label:<36}", self.bench);
            let _ = match measure {
                Measure::Time(stats) => write!(
                    line,
                    " p50 {:>9.3} ms  p95 {:>9.3} ms  max {:>9.3} ms  ({} samples)",
                    ms(stats.p50),
                    ms(stats.p95),
                    ms(stats.max),
                    stats.n
                ),
                Measure::Bytes(bytes) => {
                    write!(line, " {:>12.3} MiB", *bytes as f64 / (1024.0 * 1024.0))
                }
                Measure::Count(count) => write!(line, " {count:>12}"),
                Measure::Slope(k) => write!(line, " slope {k:>6.2}"),
            };
            out.push(line);
        }
        out
    }

    /// The lines as JSON objects, each with the bench, the label and the machine.
    pub fn json(&self, machine: &Machine) -> Vec<Value> {
        let ns = |time: Duration| u64::try_from(time.as_nanos()).unwrap_or(u64::MAX);
        self.lines
            .iter()
            .map(|(label, measure)| {
                let value = match measure {
                    Measure::Time(stats) => json!({
                        "kind": "time",
                        "n": stats.n,
                        "min_ns": ns(stats.min),
                        "p50_ns": ns(stats.p50),
                        "p95_ns": ns(stats.p95),
                        "max_ns": ns(stats.max),
                        "mean_ns": ns(stats.mean),
                    }),
                    Measure::Bytes(bytes) => json!({ "kind": "bytes", "bytes": bytes }),
                    Measure::Count(count) => json!({ "kind": "count", "count": count }),
                    Measure::Slope(k) => json!({ "kind": "slope", "slope": k }),
                };
                json!({
                    "bench": self.bench,
                    "label": label,
                    "measure": value,
                    "machine": machine.json(),
                })
            })
            .collect()
    }

    /// Prints the lines to stderr and, when [`BENCH_OUT`] names a directory, appends
    /// the JSON lines to `<dir>/<bench>.jsonl`.
    #[expect(clippy::print_stderr, reason = "a bench reports what it measured")]
    pub fn finish(self) {
        let machine = Machine::now();
        for line in self.text(&machine) {
            eprintln!("{line}");
        }
        let Some(dir) = std::env::var_os(BENCH_OUT).map(PathBuf::from) else {
            return;
        };
        let written = fs::create_dir_all(&dir).and_then(|()| {
            let mut file = fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(dir.join(format!("{}.jsonl", self.bench)))?;
            let mut text = String::new();
            for value in self.json(&machine) {
                text.push_str(&value.to_string());
                text.push('\n');
            }
            file.write_all(text.as_bytes())
        });
        if let Err(error) = written {
            eprintln!(
                "{}: could not write the JSON lines to {}: {error}",
                self.bench,
                dir.display()
            );
        }
    }
}

#[cfg(test)]
mod tests;

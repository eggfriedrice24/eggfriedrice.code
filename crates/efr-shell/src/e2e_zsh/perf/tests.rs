use std::io::Write as _;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use efr_test_support::bench::Report;

use super::PerfShell;
use crate::{Completion, NoProgress, RunRequest};

const MIB: u64 = 1024 * 1024;

/// A ladder step that takes longer than this ends the ladder.
const STOP_AFTER: Duration = Duration::from_secs(10);

/// The CPU time of this process so far (user and system, all threads): the reader,
/// the session, the screens and the runtime. zsh and the programs it runs are other
/// processes and do not count.
fn cpu() -> Duration {
    let stat = std::fs::read_to_string("/proc/self/stat").unwrap_or_default();
    // The fields after the command name, which ends at the last `)`.
    let fields: Vec<&str> = stat
        .rsplit_once(')')
        .map(|(_, rest)| rest.split_whitespace().collect())
        .unwrap_or_default();
    // utime and stime are fields 14 and 15 of the line, 12 and 13 after the name.
    let ticks: u64 =
        fields.iter().skip(11).take(2).filter_map(|field| field.parse::<u64>().ok()).sum();
    // NOTE: the kernel counts in USER_HZ, which is 100 on Linux.
    Duration::from_millis(ticks * 10)
}

/// What one measured run cost.
struct Cost {
    wall: Duration,
    cpu: Duration,
    chunks: u64,
    bytes: u64,
    replays: u64,
}

/// Runs `command` in the shell and returns its cost; the run must finish with 0.
async fn measure(shell: &PerfShell, command: &str) -> Cost {
    let (chunks_before, bytes_before) = shell.chunks.now();
    let replays_before = shell.screens.replays.load(Ordering::SeqCst);
    let cpu_before = cpu();
    let start = Instant::now();
    let mut request = RunRequest::new(command, shell.dir());
    request.timeout = Duration::from_secs(3600);
    let result =
        shell.sessions.run_command(shell.conversation, request, &mut NoProgress).await.unwrap();
    let wall = start.elapsed();
    assert_eq!(result.completion, Completion::Finished, "{command}");
    assert_eq!(result.exit_code, Some(0), "{command}");
    let (chunks, bytes) = shell.chunks.now();
    Cost {
        wall,
        cpu: cpu().saturating_sub(cpu_before),
        chunks: chunks - chunks_before,
        bytes: bytes - bytes_before,
        replays: shell.screens.replays.load(Ordering::SeqCst) - replays_before,
    }
}

/// Adds the lines of `costs` under `label`.
fn add(report: &mut Report, label: &str, costs: &[Cost]) -> Duration {
    let walls: Vec<Duration> = costs.iter().map(|cost| cost.wall).collect();
    let cpus: Vec<Duration> = costs.iter().map(|cost| cost.cpu).collect();
    let stats = report.time(&format!("{label} wall"), &walls);
    report.time(&format!("{label} cpu of efr"), &cpus);
    let last = costs.last().unwrap();
    report.count(&format!("{label} chunks"), last.chunks);
    report.bytes(&format!("{label} bytes read"), last.bytes);
    report.count(&format!("{label} bytes per chunk"), last.bytes / last.chunks.max(1));
    report.count(&format!("{label} capture screens"), last.replays);
    stats.p50
}

/// A file of progress output: a two-line display that moves up with `ESC [1A` and
/// redraws with `\r`, as a download or a layer pull does, `size` bytes long.
fn progress_file(shell: &PerfShell, size: u64) -> std::path::PathBuf {
    let path = shell.dir().join("progress.out");
    let mut file = std::io::BufWriter::new(std::fs::File::create(&path).unwrap());
    let mut written = 0;
    let mut n = 0_u64;
    file.write_all(b"\r\n").unwrap();
    while written < size {
        let bar = "#".repeat(usize::try_from(n % 40).unwrap());
        let frame = format!(
            "\x1b[1A\r\x1b[2Kdownloading layer {:>4} [{bar:<40}] {:>3}%\r\n\r\x1b[2K{n} kB of {} kB",
            n / 100,
            n % 100,
            n + 1000
        );
        file.write_all(frame.as_bytes()).unwrap();
        written += frame.len() as u64;
        n += 1;
    }
    file.write_all(b"\r\n").unwrap();
    file.flush().unwrap();
    path
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "bench: run by just bench-perf"]
async fn bench_perf_pty_read() {
    let mut report = Report::new("bench_perf_pty_read");
    let Some(shell) = PerfShell::start() else {
        report.count("skipped: no zsh at /usr/bin/zsh or /bin/zsh", 0);
        report.finish();
        return;
    };
    // The first run starts the shell; it does not count.
    measure(&shell, "true").await;
    let mut points = Vec::new();
    for (mib, runs) in [(50, 3), (500, 1)] {
        let bytes = mib * MIB;
        // Text lines of 64 bytes, as a build log prints them.
        let command = format!(
            "yes 'line of output 0123456789 abcdefghijklmnopqrstuvwxyz ABCDEFG' | head -c {bytes}"
        );
        let mut costs = Vec::new();
        for _ in 0..runs {
            costs.push(measure(&shell, &command).await);
        }
        assert!(costs.iter().all(|cost| cost.bytes >= bytes), "the reader read all the output");
        let p50 = add(&mut report, &format!("text {mib} MiB"), &costs);
        points.push((bytes, p50));
        if p50 > STOP_AFTER {
            report.count("text stopped_at_n", bytes);
            break;
        }
    }
    report.slope("text slope", &points);
    let size = 10 * MIB;
    let file = progress_file(&shell, size);
    let command = format!("cat {}", file.display());
    let mut costs = Vec::new();
    for _ in 0..3 {
        costs.push(measure(&shell, &command).await);
    }
    add(&mut report, "progress 10 MiB", &costs);
    let largest = shell.chunks.largest.load(Ordering::SeqCst);
    report.count("largest chunk", largest);
    shell.sessions.close_all().await;
    report.finish();
    // Budget: the reader hands on many reads as one chunk. One read took at most 16 KiB
    // before, and a fast `cat` gave about 6 KB a chunk; it gives about 55 KB now.
    let batch = crate::reader::BATCH as u64;
    assert!(largest > 16 * 1024 && largest <= batch, "largest chunk {largest}");
    let last = costs.last().unwrap();
    let per_chunk = last.bytes / last.chunks.max(1);
    assert!(per_chunk >= 16 * 1024, "progress: {per_chunk} bytes per chunk");
}

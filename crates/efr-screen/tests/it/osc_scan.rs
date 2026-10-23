//! The bench of the OSC 133 and OSC 7 mark scanner (`ShellMarkScanner`), which the
//! screen actor runs on every PTY chunk before the backend sees it. It reports the time
//! and the throughput on plain output, on output with many escape sequences, and on a
//! stream of marks cut into chunks so that marks cross the chunk edges. A bench, ignored:
//! `just bench-perf` runs it in release.

use std::time::Duration;

use efr_screen::{Seq, ShellMarkScanner};
use efr_test_support::bench::{self, Report};

const MIB: usize = 1024 * 1024;

/// The read size of the hidden shell's PTY reader (`efr-shell/src/reader.rs`).
const READ_CHUNK: usize = 16 * 1024;

/// The sizes of the ladder: 0.5 MiB, 5 MiB and 50 MiB.
const SIZES: [usize; 3] = [MIB / 2, 5 * MIB, 50 * MIB];

/// A ladder step that takes longer than this ends the ladder.
const STOP_AFTER: Duration = Duration::from_secs(10);

/// Lines of text, as `cargo build` or `ls -l` print them.
fn plain(size: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(size + 128);
    let mut n = 0_u64;
    while out.len() < size {
        out.extend_from_slice(
            format!("   Compiling crate-{n} v0.{n}.0 (/home/user/src/crate-{n})\r\n").as_bytes(),
        );
        n += 1;
    }
    out.truncate(size);
    out
}

/// Coloured output with a cursor or erase sequence on every line, a title change every
/// 16 lines and a DCS string every 64 lines: the escapes that the scanner must step over.
fn escapes(size: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(size + 256);
    let mut n = 0_u64;
    while out.len() < size {
        out.extend_from_slice(
            format!(
                "\x1b[1;32m{n:>6}\x1b[0m \x1b[38;5;{}mfile-{n}.rs\x1b[39m \x1b[2K\r\x1b[1A\x1b[1B ok\r\n",
                n % 256
            )
            .as_bytes(),
        );
        if n.is_multiple_of(16) {
            out.extend_from_slice(format!("\x1b]0;build {n}\x07").as_bytes());
        }
        if n.is_multiple_of(64) {
            out.extend_from_slice(b"\x1bP1;2|sixel-ish payload\x1b\\");
        }
        n += 1;
    }
    out.truncate(size);
    out
}

/// One prompt cycle of the zsh integration: the four OSC 133 marks and an OSC 7 report.
fn cycle(n: u64) -> Vec<u8> {
    format!(
        "\x1b]133;A;cl=line\x07$ \x1b]133;B\x07echo {n}\r\n\x1b]133;C\x07{n}\r\n\
         \x1b]133;D;0\x07\x1b]7;file://host/home/user/project/dir-{n}\x07"
    )
    .into_bytes()
}

/// Prompt cycles back to back, and how many marks they hold.
fn marks(size: usize) -> (Vec<u8>, u64) {
    let mut out = Vec::with_capacity(size + 256);
    let mut count = 0;
    let mut n = 0;
    while out.len() < size {
        out.extend_from_slice(&cycle(n));
        count += 5;
        n += 1;
    }
    (out, count)
}

/// Scans `bytes` in chunks of `chunk` bytes as one stream; the count of marks found.
fn scan(bytes: &[u8], chunk: usize) -> u64 {
    let mut scanner = ShellMarkScanner::new();
    let mut found = 0;
    let mut offset = 0_u64;
    for piece in bytes.chunks(chunk) {
        found += scanner.scan(piece, Seq::new(offset)).len() as u64;
        offset += piece.len() as u64;
    }
    found
}

/// The throughput of `samples` over `bytes`, in MiB per second at the median.
fn mib_per_s(bytes: usize, samples: &[Duration]) -> u64 {
    let p50 = bench::Stats::of(samples).p50.as_secs_f64();
    if p50 == 0.0 { 0 } else { (bytes as f64 / MIB as f64 / p50) as u64 }
}

/// Runs the ladder of `name` over the inputs of `make`, scanned in chunks of `chunk`
/// bytes. `expect` gives the marks that an input must hold, when it holds any.
fn ladder(
    report: &mut Report,
    name: &str,
    chunk: usize,
    make: impl Fn(usize) -> (Vec<u8>, Option<u64>),
) {
    let mut points = Vec::new();
    for size in SIZES {
        let (bytes, expect) = make(size);
        let label = format!("{name} {} KiB", size / 1024);
        let mut found = 0;
        let samples = bench::run(1, 5, || found = scan(&bytes, chunk));
        if let Some(expect) = expect {
            assert_eq!(found, expect, "{label}: every mark is found");
        }
        let stats = report.time(&label, &samples);
        report.count(&format!("{label} MiB/s"), mib_per_s(bytes.len(), &samples));
        if expect.is_some() {
            report.count(&format!("{label} marks"), found);
        }
        points.push((size as u64, stats.p50));
        if stats.p50 > STOP_AFTER {
            report.count(&format!("{name} stopped_at_n"), size as u64);
            break;
        }
    }
    report.slope(&format!("{name} slope"), &points);
}

#[test]
#[ignore = "bench: run by just bench-perf"]
fn bench_perf_osc_scan() {
    let mut report = Report::new("bench_perf_osc_scan");
    ladder(&mut report, "plain", READ_CHUNK, |size| (plain(size), None));
    ladder(&mut report, "escapes", READ_CHUNK, |size| (escapes(size), None));
    // 4093 is prime and shorter than a cycle's span, so the chunk edges fall at every
    // place inside the marks.
    ladder(&mut report, "marks split 4093", 4093, |size| {
        let (bytes, count) = marks(size);
        (bytes, Some(count))
    });
    // Chunks of 7 bytes: almost every mark crosses one or more edges.
    ladder(&mut report, "marks split 7", 7, |size| {
        let (bytes, count) = marks(size);
        (bytes, Some(count))
    });
    report.finish();
}

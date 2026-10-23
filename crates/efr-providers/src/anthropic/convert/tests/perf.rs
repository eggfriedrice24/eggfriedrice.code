//! The bench of the Messages request body on the history of `bench_perf_ws_body`.
//! Ignored; `just bench-perf` runs it in release.
//!
//! The history's assistant messages come without their `provider_raw`, as the
//! conversation sends the turns that another provider answered. The test binary has no
//! counting allocator, so the bench reports the time and the growth of the peak
//! resident set over one build of the body.

use std::hint::black_box;

use efr_provider::Message;
use efr_test_support::bench::{self, Memory, Report};
use efr_test_support::{PerfCorpus, PerfSize};

use super::{opus, request, request_body};
use crate::anthropic::AnthropicConfig;

/// The seed of the corpus, the one of `bench_perf_ws_body`.
const SEED: u64 = 42;

/// The turns of the ladder: the histories of `bench_perf_ws_body`, of 210 and 2,015
/// Responses input items and of 3 MB of input.
const TURNS: [usize; 3] = [11, 106, 234];

/// The first `turns` turns of the corpus without provider items, then the next turn's
/// prompt.
fn history(turns: usize) -> Vec<Message> {
    let mut corpus = PerfCorpus::new(PerfSize::L, SEED);
    let mut messages: Vec<Message> = corpus
        .by_ref()
        .take(turns)
        .flat_map(|turn| turn.messages)
        .map(|mut message| {
            message.provider_raw = None;
            message
        })
        .collect();
    messages.push(corpus.next().unwrap().messages.swap_remove(0));
    messages
}

#[test]
#[ignore = "bench: run by just bench-perf"]
fn bench_perf_messages_body() {
    let mut report = Report::new("bench_perf_messages_body");
    let config = AnthropicConfig::new();
    let model = opus();
    let mut points = Vec::new();
    for turns in TURNS {
        let messages = history(turns);
        let name = format!("{turns} turns");
        report.count(&format!("{name}: messages"), messages.len() as u64);
        let build = || {
            let body = request_body(&request(messages.clone()), &config, Some(&model)).unwrap();
            black_box(serde_json::to_vec(&body).unwrap());
        };
        let bytes = {
            let body = request_body(&request(messages.clone()), &config, Some(&model)).unwrap();
            serde_json::to_vec(&body).unwrap().len()
        };
        report.bytes(&format!("{name}: body as JSON"), bytes as u64);
        bench::reset_peak();
        let before = Memory::of_self().unwrap();
        build();
        let after = Memory::of_self().unwrap();
        report.bytes(&format!("{name}: peak growth"), after.peak.saturating_sub(before.rss));
        let samples = bench::run(2, 20, build);
        let stats = report.time(&format!("{name}: messages body"), &samples);
        points.push((messages.len() as u64, stats.p50));
    }
    report.slope("messages body over the messages", &points);
    report.finish();
}

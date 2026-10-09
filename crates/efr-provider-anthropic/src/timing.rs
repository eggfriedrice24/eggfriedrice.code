//! The debug lines that say how fast the API answers.
//!
//! Each model call writes two `phase` lines at debug level, in the format of
//! `efr-provider-openai` and the other phase lines (`docs/sandbox.md`):
//! `provider_accepted` when the first event of the stream arrives, and
//! `provider_first_event` when the first canonical event (text, reasoning or a tool
//! call) is ready, both with `transport=http connection=pool input=full`, so the log
//! compares the providers line for line.

use efr_stdx::time::Stopwatch;

/// The only transport of this provider, in the words of the OpenAI provider's lines.
const TRANSPORT: &str = "http";

/// reqwest keeps the connections of HTTP calls in a pool.
const CONNECTION: &str = "pool";

/// Every call sends its whole input.
const INPUT: &str = "full";

/// The clock of one model call, from the start of `Provider::stream`.
#[derive(Debug, Clone)]
pub(crate) struct Timing {
    watch: Stopwatch,
    accepted: bool,
    first_event: bool,
}

impl Timing {
    /// Starts measuring a call now.
    pub(crate) fn start() -> Timing {
        Timing { watch: Stopwatch::start(), accepted: false, first_event: false }
    }

    /// Writes `provider_accepted` once: the server's first event has arrived.
    pub(crate) fn accepted(&mut self) {
        if !std::mem::replace(&mut self.accepted, true) {
            self.line("provider_accepted");
        }
    }

    /// Writes `provider_first_event` once: the first canonical event is ready.
    pub(crate) fn first_event(&mut self) {
        if !std::mem::replace(&mut self.first_event, true) {
            self.line("provider_first_event");
        }
    }

    fn line(&self, phase: &'static str) {
        let watch = &self.watch;
        tracing::debug!(
            phase,
            transport = TRANSPORT,
            connection = CONNECTION,
            input = INPUT,
            elapsed_ms = %watch,
            "phase={phase} transport={TRANSPORT} connection={CONNECTION} input={INPUT} elapsed_ms={watch}"
        );
    }
}

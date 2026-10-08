//! The debug lines that say how fast each transport answers.
//!
//! Each model call writes two `phase` lines at debug level, in the format of the other
//! phase lines (`docs/sandbox.md`): `provider_accepted` when the server's first event
//! arrives, which says it took the request, and `provider_first_event` when the first
//! canonical event (text, reasoning or a tool call) is ready. Both carry the transport
//! and, for a WebSocket, whether the connection was new and whether the input was sent
//! whole, so the user can compare the two transports in the log.

use efr_stdx::time::Stopwatch;

/// How a model call was sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Route {
    /// `http` or `websocket`.
    pub(crate) transport: &'static str,
    /// `new` or `reused` for a WebSocket, `pool` for HTTP, whose connections reqwest
    /// keeps.
    pub(crate) connection: &'static str,
    /// `full` or `incremental`.
    pub(crate) input: &'static str,
}

impl Route {
    /// A call over HTTP.
    pub(crate) const HTTP: Route = Route { transport: "http", connection: "pool", input: "full" };
}

/// The clock of one model call, from the start of `Provider::stream`.
#[derive(Debug, Clone)]
pub(crate) struct Timing {
    watch: Stopwatch,
    route: Route,
    accepted: bool,
    first_event: bool,
}

impl Timing {
    /// Starts measuring a call now.
    pub(crate) fn start() -> Timing {
        Timing {
            watch: Stopwatch::start(),
            route: Route::HTTP,
            accepted: false,
            first_event: false,
        }
    }

    /// The same clock, for a call sent on `route`.
    pub(crate) fn on(mut self, route: Route) -> Timing {
        self.route = route;
        self
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
        let Route { transport, connection, input } = self.route;
        let watch = &self.watch;
        tracing::debug!(
            phase,
            transport,
            connection,
            input,
            elapsed_ms = %watch,
            "phase={phase} transport={transport} connection={connection} input={input} elapsed_ms={watch}"
        );
    }
}

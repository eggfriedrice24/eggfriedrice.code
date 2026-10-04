//! The hello exchange: which requests a connection may make before and after `hello`.
//!
//! The rule is a pure decision table so it can be tested without a socket:
//!
//! | Hello done | Request | Answer |
//! |---|---|---|
//! | no | `hello`, same protocol | run the hello exchange |
//! | no | `hello`, other protocol | `protocol_mismatch`, then close the connection |
//! | no | any other method | `unauthorized`; the method does not run |
//! | yes | `hello` | `conflict`; the first hello stands |
//! | yes | any other method | dispatch it |
//!
//! So a protocol mismatch is answered before any method runs, and nothing reaches the
//! dispatcher on a connection whose version was never checked.

use efr_protocol::{ErrorBody, ErrorCode, Hello, Method};

/// What the connection does with one request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Gate {
    /// Run the hello exchange with these params.
    Hello(Hello),
    /// Hand the method to the dispatcher.
    Dispatch(Method),
    /// Answer the request with this error and keep reading.
    Refuse(ErrorBody),
    /// Answer the request with this error, then close the connection.
    RefuseAndClose(ErrorBody),
}

/// Decides what happens to `method` on a connection that has (`hello_done`) or has not
/// completed hello, on a daemon that speaks `daemon_protocol`.
pub(crate) fn gate(hello_done: bool, method: Method, daemon_protocol: u32) -> Gate {
    match (hello_done, method) {
        (false, Method::Hello(hello)) if hello.protocol == daemon_protocol => Gate::Hello(hello),
        (false, Method::Hello(hello)) => {
            Gate::RefuseAndClose(ErrorBody::protocol_mismatch(daemon_protocol, hello.protocol))
        }
        (false, _) => Gate::Refuse(ErrorBody::new(
            ErrorCode::Unauthorized,
            "the connection must send hello before any other method",
        )),
        (true, Method::Hello(_)) => Gate::Refuse(ErrorBody::new(
            ErrorCode::Conflict,
            "hello was already sent on this connection",
        )),
        (true, method) => Gate::Dispatch(method),
    }
}

#[cfg(test)]
mod tests;

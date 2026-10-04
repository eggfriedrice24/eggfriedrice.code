//! The loopback listener the browser returns to after the login.
//!
//! The listener binds `127.0.0.1` only, so nothing off the machine can reach it, and it
//! lives only inside one login: [`CallbackListener::wait`] consumes it, and when the
//! wait ends, for any reason, the socket closes and every open connection is dropped.
//! [`LoginSlot`] lets one login run at a time.
//!
//! A request whose `state` does not match is answered with an error page and otherwise
//! ignored, as codex does (`codex-rs/login/src/server.rs:376-389`): it may come from a
//! stale browser tab or from a web page that guesses the port, and neither should end
//! the login the user is still completing. Only a callback with the right `state`
//! decides the outcome.

use std::convert::Infallible;
use std::future::ready;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use bytes::Bytes;
use http_body_util::Full;
use hyper::body::Incoming;
use hyper::header::{CACHE_CONTROL, CONTENT_TYPE, HeaderValue};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode, Uri};
use hyper_util::rt::TokioIo;
use secrecy::{ExposeSecret as _, SecretString};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio::task::JoinSet;

use crate::OAuthError;
use crate::authorize::CALLBACK_PATH;

/// What a callback with the right `state` decided: the authorization code, or why
/// there is none.
pub(crate) type Outcome = Result<SecretString, OAuthError>;

/// A small HTML page, built at compile time. Pages never include request data, so
/// nothing a URL carries can end up in markup.
macro_rules! page {
    ($title:literal, $text:literal) => {
        concat!(
            "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">",
            "<meta name=\"viewport\" content=\"width=device-width\">",
            "<title>efr: ",
            $title,
            "</title><style>body{font-family:system-ui,sans-serif;max-width:34rem;",
            "margin:4rem auto;padding:0 1rem;line-height:1.5}</style></head><body><h1>",
            $title,
            "</h1><p>",
            $text,
            "</p></body></html>\n"
        )
    };
}

const SUCCESS_PAGE: &str = page!(
    "Signed in",
    "efr received the sign-in. You can close this tab and return to the terminal."
);

const STALE_PAGE: &str = page!(
    "Sign-in link not recognised",
    "This page does not belong to the login efr is waiting for. Use the newest link \
     from the terminal, or start the login again."
);

const FAILED_PAGE: &str = page!(
    "Sign-in failed",
    "The sign-in did not finish. The terminal shows why; start the login again from there."
);

/// One login at a time.
///
/// The daemon keeps one slot for the subscription login; a second `admin.login_openai`
/// while the first is still waiting gets [`OAuthError::LoginInProgress`] instead of a
/// confusing failure to bind the port.
#[derive(Debug, Clone, Default)]
pub(crate) struct LoginSlot {
    busy: Arc<AtomicBool>,
}

impl LoginSlot {
    /// Claims the slot until the returned guard is dropped, or fails when a login holds
    /// it already.
    pub(crate) fn claim(&self) -> Result<SlotGuard, OAuthError> {
        self.busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| SlotGuard { busy: Arc::clone(&self.busy) })
            .map_err(|_| OAuthError::LoginInProgress)
    }
}

/// The claim on a [`LoginSlot`]; dropping it frees the slot.
#[derive(Debug)]
pub(crate) struct SlotGuard {
    busy: Arc<AtomicBool>,
}

impl Drop for SlotGuard {
    fn drop(&mut self) {
        self.busy.store(false, Ordering::Release);
    }
}

/// A bound callback listener that has not served anything yet.
///
/// Connections that arrive before [`wait`](CallbackListener::wait) runs wait in the
/// kernel's backlog, so the authorize URL can be handed out as soon as the listener
/// exists.
#[derive(Debug)]
pub(crate) struct CallbackListener {
    listener: TcpListener,
    port: u16,
}

impl CallbackListener {
    /// Binds `127.0.0.1:port`; port `0` picks a free one.
    pub(crate) async fn bind(port: u16) -> Result<Self, OAuthError> {
        let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
        let listener =
            TcpListener::bind(addr).await.map_err(|source| OAuthError::Bind { addr, source })?;
        let port =
            listener.local_addr().map_err(|source| OAuthError::Bind { addr, source })?.port();
        Ok(CallbackListener { listener, port })
    }

    /// The port the listener is bound to.
    pub(crate) fn port(&self) -> u16 {
        self.port
    }

    /// Serves callbacks until one with `expected_state` arrives, and returns what it
    /// carried. The page of that callback is written before this returns.
    pub(crate) async fn wait(self, expected_state: SecretString) -> Outcome {
        let expected = Arc::new(expected_state);
        let (sender, mut outcomes) = mpsc::channel(1);
        // Dropping the set when this returns aborts every connection still open, such
        // as a browser's idle preconnect.
        let mut connections = JoinSet::new();
        loop {
            tokio::select! {
                Some(outcome) = outcomes.recv() => return outcome,
                accepted = self.listener.accept() => {
                    let (stream, _peer) = accepted.map_err(|source| OAuthError::Accept { source })?;
                    connections.spawn(serve(stream, Arc::clone(&expected), sender.clone()));
                }
                Some(_) = connections.join_next(), if !connections.is_empty() => {}
            }
        }
    }
}

/// Serves one connection. The outcome goes out only after hyper has written the page
/// and closed the connection, so the browser never sees a reset instead of the page.
async fn serve(stream: TcpStream, expected: Arc<SecretString>, outcomes: mpsc::Sender<Outcome>) {
    let decided: Arc<Mutex<Option<Outcome>>> = Arc::default();
    let service = {
        let decided = Arc::clone(&decided);
        service_fn(move |request: Request<Incoming>| {
            let Reply { status, page, outcome } =
                reply(request.method(), request.uri(), expected.expose_secret());
            if let Some(outcome) = outcome {
                lock(&decided).get_or_insert(outcome);
            }
            ready(Ok::<_, Infallible>(response(status, page)))
        })
    };
    let served = http1::Builder::new()
        .keep_alive(false)
        .serve_connection(TokioIo::new(stream), service)
        .await;
    if let Err(error) = served {
        tracing::debug!(%error, "a login callback connection failed");
    }
    let outcome = lock(&decided).take();
    if let Some(outcome) = outcome {
        // The receiver is gone only when the wait already ended, and then nobody needs
        // this outcome.
        let _ = outcomes.send(outcome).await;
    }
}

/// The answer to one request.
#[derive(Debug)]
pub(crate) struct Reply {
    pub(crate) status: StatusCode,
    pub(crate) page: &'static str,
    /// Set only for a callback with the right `state`.
    pub(crate) outcome: Option<Outcome>,
}

/// Decides the answer to a request, without IO.
pub(crate) fn reply(method: &Method, uri: &Uri, expected_state: &str) -> Reply {
    let plain = |status, page| Reply { status, page, outcome: None };
    if uri.path() != CALLBACK_PATH {
        return plain(StatusCode::NOT_FOUND, "not found\n");
    }
    if method != Method::GET {
        return plain(StatusCode::METHOD_NOT_ALLOWED, "method not allowed\n");
    }
    let mut params = CallbackParams::default();
    for (name, value) in url::form_urlencoded::parse(uri.query().unwrap_or_default().as_bytes()) {
        let slot = match name.as_ref() {
            "code" => &mut params.code,
            "state" => &mut params.state,
            "error" => &mut params.error,
            "error_description" => &mut params.error_description,
            _ => continue,
        };
        // The first occurrence wins, so a parameter repeated later cannot override it.
        if slot.is_none() {
            *slot = Some(SecretString::from(value.into_owned()));
        }
    }
    let state_matches = params
        .state
        .as_ref()
        .is_some_and(|state| constant_time_eq(state.expose_secret(), expected_state));
    if !state_matches {
        return plain(StatusCode::BAD_REQUEST, STALE_PAGE);
    }
    let failed = |error| Reply {
        status: StatusCode::BAD_REQUEST,
        page: FAILED_PAGE,
        outcome: Some(Err(error)),
    };
    if let Some(error) = params.error.filter(|error| !error.expose_secret().is_empty()) {
        return failed(OAuthError::Authorization {
            error: error.expose_secret().to_owned(),
            description: params.error_description.map(|text| text.expose_secret().to_owned()),
        });
    }
    match params.code.filter(|code| !code.expose_secret().is_empty()) {
        Some(code) => Reply { status: StatusCode::OK, page: SUCCESS_PAGE, outcome: Some(Ok(code)) },
        None => failed(OAuthError::MissingCode),
    }
}

/// The query parameters of a callback. Each is held as a secret until it is known to
/// be harmless, because the code and the state are credentials.
#[derive(Default)]
struct CallbackParams {
    code: Option<SecretString>,
    state: Option<SecretString>,
    error: Option<SecretString>,
    error_description: Option<SecretString>,
}

fn response(status: StatusCode, page: &'static str) -> Response<Full<Bytes>> {
    let content_type = if page.starts_with("<!doctype html>") {
        "text/html; charset=utf-8"
    } else {
        "text/plain; charset=utf-8"
    };
    let mut response = Response::new(Full::new(Bytes::from_static(page.as_bytes())));
    *response.status_mut() = status;
    let headers = response.headers_mut();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static(content_type));
    // The page answers a URL that carried a code; nothing about it should be kept.
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// Compares without stopping at the first difference, so the time an answer takes says
/// nothing about how much of a guessed `state` was right.
fn constant_time_eq(left: &str, right: &str) -> bool {
    let (left, right) = (left.as_bytes(), right.as_bytes());
    left.len() == right.len()
        && left.iter().zip(right).fold(0_u8, |diff, (a, b)| diff | (a ^ b)) == 0
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    // The guarded value is a plain `Option`, consistent at every point, so a panic
    // elsewhere while holding the lock leaves nothing broken.
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests;

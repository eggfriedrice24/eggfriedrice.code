//! The screen backend of the hidden shells.
//!
//! This is the only file with a `screen-ghostty` cfg. With the feature, the backend is
//! libghostty-vt unless the config asks for vt100 (`EFR_SCREEN=vt100`); without it,
//! vt100 is the only backend and asking for ghostty logs a warning. The choice is
//! logged once at startup. Both run through the same `ScreenActor`, so the rest of the
//! daemon never names a backend.

use std::sync::Arc;

use efr_config::ScreenChoice;
use efr_holder::Size;
use efr_screen::{ScreenActor, ScreenError, ScreenEvents, ScreenHandle};
use efr_shell::ScreenFactory;

/// A screen backend this build can run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScreenBackend {
    Vt100,
    #[cfg(feature = "screen-ghostty")]
    Ghostty,
}

impl ScreenBackend {
    /// The name `admin.status` reports.
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            ScreenBackend::Vt100 => "vt100",
            #[cfg(feature = "screen-ghostty")]
            ScreenBackend::Ghostty => "ghostty",
        }
    }

    /// The factory that starts screens of this backend.
    pub(crate) fn factory(self) -> Arc<dyn ScreenFactory> {
        match self {
            ScreenBackend::Vt100 => Arc::new(Vt100Screens),
            #[cfg(feature = "screen-ghostty")]
            ScreenBackend::Ghostty => Arc::new(GhosttyScreens),
        }
    }
}

/// The backend for `choice` in this build, logged at `info`.
pub(crate) fn choose(choice: ScreenChoice) -> ScreenBackend {
    let backend = select(choice);
    tracing::info!(backend = backend.as_str(), requested = choice.as_str(), "screen backend");
    backend
}

#[cfg(feature = "screen-ghostty")]
fn select(choice: ScreenChoice) -> ScreenBackend {
    match choice {
        ScreenChoice::Vt100 => ScreenBackend::Vt100,
        ScreenChoice::Auto | ScreenChoice::Ghostty => ScreenBackend::Ghostty,
    }
}

#[cfg(not(feature = "screen-ghostty"))]
fn select(choice: ScreenChoice) -> ScreenBackend {
    if choice == ScreenChoice::Ghostty {
        tracing::warn!("ghostty screens were asked for, but this build has only vt100");
    }
    ScreenBackend::Vt100
}

/// Starts vt100 screens.
#[derive(Debug, Clone, Copy)]
struct Vt100Screens;

impl ScreenFactory for Vt100Screens {
    fn spawn(&self, name: &str, size: Size) -> Result<(ScreenHandle, ScreenEvents), ScreenError> {
        ScreenActor::spawn(name, efr_screen_vt100::factory(size), size)
    }
}

/// Starts libghostty-vt screens.
#[cfg(feature = "screen-ghostty")]
#[derive(Debug, Clone, Copy)]
struct GhosttyScreens;

#[cfg(feature = "screen-ghostty")]
impl ScreenFactory for GhosttyScreens {
    fn spawn(&self, name: &str, size: Size) -> Result<(ScreenHandle, ScreenEvents), ScreenError> {
        ScreenActor::spawn(name, efr_screen_ghostty::factory(size), size)
    }
}

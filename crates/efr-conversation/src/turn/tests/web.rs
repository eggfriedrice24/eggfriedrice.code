//! The gate of web search: which hosted tools the requests of a turn carry, by the mode
//! and the `[web]` setting, on a provider that has the stored copy and the live web.

use std::sync::Arc;

use async_trait::async_trait;
use efr_protocol::{Event, Mode};
use efr_provider::{
    HostedTool, Provider, ProviderError, ProviderId, ProviderStream, Request, WebAccess, WebSupport,
};
use pretty_assertions::assert_eq;

use crate::WebSearch;
use crate::testing::{Recorder, Setup};

/// A provider that answers as [`Recorder`] does and says that it has `support`.
#[derive(Debug)]
struct WebProvider {
    inner: Arc<Recorder>,
    support: WebSupport,
}

#[async_trait]
impl Provider for WebProvider {
    fn id(&self) -> &ProviderId {
        self.inner.id()
    }

    fn web_search(&self) -> WebSupport {
        self.support
    }

    async fn stream(&self, request: Request) -> Result<ProviderStream, ProviderError> {
        self.inner.stream(request).await
    }
}

/// The hosted tools of the one request of a turn in `mode` with `web.search = search`,
/// on a provider with `support`.
async fn hosted_of(mode: Mode, search: WebSearch, support: WebSupport) -> Vec<HostedTool> {
    let mut setup = Setup::new();
    setup.config.mode = mode;
    setup.config.web.search = search;
    setup.config.web.max_searches = 9;
    let recorder = Recorder::new(1);
    let provider = WebProvider { inner: Arc::clone(&recorder), support };
    let mut h = setup.start_model(Arc::new(provider)).await;
    let sent = h.prompt("what changed in rust 2026").await;
    let end = h.wait_end(sent.turn_id).await;
    assert!(matches!(end, Event::TurnCompleted { .. }), "{end:?}");
    let requests = recorder.requests();
    assert_eq!(requests.len(), 1, "{requests:?}");
    requests.into_iter().next().map(|request| request.hosted).unwrap_or_default()
}

fn web(access: WebAccess) -> Vec<HostedTool> {
    vec![HostedTool::WebSearch { access, max_uses: Some(9), urls_from_tool_results: false }]
}

#[tokio::test]
async fn a_manual_turn_sends_no_web_tool_whatever_the_setting_says() {
    for search in [WebSearch::Mode, WebSearch::Cached, WebSearch::Live] {
        let hosted = hosted_of(Mode::Manual, search, WebSupport::StoredOrLive).await;
        assert_eq!(hosted, Vec::new(), "{search:?}");
    }
}

#[tokio::test]
async fn a_turn_with_web_search_off_sends_no_web_tool() {
    for mode in [Mode::Cautious, Mode::Auto] {
        let hosted = hosted_of(mode, WebSearch::Off, WebSupport::StoredOrLive).await;
        assert_eq!(hosted, Vec::new(), "{mode:?}");
    }
}

#[tokio::test]
async fn a_cautious_turn_reads_the_stored_copy_and_an_auto_turn_the_live_web() {
    let cautious = hosted_of(Mode::Cautious, WebSearch::Mode, WebSupport::StoredOrLive).await;
    assert_eq!(cautious, web(WebAccess::Stored));
    let auto = hosted_of(Mode::Auto, WebSearch::Mode, WebSupport::StoredOrLive).await;
    assert_eq!(auto, web(WebAccess::Live));
    let claude = hosted_of(Mode::Cautious, WebSearch::Mode, WebSupport::Live).await;
    assert_eq!(claude, web(WebAccess::Live));
}

#[tokio::test]
async fn a_provider_without_support_gets_no_web_tool() {
    let hosted = hosted_of(Mode::Auto, WebSearch::Live, WebSupport::None).await;
    assert_eq!(hosted, Vec::new());
}

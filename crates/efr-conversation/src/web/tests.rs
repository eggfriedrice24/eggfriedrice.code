use efr_protocol::Mode;
use efr_provider::{HostedTool, WebAccess, WebSupport};
use pretty_assertions::assert_eq;

use super::{WebPlan, WebSearch, WhyOff, hosted_tools, plan};

use WebPlan::{Live, Off, Stored};

/// The table of the build spec (3.5), and DECISIONS-web 2: every mode, every setting,
/// every support.
#[test]
fn the_plan_follows_the_table_of_the_spec() {
    let none = WebSupport::None;
    let claude = WebSupport::Live;
    let openai = WebSupport::StoredOrLive;
    let cases = [
        // No support: off, whatever the mode and the setting say.
        (Mode::Auto, WebSearch::Live, none, Off(WhyOff::NoSupport)),
        (Mode::Manual, WebSearch::Mode, none, Off(WhyOff::NoSupport)),
        // Manual: off, whatever the setting says.
        (Mode::Manual, WebSearch::Mode, openai, Off(WhyOff::Manual)),
        (Mode::Manual, WebSearch::Live, openai, Off(WhyOff::Manual)),
        (Mode::Manual, WebSearch::Cached, claude, Off(WhyOff::Manual)),
        (Mode::Manual, WebSearch::Off, claude, Off(WhyOff::Manual)),
        // Off: off in every mode.
        (Mode::Cautious, WebSearch::Off, openai, Off(WhyOff::Setting)),
        (Mode::Auto, WebSearch::Off, claude, Off(WhyOff::Setting)),
        (Mode::Full, WebSearch::Off, openai, Off(WhyOff::Setting)),
        // Live: the live web.
        (Mode::Cautious, WebSearch::Live, openai, Live),
        (Mode::Cautious, WebSearch::Live, claude, Live),
        (Mode::Full, WebSearch::Live, openai, Live),
        // Cached: the stored copy where the provider has one.
        (Mode::Auto, WebSearch::Cached, openai, Stored),
        (Mode::Full, WebSearch::Cached, openai, Stored),
        (Mode::Auto, WebSearch::Cached, claude, Live),
        // Mode: the table by mode of DECISIONS-web 2.
        (Mode::Cautious, WebSearch::Mode, openai, Stored),
        (Mode::Cautious, WebSearch::Mode, claude, Live),
        (Mode::Auto, WebSearch::Mode, openai, Live),
        (Mode::Auto, WebSearch::Mode, claude, Live),
        (Mode::Full, WebSearch::Mode, openai, Live),
        (Mode::Full, WebSearch::Mode, claude, Live),
    ];
    for (mode, setting, support, expected) in cases {
        assert_eq!(plan(mode, setting, support), expected, "{mode:?} {setting:?} {support:?}");
    }
}

#[test]
fn a_plan_that_is_off_gives_no_hosted_tool() {
    for why in [WhyOff::NoSupport, WhyOff::Manual, WhyOff::Setting] {
        for mode in Mode::ALL {
            assert_eq!(hosted_tools(Off(why), mode, 20), Vec::new());
        }
    }
}

#[test]
fn the_web_tool_has_its_access_and_limit_and_takes_tool_urls_only_in_full() {
    let tool = |access, urls_from_tool_results| HostedTool::WebSearch {
        access,
        max_uses: Some(7),
        urls_from_tool_results,
    };
    assert_eq!(hosted_tools(Stored, Mode::Cautious, 7), vec![tool(WebAccess::Stored, false)]);
    assert_eq!(hosted_tools(Live, Mode::Auto, 7), vec![tool(WebAccess::Live, false)]);
    assert_eq!(hosted_tools(Live, Mode::Full, 7), vec![tool(WebAccess::Live, true)]);
}

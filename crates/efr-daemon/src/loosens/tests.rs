use std::path::PathBuf;

use efr_config::{Settings, SudoCache};
use efr_permissions::{Action, CommandPattern, Effect, Policy, Resource, Rule};
use efr_protocol::Mode;
use pretty_assertions::assert_eq;

use super::{loosening, loosens};

fn rules(effect: Effect) -> Policy {
    let rule = Rule::new(Action::Execute, Resource::Command(CommandPattern::new("cargo")), effect);
    Policy::new(vec![rule]).unwrap()
}

#[test]
fn loosening_lists_each_key_that_loosens_in_order() {
    let before = Settings::default();
    let mut next = Settings::default();
    next.permissions.rules = rules(Effect::Allow);
    next.permissions.allow_full = true;
    next.permissions.mode = Mode::Full;
    next.shell.sudo_cache = SudoCache::Keep;
    next.sandbox.write_roots.push(PathBuf::from("~/notes"));
    next.jobs.max_minutes += 1;

    let keys = loosening(&before, &next);

    let mut expected = vec!["permissions.rules", "permissions.mode", "permissions.allow_full"];
    if before.shell.sudo_cache == SudoCache::PerCall {
        expected.push("shell.sudo_cache");
    }
    expected.extend(["sandbox.write_roots", "jobs.max_minutes"]);
    assert_eq!(keys, expected);
    assert!(loosens(&before, &next));
    assert_eq!(loosening(&next, &before), Vec::<&str>::new(), "the way back asks more");
}

#[test]
fn a_new_ask_rule_or_a_removed_allow_rule_does_not_loosen() {
    let mut allow = Settings::default();
    allow.permissions.rules = rules(Effect::Allow);
    let mut ask = Settings::default();
    ask.permissions.rules = rules(Effect::Ask);

    assert_eq!(loosening(&Settings::default(), &ask), Vec::<&str>::new());
    assert_eq!(loosening(&allow, &Settings::default()), Vec::<&str>::new());
    assert_eq!(loosening(&ask, &Settings::default()), ["permissions.rules"], "a removed ask");
}

#[test]
fn forward_ports_on_loosens_and_off_does_not() {
    let mut off = Settings::default();
    off.jobs.forward_ports = false;
    let on = Settings::default();
    assert_eq!(loosening(&off, &on), ["jobs.forward_ports"], "a host port opens to a job");
    assert_eq!(loosening(&on, &off), Vec::<&str>::new(), "off closes them");
    assert!(super::jobs_loosens(&off.jobs, &on.jobs));
}

#[test]
fn the_phone_listener_on_or_another_tailscaled_socket_loosens() {
    let off = Settings::default();
    let mut on = Settings::default();
    on.remote.enabled = true;
    assert_eq!(loosening(&off, &on), ["remote.enabled"], "a port opens to the tailnet");
    assert_eq!(loosening(&on, &off), Vec::<&str>::new(), "off closes it");

    let mut moved = Settings::default();
    moved.remote.tailscaled_socket = PathBuf::from("/run/user/1000/fake-tailscaled.sock");
    assert_eq!(
        loosening(&off, &moved),
        ["remote.tailscaled_socket"],
        "another socket's whois decides who comes in"
    );
    let mut port = Settings::default();
    port.remote.port += 1;
    assert_eq!(loosening(&off, &port), Vec::<&str>::new(), "another port admits no one more");
}

#[test]
fn web_search_up_the_order_off_cached_mode_live_loosens() {
    use efr_config::WebSearch;

    let with = |search| {
        let mut settings = Settings::default();
        settings.web.search = search;
        settings
    };
    let cases = [
        (WebSearch::Mode, WebSearch::Live, true),
        (WebSearch::Cached, WebSearch::Live, true),
        (WebSearch::Off, WebSearch::Mode, true),
        (WebSearch::Off, WebSearch::Cached, true),
        (WebSearch::Off, WebSearch::Live, true),
        (WebSearch::Live, WebSearch::Mode, false),
        (WebSearch::Mode, WebSearch::Cached, false),
        // An OpenAI model goes from the stored copy to the live web in auto and full.
        (WebSearch::Cached, WebSearch::Mode, true),
        (WebSearch::Cached, WebSearch::Off, false),
        (WebSearch::Mode, WebSearch::Off, false),
        (WebSearch::Live, WebSearch::Live, false),
    ];
    for (before, next, loosens) in cases {
        let keys = loosening(&with(before), &with(next));
        let expected: &[&str] = if loosens { &["web.search"] } else { &[] };
        assert_eq!(keys, expected, "{before:?} to {next:?}");
    }
    let mut more = Settings::default();
    more.web.max_searches += 10;
    assert_eq!(loosening(&Settings::default(), &more), Vec::<&str>::new());
}

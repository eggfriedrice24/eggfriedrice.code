use std::sync::Arc;
use std::time::Duration;

use efr_config::Settings;
use efr_protocol::{AccountOut, AccountState, LimitKind, LimitReached, OthersFull};
use efr_providers::openai::{Backend, Catalog, ModelCatalog};
use efr_test_support::TestClock;
use pretty_assertions::assert_eq;

use super::Catalogs;
use crate::catalog::{Models, ProviderCatalog};
use crate::providers::{ANTHROPIC, API, LimitBook, SUBSCRIPTION};

fn openai(provider: &str, backend: Backend) -> Arc<Models> {
    let catalog = ProviderCatalog::OpenAi(ModelCatalog::new(Catalog::builtin(backend)));
    Arc::new(Models::fixed(provider, catalog))
}

fn catalogs() -> Catalogs {
    // NOTE: a list of Anthropic needs a fetch or a cache file; only the provider of
    // each catalog counts here.
    Catalogs::new(
        openai(SUBSCRIPTION, Backend::Subscription),
        vec![openai(API, Backend::Api), openai(ANTHROPIC, Backend::Api)],
        Arc::new(LimitBook::new(TestClock::new().shared())),
    )
}

/// The ChatGPT plan, its second account `b`, an account `c` and an API key, with the
/// limits on `clock`.
fn accounts(clock: &TestClock) -> Catalogs {
    Catalogs::new(
        openai(SUBSCRIPTION, Backend::Subscription),
        vec![
            openai(API, Backend::Api),
            openai(PLAN_B, Backend::Subscription),
            openai(PLAN_C, Backend::Subscription),
        ],
        Arc::new(LimitBook::new(clock.shared())),
    )
}

const PLAN_B: &str = "openai-subscription.b";
const PLAN_C: &str = "openai-subscription.c";

/// A model that the built-in table of the ChatGPT plan lists.
fn plan_model(catalogs: &Catalogs, settings: &Settings) -> String {
    catalogs.configured(settings).effective(settings)[0].id.clone()
}

fn usage_until(clock: &TestClock, after: Duration) -> LimitReached {
    LimitReached { kind: LimitKind::Usage, resets_at: Some(clock.shared().now() + after) }
}

/// Each account of an offer, with its state and its reset.
fn offered(out: &AccountOut) -> Vec<(&str, AccountState, Option<jiff::Timestamp>)> {
    out.others.iter().map(|other| (other.provider.as_str(), other.state, other.resets_at)).collect()
}

#[test]
fn an_account_that_the_config_names_is_listed_last_as_signed_out_with_its_label() {
    let clock = TestClock::new();
    let catalogs = accounts(&clock);
    let text =
        "[accounts.labels]\n\"openai-subscription.c\" = \"Team\"\nopenai-subscription = \"Home\"\n";
    let settings = Settings::parse(std::path::Path::new("/c/config.toml"), Some(text)).unwrap();
    catalogs.set_logged_in(SUBSCRIPTION, true);
    catalogs.set_logged_in(PLAN_B, true);

    assert_eq!(providers(&catalogs, &settings), [SUBSCRIPTION, PLAN_B, PLAN_C]);
    let listed = catalogs.listed(&settings);
    let shown: Vec<(Option<&str>, Option<&str>, bool)> = listed
        .catalogs
        .iter()
        .map(|status| (status.provider.as_deref(), status.label.as_deref(), status.signed_out))
        .collect();
    assert_eq!(
        shown,
        [
            (Some(SUBSCRIPTION), Some("Home"), false),
            (Some(PLAN_B), None, false),
            (Some(PLAN_C), Some("Team"), true),
        ]
    );
    assert!(
        listed
            .models
            .iter()
            .filter(|model| model.default)
            .all(|model| { model.provider.as_deref() == Some(SUBSCRIPTION) })
    );
    // NOTE: a turn tries the accounts with a login first.
    let routed = catalogs.routed(&settings);
    assert_eq!(routed.last().unwrap().provider.as_deref(), Some(PLAN_C));

    // A limit shows on an account with a login, never on a signed-out one.
    catalogs.limits().note(PLAN_B, &usage_until(&clock, Duration::from_secs(600)));
    catalogs.limits().note(PLAN_C, &usage_until(&clock, Duration::from_secs(600)));
    let listed = catalogs.listed(&settings);
    assert!(listed.catalogs[1].limits.as_ref().is_some_and(|limits| limits.reached.is_some()));
    assert_eq!(listed.catalogs[2].limits, None);
}

/// TM-Accounts 8 and TM-Commands 2 (D2): the list is grouped by kind, so a signed-out
/// account stands inside its kind, and the default account comes first in its kind.
#[test]
fn the_list_groups_the_accounts_by_kind_with_a_signed_out_one_inside_its_kind() {
    let clock = TestClock::new();
    let catalogs = accounts(&clock);
    let text = "[accounts.labels]\n\"openai-subscription.c\" = \"Team\"\n";
    let settings = Settings::parse(std::path::Path::new("/c/config.toml"), Some(text)).unwrap();
    catalogs.set_logged_in(SUBSCRIPTION, true);
    catalogs.set_logged_in(API, true);
    catalogs.set_logged_in(PLAN_B, true);

    assert_eq!(providers(&catalogs, &settings), [SUBSCRIPTION, PLAN_B, PLAN_C, API]);
    assert_eq!(catalogs.order(&settings), [SUBSCRIPTION, PLAN_B, PLAN_C, API]);

    // NOTE: with b as the account of new conversations, b leads its kind and the
    // default account of the kind comes next.
    let mut on_b = settings.clone();
    on_b.model.provider = PLAN_B.to_owned();
    assert_eq!(catalogs.order(&on_b), [PLAN_B, SUBSCRIPTION, PLAN_C, API]);
}

#[test]
fn the_offer_names_the_accounts_of_the_model_a_full_one_only_when_it_frees_up_first() {
    let clock = TestClock::new();
    let catalogs = accounts(&clock);
    let text = "[accounts.labels]\n\"openai-subscription.c\" = \"Team\"\n";
    let settings = Settings::parse(std::path::Path::new("/c/config.toml"), Some(text)).unwrap();
    for provider in [SUBSCRIPTION, PLAN_B] {
        catalogs.set_logged_in(provider, true);
    }
    let model = plan_model(&catalogs, &settings);
    let stopped = usage_until(&clock, Duration::from_secs(3600));

    let out = catalogs.account_out(&settings, SUBSCRIPTION, &model, stopped.clone());
    assert_eq!(out.provider, SUBSCRIPTION);
    assert_eq!(out.model, model);
    assert_eq!(out.reached, stopped);
    // NOTE: the API key has no login, and `[accounts]` does not name it.
    assert_eq!(
        offered(&out),
        [(PLAN_B, AccountState::Ready, None), (PLAN_C, AccountState::SignedOut, None)]
    );
    assert_eq!(out.others[1].label.as_deref(), Some("Team"));

    // b is full too: it is named only while its reset comes first.
    let sooner = usage_until(&clock, Duration::from_secs(600));
    catalogs.limits().note(PLAN_B, &sooner);
    let out = catalogs.account_out(&settings, SUBSCRIPTION, &model, stopped.clone());
    assert_eq!(offered(&out)[0], (PLAN_B, AccountState::Full, sooner.resets_at));
    catalogs.limits().note(PLAN_B, &usage_until(&clock, Duration::from_secs(7200)));
    let out = catalogs.account_out(&settings, SUBSCRIPTION, &model, stopped);
    assert_eq!(offered(&out), [(PLAN_C, AccountState::SignedOut, None)]);
    // With no reset of the stopped account, any known reset comes first.
    let credit = LimitReached { kind: LimitKind::Credit, resets_at: None };
    let out = catalogs.account_out(&settings, SUBSCRIPTION, &model, credit);
    assert_eq!(offered(&out)[0].1, AccountState::Full);

    // An account that does not list the model is never named.
    let out = catalogs.account_out(&settings, SUBSCRIPTION, "no-such-model", sooner);
    assert_eq!(out.others, []);
    assert_eq!(out.others_full, None);
}

/// D3: each other account of the model is full until after the stopped one, so the
/// offer names none; `others_full` says so with the soonest reset of those accounts.
#[test]
fn the_accounts_that_are_full_for_longer_give_the_soonest_reset() {
    let clock = TestClock::new();
    let catalogs = accounts(&clock);
    let settings = Settings::default();
    for provider in [SUBSCRIPTION, PLAN_B, PLAN_C] {
        catalogs.set_logged_in(provider, true);
    }
    let model = plan_model(&catalogs, &settings);
    let stopped = usage_until(&clock, Duration::from_secs(3600));
    let b_later = usage_until(&clock, Duration::from_secs(7200));
    let c_latest = usage_until(&clock, Duration::from_secs(9000));
    catalogs.limits().note(PLAN_B, &b_later);
    catalogs.limits().note(PLAN_C, &c_latest);

    let out = catalogs.account_out(&settings, SUBSCRIPTION, &model, stopped.clone());
    assert_eq!(out.others, [], "no account frees up first");
    assert_eq!(out.others_full, Some(OthersFull { resets_at: b_later.resets_at }));

    // A full account with no reset counts too, but gives no time.
    catalogs.limits().note(PLAN_B, &LimitReached { kind: LimitKind::Credit, resets_at: None });
    let out = catalogs.account_out(&settings, SUBSCRIPTION, &model, stopped.clone());
    assert_eq!(out.others_full, Some(OthersFull { resets_at: c_latest.resets_at }));

    // A ready account is offered, and the full one still counts.
    catalogs.limits().clear(PLAN_C);
    let out = catalogs.account_out(&settings, SUBSCRIPTION, &model, stopped);
    assert_eq!(offered(&out), [(PLAN_C, AccountState::Ready, None)]);
    assert_eq!(out.others_full, Some(OthersFull { resets_at: None }));
}

/// An account that `[accounts]` names and that never had a login has no list. It is of
/// the kind of the account that is out, so the offer names it with its login line
/// (TM-Accounts 5). One of another kind with no list is not named.
#[test]
fn a_named_account_that_never_had_a_login_is_offered_signed_out() {
    let anthropic = |provider: &str| {
        let catalog = ProviderCatalog::Anthropic(efr_providers::anthropic::ModelCatalog::new());
        Arc::new(Models::fixed(provider, catalog))
    };
    let catalogs = Catalogs::new(
        anthropic(ANTHROPIC),
        vec![anthropic("anthropic-api.personal"), anthropic("anthropic-api.work")],
        Arc::new(LimitBook::new(TestClock::new().shared())),
    );
    let text = "[model]\nprovider = \"anthropic-api\"\n\n[accounts.labels]\n\
                \"anthropic-api.personal\" = \"Personal\"\n";
    let settings = Settings::parse(std::path::Path::new("/c/config.toml"), Some(text)).unwrap();
    catalogs.set_logged_in(ANTHROPIC, true);
    let spend = LimitReached { kind: LimitKind::SpendLimit, resets_at: None };

    let out = catalogs.account_out(&settings, ANTHROPIC, "claude-opus-5-5", spend);

    // NOTE: work has no login and `[accounts]` does not name it.
    assert_eq!(offered(&out), [("anthropic-api.personal", AccountState::SignedOut, None)]);
    let other_kind = LimitReached { kind: LimitKind::Credit, resets_at: None };
    let out = catalogs.account_out(&settings, "openai-api", "claude-opus-5-5", other_kind);
    assert_eq!(offered(&out), [], "the Claude accounts are of another kind");
}

/// The providers of the listed models, each once, in their order.
fn providers(catalogs: &Catalogs, settings: &Settings) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    for model in catalogs.listed(settings).models {
        let provider = model.provider.expect("each model names its provider");
        if seen.last() != Some(&provider) {
            assert!(!seen.contains(&provider), "{provider} comes twice");
            seen.push(provider);
        }
    }
    seen
}

#[test]
fn the_configured_provider_comes_first_and_others_only_with_a_login() {
    let catalogs = catalogs();
    let mut settings = Settings::default();
    settings.model.provider = ANTHROPIC.to_owned();

    // Without a login the configured provider is listed all the same.
    assert_eq!(providers(&catalogs, &settings), [ANTHROPIC]);
    assert_eq!(catalogs.listed(&settings).catalogs.len(), 1);

    catalogs.set_logged_in(API, true);
    catalogs.set_logged_in(SUBSCRIPTION, true);
    assert_eq!(providers(&catalogs, &settings), [ANTHROPIC, SUBSCRIPTION, API]);
    let listed = catalogs.listed(&settings);
    assert_eq!(listed.catalogs.len(), 3);
    let defaults: Vec<_> = listed.models.iter().filter(|model| model.default).collect();
    assert_eq!(defaults.len(), 1, "{defaults:?}");
    assert_eq!(defaults[0].provider.as_deref(), Some(ANTHROPIC));

    catalogs.set_logged_in(SUBSCRIPTION, false);
    assert_eq!(providers(&catalogs, &settings), [ANTHROPIC, API]);
}

#[test]
fn the_list_follows_the_provider_of_the_settings() {
    let catalogs = catalogs();
    catalogs.set_logged_in(ANTHROPIC, true);
    let mut settings = Settings::default();
    settings.model.provider = SUBSCRIPTION.to_owned();
    assert_eq!(providers(&catalogs, &settings), [SUBSCRIPTION, ANTHROPIC]);

    settings.model.provider = ANTHROPIC.to_owned();
    assert_eq!(providers(&catalogs, &settings), [ANTHROPIC]);
    assert_eq!(catalogs.configured(&settings).provider(), ANTHROPIC);
}

#[test]
fn the_routing_order_puts_a_provider_without_a_login_behind_the_others() {
    let catalogs = catalogs();
    let settings = Settings::default();
    assert_eq!(settings.model.provider, SUBSCRIPTION);
    catalogs.set_logged_in(API, true);

    let routed: Vec<_> =
        catalogs.routed(&settings).into_iter().map(|model| model.provider.unwrap()).collect();
    let first_subscription = routed.iter().position(|provider| provider == SUBSCRIPTION);
    assert_eq!(routed.first().map(String::as_str), Some(API), "{routed:?}");
    assert!(first_subscription.is_some(), "the configured provider stays in the list");
    assert!(routed[..first_subscription.unwrap()].iter().all(|provider| provider == API));
    // The listing keeps its own order: the configured provider first.
    let listed = catalogs.listed(&settings).models;
    assert_eq!(listed[0].provider.as_deref(), Some(SUBSCRIPTION));

    // With a login, the configured provider routes first again.
    catalogs.set_logged_in(SUBSCRIPTION, true);
    assert_eq!(
        catalogs.routed(&settings)[0].provider.as_deref(),
        Some(SUBSCRIPTION),
        "a shared id goes to the provider of new conversations"
    );
}

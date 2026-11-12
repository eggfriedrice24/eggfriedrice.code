use efr_protocol::{
    AdminStatusResult, CatalogOrigin, CatalogStatus, Method, ModelInfo, ModelSource,
    ModelsListResult, Origin, ProviderStatus,
};
use efr_render::RenderOptions;
use efr_test_support::design::{Depth, Frame, assert_rows};
use jiff::SignedDuration;
use pretty_assertions::assert_eq;

use super::{listing, names};
use crate::error::Exit;
use crate::run;
use crate::terminal::TermFacts;
use crate::testing::{TestEnv, bare, capture, command, design_options, models, now};

/// The listing in a pipe: no colour, no underline.
fn plain(list: &ModelsListResult, signed_out: &[String]) -> String {
    listing(list, signed_out, now(), &RenderOptions::new(100).with_terminal(false))
}

/// A model of the catalog of `provider` with a window of 200k and the efforts `efforts`,
/// whose default effort is the last one.
fn claude(id: &str, provider: &str, efforts: &[&str]) -> ModelInfo {
    ModelInfo {
        id: id.to_owned(),
        provider: Some(provider.to_owned()),
        efforts: efforts.iter().map(|effort| (*effort).to_owned()).collect(),
        default_effort: efforts.last().map(|effort| (*effort).to_owned()),
        default: false,
        source: ModelSource::Builtin,
        context_window: Some(200_000),
        max_context_window: None,
        prefer_websockets: false,
    }
}

/// The list of TM-Commands frame 2: the work account of `anthropic-api` with three
/// models, an account `personal` that logged out with two, and the ChatGPT account `b`
/// with one.
fn accounts() -> ModelsListResult {
    const EFFORTS: [&str; 3] = ["low", "medium", "high"];
    let catalog = |provider: &str, origin, minutes| CatalogStatus {
        provider: Some(provider.to_owned()),
        origin,
        fetched_at: Some(now() - SignedDuration::from_mins(minutes)),
    };
    let work = "anthropic-api.work";
    let personal = "anthropic-api.personal";
    ModelsListResult {
        models: vec![
            ModelInfo { default: true, ..claude("claude-opus-5-5", work, &EFFORTS) },
            claude("claude-sonnet-5", work, &EFFORTS),
            claude("claude-haiku-4-5", work, &[]),
            claude("claude-opus-5-5", personal, &EFFORTS),
            claude("claude-sonnet-5", personal, &EFFORTS),
            ModelInfo {
                id: "gpt-5.5".to_owned(),
                efforts: ["minimal", "low", "medium", "high", "xhigh"].map(str::to_owned).into(),
                default_effort: Some("medium".to_owned()),
                context_window: Some(272_000),
                max_context_window: Some(872_000),
                ..claude("", "openai-subscription.b", &[])
            },
        ],
        catalog: Some(catalog(work, CatalogOrigin::Backend, 5)),
        catalogs: vec![
            catalog(work, CatalogOrigin::Backend, 5),
            catalog("openai-subscription.b", CatalogOrigin::Cache, 125),
        ],
    }
}

fn personal() -> Vec<String> {
    vec!["anthropic-api.personal".to_owned()]
}

/// TM-Commands frame 2, rows 1 to 13 (row 0 and row 14 are the user's prompt), in the
/// three depths. The underline of the default effort stays without colour.
#[test]
fn efr_models_is_the_design() {
    let frame = Frame::load("TM-Commands", 2).unwrap();
    for depth in Depth::ALL {
        let options = design_options(depth, frame.cols);
        let out = listing(&accounts(), &personal(), now(), &options);
        assert_rows(&frame, 1..=13, &out, depth);
    }
}

#[test]
fn a_pipe_gets_the_same_rows_without_colour() {
    let out = plain(&accounts(), &personal());
    let terminal = listing(&accounts(), &personal(), now(), &design_options(Depth::Ansi16, 100));
    assert_eq!(out, bare(&terminal));
    assert!(!out.contains('\x1b'));
}

#[test]
fn the_listing_marks_the_default_and_names_the_efforts() {
    insta::assert_snapshot!(plain(&models(), &[]), @"
    openai · openai-subscription · from the backend, fetched 5m ago
    * gpt-5.5            272k (up to 872k)  efforts low medium high xhigh
      gpt-5.4            128k               efforts low medium high
      my-model           window unknown     efforts unknown · from [openai] models

    * the default · underlined: the default effort · ,model openai/gpt-5.4 picks one
    ");
}

#[test]
fn names_are_the_ids_alone() {
    assert_eq!(names(&models()), "gpt-5.5\ngpt-5.4\nmy-model\n");
}

#[test]
fn the_models_of_each_provider_with_a_login_are_listed_under_their_provider() {
    let status = |provider: &str| CatalogStatus {
        provider: Some(provider.to_owned()),
        origin: CatalogOrigin::Builtin,
        fetched_at: None,
    };
    let mut list = models();
    list.catalog = Some(status("openai-subscription"));
    list.catalogs = vec![status("openai-subscription"), status("openai-api")];
    list.models[0].provider = Some("openai-subscription".to_owned());
    list.models[1].provider = Some("openai-subscription".to_owned());
    list.models[2].provider = Some("openai-api".to_owned());
    let mut again = list.models[0].clone();
    again.provider = Some("openai-api".to_owned());
    again.default = false;
    list.models.push(again);

    insta::assert_snapshot!(plain(&list, &[]), @"
    openai · openai-subscription · built into efr
    * gpt-5.5            272k (up to 872k)  efforts low medium high xhigh
      gpt-5.4            128k               efforts low medium high

    openai-api · openai-api · built into efr
      my-model           window unknown     efforts unknown · from [openai] models
      gpt-5.5            272k (up to 872k)  efforts low medium high xhigh

    * the default · underlined: the default effort · ,model openai/gpt-5.4 picks one
    ");
    // The completion offers each id once.
    assert_eq!(names(&list), "gpt-5.5\ngpt-5.4\nmy-model\n");
}

#[test]
fn an_account_that_logged_out_keeps_its_head() {
    let mut list = accounts();
    list.models.retain(|model| model.provider.as_deref() != Some("anthropic-api.personal"));
    let out = plain(&list, &personal());
    assert!(
        out.ends_with(
            "personal · anthropic-api · signed out\n\n* the default · underlined: the default \
             effort · ,model work/claude-sonnet-5 picks one\n"
        ),
        "{out}"
    );
}

#[test]
fn text_from_the_daemon_cannot_drive_the_terminal() {
    let mut list = models();
    list.models[0].id = "evil\x1b]52;c;aGk=\x07".to_owned();
    list.models[0].efforts = vec!["\x1b[2J".to_owned()];
    let signed_out = ["openai-subscription.b\x1b[2J".to_owned()];
    let options = design_options(Depth::Ansi16, 100);
    let out = bare(&listing(&list, &signed_out, now(), &options));
    assert!(!out.contains('\x1b'), "{out}");
    assert!(!names(&list).contains('\x1b'));
}

#[test]
fn the_listing_says_where_an_older_list_came_from() {
    let mut list = models();
    list.catalog = Some(CatalogStatus {
        provider: None,
        origin: CatalogOrigin::Cache,
        fetched_at: Some(now() - SignedDuration::from_hours(26)),
    });
    let text = plain(&list, &[]);
    assert!(text.starts_with("from the cache, fetched 1d 2h ago\n* gpt-5.5 "), "{text}");
    list.catalog = None;
    assert!(plain(&list, &[]).starts_with("* gpt-5.5 "), "an older daemon does not say");
}

#[test]
fn a_claude_list_names_its_provider_and_the_anthropic_table_of_the_config() {
    let mut list = models();
    list.models.retain(|model| model.id == "my-model");
    list.models[0].id = "claude-next".to_owned();
    list.catalog = Some(CatalogStatus {
        provider: Some("anthropic-api".to_owned()),
        origin: CatalogOrigin::Missing,
        fetched_at: None,
    });

    insta::assert_snapshot!(plain(&list, &[]), @"
    anthropic · anthropic-api · no list yet; efrd fetches it in the background
      claude-next        window unknown  efforts unknown · from [anthropic] models

    * the default · underlined: the default effort · ,model anthropic/claude-next picks one
    ");
}

#[tokio::test]
async fn efr_models_prints_the_daemons_list() {
    let env = TestEnv::new();
    let daemon = env.listen();
    let ctx = env.context();
    let (mut out, captured) = capture();
    let script = async {
        let mut conn = daemon.accept().await;
        assert_eq!(conn.hello().origin, Origin::Cli);
        conn.answer_models(&models()).await;
        conn.until_closed().await;
    };
    let line = command(&["models", "--names"]);
    let (exit, ()) = tokio::join!(run::run(&line, &ctx, &mut out), script);
    assert_eq!(exit, Exit::Success, "{}", captured.stderr());
    assert_eq!(captured.stdout(), "gpt-5.5\ngpt-5.4\nmy-model\n");
}

/// A daemon's status with one account, `personal`, that logged out.
fn signed_out_status() -> AdminStatusResult {
    AdminStatusResult {
        daemon_id: "019a9b1c-3d00-7a10-8b20-000000000007".parse().unwrap(),
        version: "0.1.0".to_owned(),
        protocol: efr_protocol::PROTOCOL_VERSION,
        pid: 777,
        started_at: now(),
        screen_backend: "vt100".to_owned(),
        conversations: 0,
        shells: 0,
        providers: vec![ProviderStatus {
            provider: "anthropic-api.personal".to_owned(),
            logged_in: false,
            expires_at: None,
            active: false,
            login: None,
            key_hint: None,
            key_refused_at: None,
            key_checked_at: None,
        }],
        catalog: None,
        roots: None,
        config: None,
        sandbox: None,
        sandbox_paths: None,
        full_mode: None,
    }
}

#[tokio::test]
async fn efr_models_asks_which_accounts_logged_out() {
    let env = TestEnv::new();
    let daemon = env.listen();
    let term = TermFacts { utf8: true, ..TermFacts::default() };
    let ctx = crate::context::Context { term, ..env.context() };
    let (mut out, captured) = capture();
    let mut list = accounts();
    list.models.retain(|model| model.provider.as_deref() != Some("anthropic-api.personal"));
    let script = async {
        let mut conn = daemon.accept().await;
        conn.answer_models(&list).await;
        let (id, method) = conn.request().await;
        assert!(matches!(method, Method::AdminStatus(_)), "{}", method.name());
        conn.reply(id, &signed_out_status()).await;
        conn.until_closed().await;
    };
    let line = command(&["models"]);
    let (exit, ()) = tokio::join!(run::run(&line, &ctx, &mut out), script);
    assert_eq!(exit, Exit::Success, "{}", captured.stderr());
    assert!(captured.stdout().contains("\npersonal · anthropic-api · signed out\n"));
}

#[tokio::test]
async fn without_a_daemon_models_exits_with_three() {
    let env = TestEnv::new();
    let (mut out, captured) = capture();
    let exit = run::run(&command(&["models"]), &env.context(), &mut out).await;
    assert_eq!(exit, Exit::NotRunning);
    assert!(captured.stderr().contains("systemctl --user start efrd"), "{}", captured.stderr());
}

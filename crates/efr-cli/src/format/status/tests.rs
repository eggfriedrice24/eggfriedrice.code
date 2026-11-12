use std::path::{Path, PathBuf};

use efr_protocol::{
    AdminStatusResult, CatalogOrigin, CatalogStatus, ConfigStatus, ConversationStatus,
    ConversationSummary, ConversationsListResult, FullModeStatus, LoginKind, Mode, NetworkMode,
    PageCursor, ProviderStatus, SandboxStatus, Seq,
};
use efr_render::{Glyphs, RenderOptions};
use efr_test_support::design::{Depth, Frame, assert_rows};
use jiff::{SignedDuration, Timestamp};
use pretty_assertions::assert_eq;

use super::{StatusView, account_of, conversations, origin, short_span, status};
use crate::testing::{bare, conversation, design_fixed, design_options, now};

fn before(seconds: i64) -> Timestamp {
    now() - SignedDuration::from_secs(seconds)
}

fn after(seconds: i64) -> Timestamp {
    now() + SignedDuration::from_secs(seconds)
}

/// A provider with no login.
fn provider(id: &str) -> ProviderStatus {
    ProviderStatus {
        provider: id.to_owned(),
        logged_in: false,
        expires_at: None,
        active: false,
        login: None,
        key_hint: None,
        key_refused_at: None,
        key_checked_at: None,
    }
}

/// The work account of `anthropic-api`, the account of new conversations, logged in
/// with a key that a request used.
fn work() -> ProviderStatus {
    ProviderStatus {
        logged_in: true,
        active: true,
        login: Some(LoginKind::ApiKey),
        key_hint: Some("sk-ant-...a1b2".to_owned()),
        key_checked_at: Some(before(60)),
        ..provider("anthropic-api.work")
    }
}

/// A ready sandbox with efr's network proxy, the network view and the bus filter.
fn sandbox() -> SandboxStatus {
    SandboxStatus {
        available: true,
        reason: None,
        landlock_abi: Some(10),
        bwrap: Some(PathBuf::from("/usr/bin/bwrap")),
        bwrap_version: Some("0.13.0".to_owned()),
        network_mode: NetworkMode::Proxy,
        proxy: true,
        netview: true,
        bus_proxy: true,
        ..SandboxStatus::unavailable("")
    }
}

/// The daemon of TM-Commands frame 1: up 3d 4h, four accounts, a ready sandbox.
fn status_result() -> AdminStatusResult {
    AdminStatusResult {
        daemon_id: "019a9b1c-3d00-7a10-8b20-000000000007".parse().unwrap(),
        version: "0.0.5".to_owned(),
        protocol: 1,
        pid: 4242,
        started_at: before((3 * 24 + 4) * 3_600),
        screen_backend: "vt100".to_owned(),
        conversations: 2,
        shells: 1,
        providers: vec![
            provider("openai-subscription"),
            work(),
            provider("anthropic-api.personal"),
            ProviderStatus {
                logged_in: true,
                expires_at: Some(after((6 * 24 + 6) * 3_600)),
                login: Some(LoginKind::Subscription),
                ..provider("openai-subscription.b")
            },
        ],
        catalog: Some(catalog("anthropic-api.work", CatalogOrigin::Backend, 5 * 60)),
        roots: None,
        config: Some(ConfigStatus {
            path: PathBuf::from("/home/u/.config/efr/config.toml"),
            exists: true,
            symlink_target: None,
            reload_error: None,
            restart_needed: Vec::new(),
            held: None,
        }),
        sandbox: Some(sandbox()),
        sandbox_paths: None,
        full_mode: Some(FullModeStatus { allowed: true, refused: None }),
    }
}

fn catalog(provider: &str, origin: CatalogOrigin, seconds: i64) -> CatalogStatus {
    CatalogStatus { provider: Some(provider.to_owned()), origin, fetched_at: Some(before(seconds)) }
}

/// The model lists of the two accounts with a login.
fn catalogs() -> Vec<CatalogStatus> {
    vec![
        catalog("anthropic-api.work", CatalogOrigin::Backend, 5 * 60),
        catalog("openai-subscription.b", CatalogOrigin::Cache, (2 * 60 + 5) * 60),
    ]
}

fn shown(
    result: &AdminStatusResult,
    catalogs: &[CatalogStatus],
    options: &RenderOptions,
) -> String {
    let view = StatusView {
        status: result,
        catalogs,
        socket: Path::new("/run/user/1000/efr/efrd.sock"),
        now: now(),
        file_mode: None,
        home: Some(Path::new("/home/u")),
    };
    status(&view, options)
}

/// TM-Commands frame 1, rows 1 to 13 (row 0 and row 14 are the user's prompt), in the
/// three depths. The board draws the frame in truecolor; the other depths come from the
/// table of the design (`efr_test_support::design::expect`).
#[test]
fn efr_status_is_the_design() {
    let frame = Frame::load("TM-Commands", 1).unwrap();
    for depth in Depth::ALL {
        let options = design_options(depth, frame.cols);
        let fixed = design_fixed(
            &frame,
            depth,
            &[
                // NOTE: the board starts the account rows after the first two cells to
                // the right of the first one; its own note says "Columns align by
                // spaces", so the rows start in the column of `work`.
                (5, "              personal", "            personal"),
                (6, "              b", "            b"),
                (
                    7,
                    "              api       openai-api         ",
                    "            openai    openai-subscription",
                ),
                // NOTE: the board names the openai-api account `api`, a name that
                // `,model` does not take; its note says that accounts are named as
                // `,model` takes them. The test has the default openai account there,
                // whose name `openai` takes the same column.
                // NOTE: the same row says "fetched" for one list and not for the other;
                // TM-Commands frame 2 writes the same fact as "from the cache, fetched
                // 2h 5m ago", so one fact keeps one wording.
                (8, "from the cache, 2h 5m ago", "from the cache, fetched 2h 5m ago"),
            ],
        );
        let out = shown(&status_result(), &catalogs(), &options);
        assert_rows(&fixed, 1..=13, &out, depth);
    }
}

#[test]
fn a_pipe_gets_the_same_rows_without_colour() {
    let options = RenderOptions::new(100).with_terminal(false);
    let out = shown(&status_result(), &catalogs(), &options);
    assert!(!out.contains('\x1b'), "{out}");
    let terminal = shown(&status_result(), &catalogs(), &design_options(Depth::Ansi16, 100));
    assert_eq!(out, bare(&terminal));
    insta::assert_snapshot!(out, @"
    efrd        0.0.5 · protocol 1 · up 3d 4h · /run/user/1000/efr/efrd.sock
    config      ~/.config/efr/config.toml

    accounts    work      anthropic-api        logged in · key sk-ant-…a1b2 · active
                personal  anthropic-api        signed out · efr login anthropic-api --account personal
                b         openai-subscription  logged in · token expires in 6d 6h
                openai    openai-subscription  not logged in
    models      work: from the backend, fetched 5m ago · b: from the cache, fetched 2h 5m ago

    sandbox     ready · Landlock ABI 10 · bubblewrap 0.13.0 · network proxy
    netview     yes
    bus filter  on
    full mode   allowed
    ");
}

#[test]
fn an_ascii_locale_gets_the_ascii_marks() {
    let options = RenderOptions::new(100).with_terminal(false).with_glyphs(&Glyphs::ASCII);
    let out = shown(&status_result(), &catalogs(), &options);
    assert!(out.is_ascii(), "{out}");
    assert!(out.starts_with("efrd        0.0.5 - protocol 1 - up 3d 4h - /run/"), "{out}");
    assert!(out.contains("logged in - key sk-ant-...a1b2 - active\n"), "{out}");
}

#[test]
fn an_older_daemon_shows_its_one_model_list() {
    let options = RenderOptions::new(100).with_terminal(false);
    let out = shown(&status_result(), &[], &options);
    assert!(out.contains("\nmodels      work: from the backend, fetched 5m ago\n"), "{out}");
    let older = AdminStatusResult {
        catalog: Some(CatalogStatus { provider: None, ..catalogs()[0].clone() }),
        sandbox: None,
        full_mode: None,
        config: None,
        ..status_result()
    };
    let out = shown(&older, &[], &options);
    assert!(out.contains("\nmodels      from the backend, fetched 5m ago\n\n"), "{out}");
    assert!(!out.contains("sandbox"), "{out}");
}

#[test]
fn a_key_says_when_it_was_refused_or_that_efrd_did_not_check_it() {
    let options = RenderOptions::new(100).with_terminal(false);
    let line = |provider: ProviderStatus| {
        let result = AdminStatusResult { providers: vec![provider], ..status_result() };
        let out = shown(&result, &[], &options);
        out.lines().find(|line| line.starts_with("accounts")).unwrap().to_owned()
    };
    let refused = ProviderStatus { key_refused_at: Some(before(3 * 60)), ..work() };
    assert_eq!(
        line(refused.clone()),
        "accounts    work  anthropic-api  logged in · key sk-ant-…a1b2 · refused 3m ago · active"
    );
    let hidden = ProviderStatus { key_hint: None, ..refused };
    assert_eq!(
        line(hidden),
        "accounts    work  anthropic-api  logged in · key refused 3m ago · active"
    );
    let unchecked = ProviderStatus { key_checked_at: None, ..work() };
    assert_eq!(
        line(unchecked.clone()),
        "accounts    work  anthropic-api  logged in · key sk-ant-…a1b2 · not checked since efrd \
         started · active"
    );
    let unchecked = ProviderStatus { key_hint: None, ..unchecked };
    assert_eq!(
        line(unchecked),
        "accounts    work  anthropic-api  logged in · key not checked since efrd started · active"
    );
    let expired = ProviderStatus {
        login: Some(LoginKind::Subscription),
        key_hint: None,
        expires_at: Some(before(90)),
        active: false,
        ..provider("openai-subscription")
    };
    let expired = ProviderStatus { logged_in: true, ..expired };
    assert_eq!(
        line(expired),
        "accounts    openai  openai-subscription  logged in · token expired 1m 30s ago"
    );
}

#[test]
fn the_rows_that_need_care_take_their_colour() {
    let options = design_options(Depth::Ansi16, 100);
    let refused = efr_protocol::FullRefusal::Root;
    let result = AdminStatusResult {
        providers: Vec::new(),
        config: Some(ConfigStatus {
            exists: false,
            restart_needed: vec!["model.provider".to_owned()],
            ..status_result().config.unwrap()
        }),
        sandbox: Some(SandboxStatus {
            fix: Some("install bubblewrap".to_owned()),
            ..SandboxStatus::unavailable("bubblewrap is not installed")
        }),
        full_mode: Some(FullModeStatus { allowed: true, refused: Some(refused) }),
        ..status_result()
    };
    let view = StatusView {
        status: &result,
        catalogs: &[],
        socket: Path::new("/s"),
        now: now(),
        file_mode: Some(Mode::Full),
        home: Some(Path::new("/home/u")),
    };
    let out = status(&view, &options);
    insta::assert_snapshot!(crate::testing::readable(&out), @r"
    \e[2mefrd\e[0m            0.0.5 · protocol 1 · up 3d 4h · /s
    \e[2mconfig\e[0m          ~/.config/efr/config.toml\e[2m (absent)\e[0m
    \e[2mrestart needed\e[0m  \e[33mmodel.provider (systemctl --user restart efrd)\e[0m

    \e[2maccounts\e[0m        \e[2mnone configured\e[0m
    \e[2mmodels\e[0m          work: from the backend, fetched 5m ago

    \e[2msandbox\e[0m         \e[33munavailable\e[0m · bubblewrap is not installed · auto runs as cautious
    \e[2msandbox fix\e[0m     install bubblewrap
    \e[2mfull mode\e[0m       \e[33mrefused (efrd runs as root)\e[0m
    \e[2mdefault mode\e[0m    \e[31mfull\e[0m · efrd cannot run it, so a turn that names no mode runs as auto
    ");
}

#[test]
fn each_account_has_the_name_that_model_takes() {
    let named = |id: &str| account_of(id);
    assert_eq!(named("anthropic-api.work"), ("work".to_owned(), "anthropic-api".to_owned()));
    assert_eq!(named("anthropic-api"), ("anthropic".to_owned(), "anthropic-api".to_owned()));
    assert_eq!(
        named("openai-subscription"),
        ("openai".to_owned(), "openai-subscription".to_owned())
    );
    assert_eq!(named("openai-api"), ("openai-api".to_owned(), "openai-api".to_owned()));
}

#[test]
fn a_time_leaves_out_a_second_unit_of_zero() {
    assert_eq!(short_span(45), "45s");
    assert_eq!(short_span(300), "5m");
    assert_eq!(short_span(303), "5m 3s");
    assert_eq!(short_span(3_600), "1h");
    assert_eq!(short_span(2 * 3_600 + 300), "2h 5m");
    assert_eq!(short_span(3 * 86_400), "3d");
    assert_eq!(short_span(3 * 86_400 + 4 * 3_600), "3d 4h");
}

#[test]
fn a_list_says_where_it_came_from_and_when() {
    let status = |origin, fetched_at| CatalogStatus { provider: None, origin, fetched_at };
    let backend = status(CatalogOrigin::Backend, Some(before(5 * 60)));
    assert_eq!(origin(&backend, now()), "from the backend, fetched 5m ago");
    let cache = status(CatalogOrigin::Cache, Some(before(26 * 3_600)));
    assert_eq!(origin(&cache, now()), "from the cache, fetched 1d 2h ago");
    assert_eq!(origin(&status(CatalogOrigin::Builtin, None), now()), "built into efr");
    assert_eq!(
        origin(&status(CatalogOrigin::Missing, None), now()),
        "no list yet; efrd fetches it in the background"
    );
}

#[test]
fn status_text_from_the_daemon_cannot_drive_the_terminal() {
    let result = AdminStatusResult {
        version: "0.1\u{1b}[2J".to_owned(),
        providers: vec![provider("openai-subscription.b\u{1b}]52;c;aGk=\u{7}")],
        ..status_result()
    };
    let out = shown(&result, &[], &RenderOptions::new(100).with_terminal(false));
    assert!(!out.contains('\u{1b}'), "{out}");
}

#[test]
fn conversations_list_newest_first_with_status_and_age() {
    let summary = |n: u64, status, title: Option<&str>, age| ConversationSummary {
        id: format!("019a9b1c-3d00-7a10-8b20-00000000000{n}").parse().unwrap(),
        title: title.map(str::to_owned),
        status,
        created_at: before(age + 600),
        updated_at: before(age),
        last_seq: Seq::new(n * 10),
        cwd: None,
        scope: None,
        tty: None,
    };
    let list = ConversationsListResult {
        conversations: vec![
            summary(1, ConversationStatus::Running, Some("fix the nginx config"), 30),
            summary(2, ConversationStatus::AwaitingApproval, Some("tidy\n~/.zshrc"), 3_700),
            summary(3, ConversationStatus::Idle, Some("rotate the logs"), 758),
            summary(4, ConversationStatus::Idle, None, 200_000),
        ],
        next_cursor: Some(PageCursor::new("c2")),
    };
    insta::assert_snapshot!(conversations(&list, now()));
    assert_eq!(list.conversations[0].id, conversation());
}

#[test]
fn an_empty_list_says_so() {
    assert_eq!(conversations(&ConversationsListResult::default(), now()), "no conversations yet\n");
}

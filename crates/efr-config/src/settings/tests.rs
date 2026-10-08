use std::path::{Path, PathBuf};

use efr_permissions::{Action, CommandPattern, Effect, PermissionsError, Policy, Resource, Rule};
use efr_protocol::Mode;
use efr_stdx::env::Var;
use pretty_assertions::assert_eq;

use crate::{
    CompactionSettings, ConfigError, Location, ModelEntry, Progress, ScreenChoice, Settings,
    Source, SudoCache, WebSocketChoice,
};

const PATH: &str = "/home/u/.config/efr/config.toml";

fn path() -> &'static Path {
    Path::new(PATH)
}

fn parse(text: &str) -> Result<Settings, ConfigError> {
    Settings::parse(path(), Some(text))
}

#[test]
fn without_a_file_every_value_is_the_default() {
    let settings = Settings::parse(path(), None).unwrap();

    assert_eq!(settings.log, "info");
    assert_eq!(settings.screen, ScreenChoice::Auto);
    assert_eq!(settings.model.provider, "openai-subscription");
    assert_eq!(settings.model.name, None);
    assert_eq!(settings.model.effort, None);
    assert_eq!(settings.permissions.mode, Mode::Cautious);
    assert!(settings.shell.login);
    assert_eq!(settings.shell.idle_minutes, 60);
    assert_eq!(settings.shell.sudo_cache, SudoCache::Keep);
    assert_eq!(settings.shell.interactive_timeout_minutes, 60);
    assert_eq!(settings.conversation.max_queued, 16);
    assert_eq!(settings.conversation.update_interval_ms, 200, "drafts keep the log at 200 ms");
    assert_eq!(settings.conversation.draft_interval_ms, 16);
    assert_eq!(settings.source("log"), Source::Default);
    assert_eq!(settings.path, PathBuf::from(PATH));
    assert_eq!(settings, Settings { path: PathBuf::from(PATH), ..Settings::default() });
}

#[test]
fn an_empty_file_is_the_defaults_too() {
    assert_eq!(parse("").unwrap(), Settings::parse(path(), None).unwrap());
}

#[test]
fn the_file_sets_what_it_names() {
    let text = r#"
        log = "warn"
        screen = "vt100"

        [model]
        provider = "openai-api"
        name = "gpt-6-sol"
        effort = "high"
        max_output_tokens = 4096

        [openai]
        originator = "efr-dev"
        models = ["gpt-6-sol", "gpt-5.5"]
        websocket = "off"

        [permissions]
        mode = "auto"
        secret_paths = ["~/.config/rclone/rclone.conf", "/srv/vault"]

        [shell]
        program = "/usr/bin/zsh"
        login = false
        idle_minutes = 0
        sudo_cache = "per_call"
        interactive_timeout_minutes = 15

        [conversation]
        max_queued = 4
        approval_timeout_secs = 600
        update_interval_ms = 50
        draft_interval_ms = 8
        tty_idle_hours = 0

        [render]
        theme = "ansi"
    "#;

    let settings = parse(text).unwrap();

    assert_eq!(settings.log, "warn");
    assert_eq!(settings.screen, ScreenChoice::Vt100);
    assert_eq!(settings.model.provider, "openai-api");
    assert_eq!(settings.model.name.as_deref(), Some("gpt-6-sol"));
    assert_eq!(settings.model.effort.as_deref(), Some("high"));
    assert_eq!(settings.model.max_output_tokens, Some(4096));
    assert_eq!(settings.openai.originator, "efr-dev");
    assert_eq!(settings.openai.websocket, WebSocketChoice::Off);
    assert_eq!(
        settings.openai.models.as_deref(),
        Some(&["gpt-6-sol".into(), "gpt-5.5".into()][..])
    );
    assert_eq!(settings.permissions.mode, Mode::Auto);
    assert_eq!(
        settings.permissions.secret_paths,
        [PathBuf::from("~/.config/rclone/rclone.conf"), PathBuf::from("/srv/vault")]
    );
    assert_eq!(settings.shell.program.as_deref(), Some(Path::new("/usr/bin/zsh")));
    assert!(!settings.shell.login);
    assert_eq!(settings.shell.idle_minutes, 0);
    assert_eq!(settings.shell.sudo_cache, SudoCache::PerCall);
    assert_eq!(settings.shell.interactive_timeout_minutes, 15);
    assert_eq!(settings.conversation.max_queued, 4);
    assert_eq!(settings.conversation.approval_timeout_secs, Some(600));
    assert_eq!(settings.conversation.update_interval_ms, 50);
    assert_eq!(settings.conversation.draft_interval_ms, 8);
    assert_eq!(settings.conversation.tty_idle_hours, 0);
    assert_eq!(settings.render.theme.as_deref(), Some("ansi"));
    assert_eq!(settings.source("model.name"), Source::File);
    assert_eq!(settings.source("permissions.mode"), Source::File);
    assert_eq!(settings.source("model.system_prompt"), Source::Default);
}

#[test]
fn an_unknown_key_is_an_error_with_its_place_and_key() {
    let error = parse("[shell]\nlogin = true\nidle_minuets = 5\n").unwrap_err();

    match &error {
        ConfigError::Parse { path: at, location, key, source } => {
            assert_eq!(at, &PathBuf::from(PATH));
            assert_eq!(*location, Some(Location { line: 3, column: 1 }));
            assert_eq!(key.as_deref(), Some("shell.idle_minuets"));
            assert!(source.message().contains("idle_minuets"), "{source}");
        }
        other => panic!("{other:?}"),
    }
    let reported = error.file_error();
    assert_eq!(reported.line, Some(3));
    assert_eq!(reported.column, Some(1));
    assert_eq!(reported.key.as_deref(), Some("shell.idle_minuets"));
    assert!(reported.message.starts_with(&format!("the config file {PATH} is not valid: ")));
    assert!(!reported.message.contains('\n'), "{}", reported.message);
}

#[test]
fn an_unknown_table_and_the_removed_reasoning_effort_are_errors() {
    for (text, key) in [
        ("[mcp]\nservers = []\n", "mcp"),
        ("[openai]\nreasoning_effort = \"high\"\n", "openai.reasoning_effort"),
        ("[render]\ntheme = \"nord\"\ncolour = \"16\"\n", "render.colour"),
    ] {
        let error = parse(text).unwrap_err();
        assert!(matches!(error, ConfigError::Parse { .. }), "{text}: {error:?}");
        assert_eq!(error.key().as_deref(), Some(key), "{text}");
    }
}

#[test]
fn a_value_of_the_wrong_type_or_name_is_an_error_with_its_key() {
    for (text, key) in [
        ("screen = \"kitty\"\n", "screen"),
        ("[permissions]\nmode = \"yolo\"\n", "permissions.mode"),
        ("[shell]\nsudo_cache = \"never\"\n", "shell.sudo_cache"),
        ("[shell]\nidle_minutes = \"soon\"\n", "shell.idle_minutes"),
        ("[conversation]\nmax_queued = -1\n", "conversation.max_queued"),
        ("[model]\nmax_output_tokens = 5000000000\n", "model.max_output_tokens"),
    ] {
        let error = parse(text).unwrap_err();
        assert!(matches!(error, ConfigError::Parse { .. }), "{text}: {error:?}");
        assert_eq!(error.key().as_deref(), Some(key), "{text}");
        assert!(error.location().is_some(), "{text}");
    }
}

#[test]
fn text_that_is_not_toml_is_an_error_with_its_place() {
    let error = parse("log = \"info\"\n[shell\n").unwrap_err();

    assert!(matches!(error, ConfigError::Parse { .. }), "{error:?}");
    assert_eq!(error.location().map(|at| at.line), Some(2));
}

#[test]
fn values_outside_their_set_or_range_are_refused_with_the_key_and_place() {
    let cases = [
        ("log = \" \"\n", "log"),
        ("[model]\nprovider = \"anthropic\"\n", "model.provider"),
        ("[model]\nname = \"\"\n", "model.name"),
        ("[model]\neffort = \"Very High\"\n", "model.effort"),
        ("[model]\nmax_output_tokens = 0\n", "model.max_output_tokens"),
        ("[openai]\noriginator = \"\"\n", "openai.originator"),
        ("[openai]\nmodels = [\"gpt-5.5\", \" \"]\n", "openai.models"),
        ("[openai]\nsubscription_base_url = \"chatgpt.com\"\n", "openai.subscription_base_url"),
        ("[openai]\napi_base_url = \"ftp://x\"\n", "openai.api_base_url"),
        ("[permissions]\nsecret_paths = [\"vault\"]\n", "permissions.secret_paths"),
        ("[shell]\nprogram = \"zsh\"\n", "shell.program"),
        ("[shell]\nidle_minutes = 600000\n", "shell.idle_minutes"),
        ("[conversation]\nmax_queued = 0\n", "conversation.max_queued"),
        ("[conversation]\napproval_timeout_secs = 0\n", "conversation.approval_timeout_secs"),
        ("[conversation]\nupdate_interval_ms = 60001\n", "conversation.update_interval_ms"),
        ("[conversation]\ndraft_interval_ms = 1001\n", "conversation.draft_interval_ms"),
        ("[conversation]\ntty_idle_hours = 9000\n", "conversation.tty_idle_hours"),
        ("[compaction]\nauto_at = 0\n", "compaction.auto_at"),
        ("[compaction]\nauto_at = 100\n", "compaction.auto_at"),
        ("[openai]\nmodels = [{ id = \"m\", context_window = 999 }]\n", "openai.models"),
        ("[openai]\nmodels = [{ id = \"m\", max_output_tokens = 0 }]\n", "openai.models"),
        (
            "[openai]\nmodels = [{ id = \"m\", context_window = 8000, max_output_tokens = 8000 }]\n",
            "openai.models",
        ),
        ("[openai]\nmodels = [{ id = \" \" }]\n", "openai.models"),
        ("[render]\ntheme = \"\"\n", "render.theme"),
        ("[render]\npalette = \"themes/efr.toml\"\n", "render.palette"),
        ("[render.colors]\naccent = \"gold\"\n", "render.colors.accent"),
        ("[render.colors]\nmuted = 16\n", "render.colors.muted"),
        ("[render.colors]\nlink = \"#12345\"\n", "render.colors.link"),
        ("[render.colors]\ndiff.add = \"bright-teal\"\n", "render.colors.diff.add"),
        ("[render.colors.diff]\nhunk = -2\n", "render.colors.diff.hunk"),
        ("[render]\ncolors = { quote = \"none\" }\n", "render.colors.quote"),
    ];
    for (text, key) in cases {
        let error = parse(text).unwrap_err();
        assert!(
            matches!(&error, ConfigError::Invalid { key: found, .. } if *found == key),
            "{text}: {error:?}"
        );
        assert_eq!(error.key().as_deref(), Some(key));
        let line = text.lines().count();
        assert_eq!(error.location().map(|at| at.line), Some(u32::try_from(line).unwrap()));
    }
}

#[test]
fn colours_of_every_form_are_read_and_their_keys_come_from_the_file() {
    let text = "[render]\npalette = \"~/theme.toml\"\n\n[render.colors]\ntext = \"#e8e2d4\"\n\
                muted = 8\naccent = \"yellow\"\ndiff.add = \"bright-green\"\n\
                diff.hunk = \"6\"\n";
    let settings = parse(text).unwrap();
    let colors = settings.render.colors.colors();
    assert_eq!(colors.len(), 5);
    assert_eq!(settings.render.palette.as_deref(), Some(Path::new("~/theme.toml")));
    for key in [
        "render.palette",
        "render.colors.text",
        "render.colors.muted",
        "render.colors.accent",
        "render.colors.diff.add",
        "render.colors.diff.hunk",
    ] {
        assert_eq!(settings.source(key), Source::File, "{key}");
    }
    assert_eq!(settings.source("render.colors.code"), Source::Default);
    assert_eq!(settings.source("render.colors"), Source::Default);
}

#[test]
fn motion_the_turn_summary_and_the_progress_bar_are_on_or_auto_until_the_file_says_otherwise() {
    let settings = parse("").unwrap();
    assert!(settings.render.motion);
    assert!(settings.render.turn_summary);
    assert_eq!(settings.render.progress, Progress::Auto);
    let text = "[render]\nmotion = false\nturn_summary = false\nprogress = \"off\"\n";
    let settings = parse(text).unwrap();
    assert!(!settings.render.motion);
    assert!(!settings.render.turn_summary);
    assert_eq!(settings.render.progress, Progress::Off);
    assert_eq!(settings.source("render.progress"), Source::File);
    let error = parse("[render]\nprogress = \"sometimes\"\n").unwrap_err();
    assert_eq!(error.key().as_deref(), Some("render.progress"));
}

#[test]
fn the_input_row_of_a_turn_is_on_until_the_file_turns_it_off() {
    let settings = parse("").unwrap();
    assert!(settings.render.turn_input);
    assert_eq!(settings.source("render.turn_input"), Source::Default);
    let settings = parse("[render]\nturn_input = false\n").unwrap();
    assert!(!settings.render.turn_input);
    assert_eq!(settings.source("render.turn_input"), Source::File);
    let error = parse("[render]\nturn_input = \"yes\"\n").unwrap_err();
    assert_eq!(error.key().as_deref(), Some("render.turn_input"));
}

#[test]
fn auto_picks_from_a_dark_and_a_light_theme_that_have_defaults() {
    let settings = parse("").unwrap();
    assert_eq!(settings.render.theme, None);
    assert_eq!(settings.render.theme_dark, "catppuccin-mocha");
    assert_eq!(settings.render.theme_light, "catppuccin-latte");
    let text = "[render]\ntheme = \"auto\"\ntheme_dark = \"nord\"\ntheme_light = \"github\"\n";
    let settings = parse(text).unwrap();
    assert_eq!(settings.render.theme.as_deref(), Some(crate::AUTO_THEME));
    assert_eq!(settings.render.theme_dark, "nord");
    assert_eq!(settings.render.theme_light, "github");
    assert_eq!(settings.source("render.theme_light"), Source::File);
    let error = parse("[render]\ntheme_light = \"\"\n").unwrap_err();
    assert_eq!(error.key().as_deref(), Some("render.theme_light"));
}

#[test]
fn a_colour_can_be_set_by_an_override_and_is_checked_like_the_file() {
    let mut settings = parse("").unwrap();
    let from = Source::Flag("--test");
    settings.apply_override("render.colors.diff.add", "#00ff00", from.clone()).unwrap();
    assert_eq!(settings.source("render.colors.diff.add"), from);
    assert_eq!(settings.render.colors.colors(), [("diff.add", crate::RoleColor::Rgb(0, 255, 0))]);
    let error = settings.apply_override("render.colors.accent", "gold", from).unwrap_err();
    assert!(matches!(error, ConfigError::InvalidOverride { .. }), "{error:?}");
}

#[test]
fn the_message_of_a_value_out_of_range_says_what_is_allowed() {
    let error = parse("[conversation]\nmax_queued = 0\n").unwrap_err();

    assert_eq!(
        error.to_string(),
        format!("the config value conversation.max_queued = 0 in {PATH} is not between 1 and 1024")
    );
}

#[test]
fn the_edges_of_each_range_are_allowed() {
    let text = "[model]\nmax_output_tokens = 1\n[shell]\nidle_minutes = 0\n\
                [conversation]\nmax_queued = 1024\napproval_timeout_secs = 1\n\
                update_interval_ms = 0\ndraft_interval_ms = 1000\ntty_idle_hours = 8760\n";

    let settings = parse(text).unwrap();

    assert_eq!(settings.conversation.max_queued, 1024);
}

#[test]
fn permission_rules_come_from_the_file_in_order() {
    let text = r#"
        [[permissions.rules]]
        action = "execute"
        resource = { command = { program = "cargo", args = ["test"] } }
        effect = "allow"

        [[permissions.rules]]
        action = "execute"
        resource = { command = { program = "systemctl", args = ["restart", "nginx"] } }
        effect = "allow"

        [[permissions.rules]]
        action = "read"
        resource = { under = "~/.ssh/config" }
        effect = "allow"
    "#;

    let settings = parse(text).unwrap();

    let expected = Policy::new(vec![
        Rule::new(
            Action::Execute,
            Resource::Command(CommandPattern::new("cargo").with_args(["test"])),
            Effect::Allow,
        ),
        Rule::new(
            Action::Execute,
            Resource::Command(CommandPattern::new("systemctl").with_args(["restart", "nginx"])),
            Effect::Allow,
        ),
        Rule::new(Action::Read, Resource::Under("~/.ssh/config".into()), Effect::Allow),
    ])
    .unwrap();
    assert_eq!(settings.permissions.rules, expected);
    assert_eq!(settings.source("permissions.rules"), Source::File);
    let policy = settings.permissions.policy(Mode::Cautious);
    let defaults = Policy::defaults();
    assert_eq!(&policy.rules()[..defaults.rules().len()], defaults.rules());
    assert_eq!(&policy.rules()[defaults.rules().len()..], expected.rules());
}

#[test]
fn rules_may_also_be_one_inline_array() {
    let text =
        "[permissions]\nrules = [{ action = \"read\", resource = \"any\", effect = \"deny\" }]\n";

    let settings = parse(text).unwrap();

    assert_eq!(settings.permissions.rules.rules().len(), 1);
}

#[test]
fn without_permission_rules_the_policy_is_the_built_in_one() {
    let settings = Settings::parse(path(), None).unwrap();

    assert_eq!(settings.permissions.rules, Policy::empty());
    assert_eq!(settings.permissions.policy(Mode::Cautious), Policy::defaults());
    assert_eq!(settings.permissions.policy(Mode::Auto), Policy::base(Mode::Auto));
    assert_eq!(settings.permissions.policy(Mode::Manual), Policy::base(Mode::Manual));
}

#[test]
fn an_invalid_permission_rule_is_named_by_its_place() {
    let text = "\
[[permissions.rules]]
action = \"execute\"
resource = { command = { program = \"cargo\", args = [\"test\"] } }
effect = \"allow\"

[[permissions.rules]]
action = \"execute\"
resource = { class = \"system\" }
effect = \"allow\"
";

    let error = parse(text).unwrap_err();

    assert_eq!(error.to_string(), format!("permissions.rules[1] in {PATH} is invalid"));
    assert_eq!(error.key().as_deref(), Some("permissions.rules[1]"));
    assert_eq!(error.location().map(|at| at.line), Some(6));
    match error {
        ConfigError::InvalidRule { path: at, index, source, .. } => {
            assert_eq!(at, PathBuf::from(PATH));
            assert_eq!(index, 1);
            assert_eq!(
                source,
                PermissionsError::RuleNeverMatches { index: 1, action: Action::Execute }
            );
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_permission_rule_of_the_wrong_shape_is_named_by_its_place() {
    let text = r#"
        [[permissions.rules]]
        action = "read"
        resource = "any"
        effect = "allow"

        [[permissions.rules]]
        action = "read"
        resource = "any"
        effect = "allow"

        [[permissions.rules]]
        action = "execute"
        resource = { command = { program = "ls" } }
        effect = "allow"
        why = "typo"
    "#;

    let error = parse(text).unwrap_err();

    assert_eq!(error.to_string(), format!("permissions.rules[2] in {PATH} is not a rule"));
    match error {
        ConfigError::ParseRule { index, source, .. } => {
            assert_eq!(index, 2);
            assert!(source.to_string().contains("why"), "{source}");
        }
        other => panic!("{other:?}"),
    }
    let relative = "[[permissions.rules]]\naction = \"write\"\nresource = { under = \"p\" }\neffect = \"allow\"\n";
    let error = parse(relative).unwrap_err();
    assert!(matches!(error, ConfigError::InvalidRule { index: 0, .. }), "{error:?}");
}

#[test]
fn a_missing_file_loads_as_the_defaults() {
    let dir = tempfile::tempdir().unwrap();

    let settings = Settings::load(dir.path()).unwrap();

    assert_eq!(settings, Settings { path: dir.path().join("config.toml"), ..Settings::default() });
}

#[test]
fn load_reads_config_toml_in_the_config_root() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("config.toml"), "[shell]\nidle_minutes = 5\n").unwrap();

    let settings = Settings::load(dir.path()).unwrap();

    assert_eq!(settings.shell.idle_minutes, 5);
    assert_eq!(settings.path, dir.path().join("config.toml"));
}

#[test]
fn a_file_that_cannot_be_read_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("config.toml")).unwrap();

    let error = Settings::load(dir.path()).unwrap_err();

    assert!(matches!(error, ConfigError::Read { .. }), "{error:?}");
}

#[test]
fn an_override_replaces_the_value_and_names_its_source() {
    let mut settings = parse("log = \"warn\"\nscreen = \"ghostty\"\n").unwrap();

    settings.apply_override("log", "debug", Source::Env(Var::Log)).unwrap();
    settings.apply_override("screen", "vt100", Source::Flag("--screen")).unwrap();

    assert_eq!(settings.log, "debug");
    assert_eq!(settings.screen, ScreenChoice::Vt100);
    assert_eq!(settings.source("log"), Source::Env(Var::Log));
    assert_eq!(settings.source("screen"), Source::Flag("--screen"));
}

#[test]
fn an_override_keeps_the_rules_and_the_other_values() {
    let text = "[shell]\nidle_minutes = 5\n[[permissions.rules]]\naction = \"read\"\n\
                resource = \"any\"\neffect = \"deny\"\n";
    let mut settings = parse(text).unwrap();
    let before = settings.clone();

    settings.apply_override("conversation.max_queued", "3", Source::Flag("--max-queued")).unwrap();

    assert_eq!(settings.conversation.max_queued, 3);
    assert_eq!(settings.permissions.rules, before.permissions.rules);
    assert_eq!(settings.shell.idle_minutes, 5);
    assert_eq!(settings.source("shell.idle_minutes"), Source::File);
    assert_eq!(settings.path, before.path);
}

#[test]
fn an_override_of_each_kind_is_read_as_the_file_would_read_it() {
    let mut settings = Settings::default();
    let from = Source::Flag("--x");

    settings.apply_override("shell.login", "false", from.clone()).unwrap();
    settings.apply_override("openai.models", "gpt-5.5, gpt-5.4", from.clone()).unwrap();
    settings.apply_override("permissions.mode", "manual", from.clone()).unwrap();
    settings.apply_override("model.max_output_tokens", "100", from).unwrap();

    assert!(!settings.shell.login);
    assert_eq!(settings.openai.models, Some(vec!["gpt-5.5".into(), "gpt-5.4".into()]));
    assert_eq!(settings.permissions.mode, Mode::Manual);
    assert_eq!(settings.model.max_output_tokens, Some(100));
}

#[test]
fn a_bad_override_is_refused_and_changes_nothing() {
    let mut settings = Settings::default();

    let error = settings.apply_override("screen", "kitty", Source::Env(Var::Screen)).unwrap_err();
    assert_eq!(
        error.to_string(),
        "screen = \"kitty\" from env EFR_SCREEN is not auto, vt100 or ghostty"
    );
    let error =
        settings.apply_override("conversation.max_queued", "0", Source::Flag("--q")).unwrap_err();
    assert!(error.to_string().ends_with("is not between 1 and 1024"), "{error}");
    let error = settings.apply_override("shell.login", "maybe", Source::Flag("--l")).unwrap_err();
    assert!(error.to_string().ends_with("is not true or false"), "{error}");
    let error = settings.apply_override("shell.colour", "red", Source::Flag("--c")).unwrap_err();
    assert!(matches!(error, ConfigError::UnknownKey { .. }), "{error:?}");

    assert_eq!(settings, Settings::default());
}

#[test]
fn a_field_changed_in_code_keeps_the_source_it_had() {
    let mut settings = Settings::default();
    settings.shell.idle_minutes = 0;

    assert_eq!(settings.source("shell.idle_minutes"), Source::Default);
    settings.set_source("shell.idle_minutes", Source::Flag("--idle"));
    assert_eq!(settings.source("shell.idle_minutes"), Source::Flag("--idle"));
}

#[test]
fn an_unknown_default_model_is_named() {
    let settings = parse("[model]\nname = \"gpt-9\"\n").unwrap();

    assert_eq!(settings.unknown_model(&["gpt-5.5", "gpt-5.4"]), Some("gpt-9"));
    assert_eq!(settings.unknown_model(&["gpt-9"]), None);
    assert_eq!(Settings::default().unknown_model(&[]), None);
}

#[test]
fn a_sandbox_key_takes_an_override_and_applies_live() {
    let mut settings = Settings::default();
    settings.apply_override("sandbox.cache_mode", "tmp", Source::Flag("--x")).unwrap();
    settings
        .apply_override("sandbox.write_roots", "~/notes, /srv/data", Source::Flag("--x"))
        .unwrap();
    assert_eq!(settings.sandbox.cache_mode, efr_protocol::CacheMode::Tmp);
    assert_eq!(
        settings.sandbox.write_roots,
        [PathBuf::from("~/notes"), PathBuf::from("/srv/data")]
    );
    assert!(settings.apply_override("sandbox.cache_mode", "fast", Source::Flag("--x")).is_err());
    assert!(settings.apply_override("sandbox.write_roots", "~", Source::Flag("--x")).is_err());
    for key in crate::keys().iter().filter(|key| key.starts_with("sandbox.")) {
        assert_eq!(crate::Applies::of(key), crate::Applies::Live, "{key}");
    }
    assert_eq!(
        crate::kind("sandbox.write_projects"),
        Some(crate::Kind::Choice(vec!["turn".to_owned(), "named".to_owned(), "all".to_owned()]))
    );
}

#[test]
fn a_model_entry_is_an_id_or_a_table_with_its_limits() {
    let text = r#"
        [openai]
        models = ["gpt-5.4", { id = "gpt-next", context_window = 400000, max_output_tokens = 128000 }]
    "#;

    let settings = parse(text).unwrap();

    let models = settings.openai.models.unwrap();
    assert_eq!(models[0], ModelEntry::from("gpt-5.4"));
    assert_eq!(models[0].context_window(), None);
    assert_eq!(models[1].id(), "gpt-next");
    assert_eq!(models[1].context_window(), Some(400_000));
    assert_eq!(models[1].max_output_tokens(), Some(128_000));
}

#[test]
fn compaction_is_on_at_76_percent_unless_the_file_says_otherwise() {
    assert_eq!(parse("").unwrap().compaction, CompactionSettings::default());
    assert!(CompactionSettings::default().auto);
    assert_eq!(CompactionSettings::default().auto_at, 76);

    let settings = parse("[compaction]\nauto = false\nauto_at = 60\n").unwrap();

    assert!(!settings.compaction.auto);
    assert_eq!(settings.compaction.auto_at, 60);
}

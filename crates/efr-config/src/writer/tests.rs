use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

use pretty_assertions::assert_eq;
use tempfile::TempDir;

use crate::{ConfigError, ConfigFile, EXAMPLE, Settings};

const ORIGINAL: &str = "\
# My efr settings, kept in dotfiles.
log = \"warn\"

[model]
# Pinned while the new one is tested.
name = \"gpt-5.5\" # the old default
effort = \"low\"

[shell]
idle_minutes = 30
";

fn config_path(dir: &TempDir) -> PathBuf {
    dir.path().join("efr").join("config.toml")
}

fn write_file(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

#[test]
fn a_change_keeps_every_comment_and_the_layout() {
    let dir = tempfile::tempdir().unwrap();
    let path = config_path(&dir);
    write_file(&path, ORIGINAL);

    let file = ConfigFile::open(&path).unwrap();
    let mut edit = file.edit().unwrap();
    edit.set_text("model.name", "gpt-5.4").unwrap();
    edit.set_text("shell.login", "false").unwrap();
    let settings = file.write(&edit).unwrap();

    let expected = ORIGINAL
        .replace("name = \"gpt-5.5\" # the old default", "name = \"gpt-5.4\" # the old default")
        .replace("idle_minutes = 30\n", "idle_minutes = 30\nlogin = false\n");
    assert_eq!(fs::read_to_string(&path).unwrap(), expected);
    assert_eq!(settings.model.name.as_deref(), Some("gpt-5.4"));
    assert!(!settings.shell.login);
}

#[test]
fn unset_removes_the_key_and_keeps_the_rest() {
    let dir = tempfile::tempdir().unwrap();
    let path = config_path(&dir);
    write_file(&path, ORIGINAL);

    let file = ConfigFile::open(&path).unwrap();
    let mut edit = file.edit().unwrap();
    assert!(edit.unset("model.effort").unwrap());
    assert!(!edit.unset("model.max_output_tokens").unwrap());
    assert!(!edit.unset("conversation.max_queued").unwrap());
    file.write(&edit).unwrap();

    assert_eq!(fs::read_to_string(&path).unwrap(), ORIGINAL.replace("effort = \"low\"\n", ""));
}

#[test]
fn a_key_of_a_missing_table_adds_the_table_and_a_top_level_key_stays_on_top() {
    let dir = tempfile::tempdir().unwrap();
    let path = config_path(&dir);
    write_file(&path, ORIGINAL);

    let file = ConfigFile::open(&path).unwrap();
    let mut edit = file.edit().unwrap();
    edit.set_text("conversation.max_queued", "4").unwrap();
    edit.set_text("screen", "vt100").unwrap();
    edit.set_text("openai.models", "gpt-5.5, gpt-5.4").unwrap();
    let settings = file.write(&edit).unwrap();

    let text = fs::read_to_string(&path).unwrap();
    assert!(
        text.starts_with(
            "# My efr settings, kept in dotfiles.\nlog = \"warn\"\nscreen = \"vt100\"\n"
        ),
        "{text}"
    );
    assert!(text.contains("\n[conversation]\nmax_queued = 4\n"), "{text}");
    assert_eq!(settings.conversation.max_queued, 4);
    assert_eq!(settings.openai.models, Some(vec!["gpt-5.5".into(), "gpt-5.4".into()]));
}

#[test]
fn a_colour_key_adds_its_tables_and_unset_removes_it() {
    let dir = tempfile::tempdir().unwrap();
    let path = config_path(&dir);
    write_file(&path, ORIGINAL);

    let file = ConfigFile::open(&path).unwrap();
    let mut edit = file.edit().unwrap();
    edit.set_text("render.colors.accent", "#f2c14e").unwrap();
    edit.set_text("render.colors.diff.add", "10").unwrap();
    edit.set_text("render.palette", "~/theme.toml").unwrap();
    let settings = file.write(&edit).unwrap();

    let text = fs::read_to_string(&path).unwrap();
    assert!(text.ends_with("\n[render]\npalette = \"~/theme.toml\"\n\n[render.colors]\naccent = \"#f2c14e\"\ndiff.add = \"10\"\n"), "{text}");
    assert_eq!(settings.render.colors.colors().len(), 2);

    let file = ConfigFile::open(&path).unwrap();
    let mut edit = file.edit().unwrap();
    assert!(edit.unset("render.colors.diff.add").unwrap());
    assert!(!edit.unset("render.colors.diff.hunk").unwrap());
    let settings = file.write(&edit).unwrap();
    assert_eq!(settings.render.colors.colors().len(), 1);
    let error = file.edit().unwrap().set_text("render.colors", "red").unwrap_err();
    assert!(matches!(error, ConfigError::UnknownKey { .. }), "{error:?}");
}

#[test]
fn the_file_behind_a_symlink_is_written_and_the_link_stays() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("dotfiles").join("efr").join("config.toml");
    write_file(&target, ORIGINAL);
    let path = config_path(&dir);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    symlink(&target, &path).unwrap();

    let file = ConfigFile::open(&path).unwrap();
    assert_eq!(file.target(), fs::canonicalize(&target).unwrap());
    let mut edit = file.edit().unwrap();
    edit.set_text("shell.idle_minutes", "5").unwrap();
    file.write(&edit).unwrap();

    assert!(fs::symlink_metadata(&path).unwrap().file_type().is_symlink());
    assert_eq!(fs::read_link(&path).unwrap(), target);
    let text = fs::read_to_string(&target).unwrap();
    assert!(text.contains("idle_minutes = 5"), "{text}");
    assert!(text.contains("# Pinned while the new one is tested."), "{text}");
    let leftovers: Vec<_> = fs::read_dir(target.parent().unwrap()).unwrap().collect();
    assert_eq!(leftovers.len(), 1, "no temporary file stays behind");
}

#[test]
fn a_link_to_nothing_is_refused_and_nothing_is_created() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("dotfiles").join("config.toml");
    let path = config_path(&dir);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    symlink(&target, &path).unwrap();

    let error = ConfigFile::open(&path).unwrap_err();

    match error {
        ConfigError::DanglingSymlink { path: link, target: to } => {
            assert_eq!(link, path);
            assert_eq!(to, target);
        }
        other => panic!("{other:?}"),
    }
    assert!(!target.exists());
    assert_eq!(Settings::load(path.parent().unwrap()).unwrap().log, "info", "a load reads no file");
}

#[test]
fn a_missing_file_is_created_with_its_directory_from_the_example() {
    let dir = tempfile::tempdir().unwrap();
    let path = config_path(&dir);

    let file = ConfigFile::open(&path).unwrap();
    assert_eq!(file.text(), None);
    let mut edit = file.edit().unwrap();
    edit.set_text("model.effort", "high").unwrap();
    edit.set_text("log", "debug").unwrap();
    let settings = file.write(&edit).unwrap();

    let text = fs::read_to_string(&path).unwrap();
    assert!(text.starts_with("#:schema "), "{text}");
    assert!(text.contains("# The default reasoning effort"), "{text}");
    assert!(
        text.contains("# screen = \"auto\"\n\nlog = \"debug\"\n\n[model]\neffort = \"high\"\n"),
        "{text}"
    );
    assert_eq!(settings.model.effort.as_deref(), Some("high"));
    assert_eq!(settings.log, "debug");
    assert_eq!(text.lines().count(), EXAMPLE.lines().count() + 3, "two keys and a blank line");
}

#[test]
fn a_change_made_by_someone_else_after_the_read_is_detected() {
    let dir = tempfile::tempdir().unwrap();
    let path = config_path(&dir);
    write_file(&path, ORIGINAL);

    let file = ConfigFile::open(&path).unwrap();
    let mut edit = file.edit().unwrap();
    edit.set_text("model.name", "gpt-5.4").unwrap();
    let theirs = ORIGINAL.replace("idle_minutes = 30", "idle_minutes = 45");
    fs::write(&path, &theirs).unwrap();

    let error = file.write(&edit).unwrap_err();

    assert!(matches!(error, ConfigError::Changed { .. }), "{error:?}");
    assert_eq!(fs::read_to_string(&path).unwrap(), theirs);
    let again = ConfigFile::open(&path).unwrap();
    let mut edit = again.edit().unwrap();
    edit.set_text("model.name", "gpt-5.4").unwrap();
    let settings = again.write(&edit).unwrap();
    assert_eq!(settings.shell.idle_minutes, 45, "the second plan keeps their change");
}

#[test]
fn a_file_created_by_someone_else_after_the_read_is_detected() {
    let dir = tempfile::tempdir().unwrap();
    let path = config_path(&dir);

    let file = ConfigFile::open(&path).unwrap();
    let mut edit = file.edit().unwrap();
    edit.set_text("log", "debug").unwrap();
    write_file(&path, "log = \"trace\"\n");

    let error = file.write(&edit).unwrap_err();

    assert!(matches!(error, ConfigError::Changed { .. }), "{error:?}");
    assert_eq!(fs::read_to_string(&path).unwrap(), "log = \"trace\"\n");
}

#[test]
fn an_invalid_change_is_refused_before_anything_is_written() {
    let dir = tempfile::tempdir().unwrap();
    let path = config_path(&dir);
    write_file(&path, ORIGINAL);

    let file = ConfigFile::open(&path).unwrap();
    let mut edit = file.edit().unwrap();
    edit.set_text("conversation.max_queued", "0").unwrap();
    let error = file.write(&edit).unwrap_err();

    assert!(
        matches!(error, ConfigError::Invalid { key: "conversation.max_queued", .. }),
        "{error:?}"
    );
    assert_eq!(fs::read_to_string(&path).unwrap(), ORIGINAL);
}

#[test]
fn values_of_the_wrong_kind_rules_and_unknown_keys_cannot_be_set() {
    let dir = tempfile::tempdir().unwrap();
    let file = ConfigFile::open(&config_path(&dir)).unwrap();
    let mut edit = file.edit().unwrap();

    let error = edit.set_text("shell.idle_minutes", "soon").unwrap_err();
    assert_eq!(error.to_string(), "shell.idle_minutes = \"soon\" is not a whole number");
    let error = edit.set_text("permissions.mode", "yolo").unwrap_err();
    assert_eq!(
        error.to_string(),
        "permissions.mode = \"yolo\" is not manual, cautious, auto or full"
    );
    for key in ["permissions.rules", "shell.colour", "model", "nothing"] {
        let error = edit.set_text(key, "x").unwrap_err();
        assert!(matches!(error, ConfigError::UnknownKey { .. }), "{key}: {error:?}");
        assert!(matches!(edit.unset(key), Err(ConfigError::UnknownKey { .. })), "{key}");
    }
    assert_eq!(edit.text(), EXAMPLE);
}

#[test]
fn a_file_that_is_not_toml_cannot_be_edited() {
    let dir = tempfile::tempdir().unwrap();
    let path = config_path(&dir);
    write_file(&path, "[model\n");

    let error = ConfigFile::open(&path).unwrap().edit().unwrap_err();

    assert!(matches!(error, ConfigError::Parse { .. }), "{error:?}");
    assert_eq!(error.location().map(|at| at.line), Some(1));
}

#[test]
fn a_file_with_an_invalid_value_can_be_fixed() {
    let dir = tempfile::tempdir().unwrap();
    let path = config_path(&dir);
    write_file(&path, "[conversation]\nmax_queued = 0\n");

    let file = ConfigFile::open(&path).unwrap();
    let mut edit = file.edit().unwrap();
    edit.set_text("conversation.max_queued", "8").unwrap();
    let settings = file.write(&edit).unwrap();

    assert_eq!(settings.conversation.max_queued, 8);
}

fn allow_cargo_test() -> efr_permissions::Rule {
    let pattern = efr_permissions::CommandPattern::new("cargo").with_args(["test"]);
    efr_permissions::Rule::new(
        efr_permissions::Action::Execute,
        efr_permissions::Resource::Command(pattern),
        efr_permissions::Effect::Allow,
    )
}

fn deny_downloads() -> efr_permissions::Rule {
    efr_permissions::Rule::new(
        efr_permissions::Action::Write,
        efr_permissions::Resource::Under("~/Downloads".into()),
        efr_permissions::Effect::Deny,
    )
}

const WITH_RULES: &str = "\
# My rules.
[permissions]
mode = \"cautious\" # for now

# Never touch the downloads.
[[permissions.rules]]
action = \"write\"
resource = { under = \"~/Downloads\" }
effect = \"deny\"

[shell]
idle_minutes = 30
";

#[test]
fn a_new_rule_goes_after_the_rules_of_the_file_and_the_comments_stay() {
    let dir = tempfile::tempdir().unwrap();
    let path = config_path(&dir);
    write_file(&path, WITH_RULES);

    let file = ConfigFile::open(&path).unwrap();
    let mut edit = file.edit().unwrap();
    edit.add_rule(&allow_cargo_test()).unwrap();
    let settings = file.write(&edit).unwrap();

    let expected = WITH_RULES.replace(
        "effect = \"deny\"\n",
        "effect = \"deny\"\n\n[[permissions.rules]]\naction = \"execute\"\nresource = { command = \
         { program = \"cargo\", args = [\"test\"] } }\neffect = \"allow\"\n",
    );
    assert_eq!(fs::read_to_string(&path).unwrap(), expected);
    assert_eq!(settings.permissions.rules.rules(), [deny_downloads(), allow_cargo_test()]);
}

#[test]
fn the_first_rule_goes_below_the_permissions_table_and_a_missing_table_is_added() {
    let dir = tempfile::tempdir().unwrap();
    let path = config_path(&dir);
    let without_rules = "[permissions]\nmode = \"auto\"\n\n[shell]\nidle_minutes = 30\n";
    write_file(&path, without_rules);

    let file = ConfigFile::open(&path).unwrap();
    let mut edit = file.edit().unwrap();
    edit.add_rule(&deny_downloads()).unwrap();
    file.write(&edit).unwrap();
    let text = fs::read_to_string(&path).unwrap();
    assert_eq!(
        text,
        "[permissions]\nmode = \"auto\"\n\n[[permissions.rules]]\naction = \"write\"\nresource \
         = { under = \"~/Downloads\" }\neffect = \"deny\"\n\n[shell]\nidle_minutes = 30\n"
    );

    write_file(&path, ORIGINAL);
    let file = ConfigFile::open(&path).unwrap();
    let mut edit = file.edit().unwrap();
    edit.add_rule(&deny_downloads()).unwrap();
    let settings = file.write(&edit).unwrap();
    let text = fs::read_to_string(&path).unwrap();
    assert!(text.starts_with(ORIGINAL), "{text}");
    assert!(text.ends_with("\n[[permissions.rules]]\naction = \"write\"\nresource = { under = \"~/Downloads\" }\neffect = \"deny\"\n"), "{text}");
    assert_eq!(settings.permissions.rules.rules(), [deny_downloads()]);
}

#[test]
fn a_rule_added_to_the_example_reads_back() {
    let dir = tempfile::tempdir().unwrap();
    let path = config_path(&dir);

    let file = ConfigFile::open(&path).unwrap();
    let mut edit = file.edit().unwrap();
    edit.add_rule(&allow_cargo_test()).unwrap();
    let settings = file.write(&edit).unwrap();

    assert_eq!(settings.permissions.rules.rules(), [allow_cargo_test()]);
    let text = fs::read_to_string(&path).unwrap();
    assert!(text.contains("# [[permissions.rules]]"), "the commented example stays: {text}");
}

#[test]
fn rules_written_inline_stay_inline() {
    let dir = tempfile::tempdir().unwrap();
    let path = config_path(&dir);
    write_file(
        &path,
        "permissions.rules = [{ action = \"write\", resource = { under = \"~/Downloads\" }, effect = \"deny\" }]\n",
    );

    let file = ConfigFile::open(&path).unwrap();
    let mut edit = file.edit().unwrap();
    edit.add_rule(&allow_cargo_test()).unwrap();
    let settings = file.write(&edit).unwrap();

    assert_eq!(settings.permissions.rules.rules(), [deny_downloads(), allow_cargo_test()]);
    let text = fs::read_to_string(&path).unwrap();
    assert!(!text.contains("[[permissions.rules]]"), "{text}");

    write_file(&path, "permissions = { mode = \"auto\" }\n");
    let file = ConfigFile::open(&path).unwrap();
    let mut edit = file.edit().unwrap();
    edit.add_rule(&deny_downloads()).unwrap();
    let settings = file.write(&edit).unwrap();
    assert_eq!(settings.permissions.rules.rules(), [deny_downloads()]);
}

#[test]
fn a_rule_is_removed_by_its_place_and_a_missing_one_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = config_path(&dir);
    write_file(&path, WITH_RULES);
    let file = ConfigFile::open(&path).unwrap();
    let mut edit = file.edit().unwrap();
    edit.add_rule(&allow_cargo_test()).unwrap();
    file.write(&edit).unwrap();

    let file = ConfigFile::open(&path).unwrap();
    let mut edit = file.edit().unwrap();
    let error = edit.remove_rule(2).unwrap_err();
    assert!(matches!(error, ConfigError::NoRule { index: 2, count: 2 }), "{error:?}");
    assert_eq!(error.key().as_deref(), Some("permissions.rules[2]"));
    edit.remove_rule(0).unwrap();
    let settings = file.write(&edit).unwrap();
    assert_eq!(settings.permissions.rules.rules(), [allow_cargo_test()]);
    let text = fs::read_to_string(&path).unwrap();
    assert!(text.contains("mode = \"cautious\" # for now"), "{text}");
    assert!(!text.contains("~/Downloads"), "{text}");

    let file = ConfigFile::open(&path).unwrap();
    let mut edit = file.edit().unwrap();
    edit.remove_rule(0).unwrap();
    file.write(&edit).unwrap();
    let text = fs::read_to_string(&path).unwrap();
    assert!(!text.contains("permissions.rules"), "the last rule takes the key with it: {text}");

    let mut edit = ConfigFile::open(&path).unwrap().edit().unwrap();
    assert!(matches!(edit.remove_rule(0), Err(ConfigError::NoRule { index: 0, count: 0 })));
}

fn notes() -> efr_permissions::AllowRule {
    efr_permissions::AllowRule::Write { under: "~/notes".to_owned(), project: None }
}

fn download_host() -> efr_permissions::AllowRule {
    efr_permissions::AllowRule::Host { host: "download.example.org".to_owned(), port: 443 }
}

const WITH_ALLOW: &str = "\
# My always rules.
[sandbox]
cache_days = 7 # a week

# The notes of the build.
[[sandbox.allow]]
kind = \"write\"
under = \"~/notes\"

[shell]
idle_minutes = 30
";

#[test]
fn add_table_entry_keeps_comments_and_order() {
    let dir = tempfile::tempdir().unwrap();
    let path = config_path(&dir);
    write_file(&path, WITH_ALLOW);

    let file = ConfigFile::open(&path).unwrap();
    let mut edit = file.edit().unwrap();
    let index = edit.add_table_entry("sandbox.allow", &download_host()).unwrap();
    let settings = file.write(&edit).unwrap();

    assert_eq!(index, 1);
    let expected = WITH_ALLOW.replace(
        "under = \"~/notes\"\n",
        "under = \"~/notes\"\n\n[[sandbox.allow]]\nkind = \"host\"\nhost = \
         \"download.example.org\"\nport = 443\n",
    );
    assert_eq!(fs::read_to_string(&path).unwrap(), expected);
    assert_eq!(settings.sandbox.allow, [notes(), download_host()]);
    assert_eq!(settings.sandbox.cache_days, 7);
}

#[test]
fn add_table_entry_first_entry() {
    let dir = tempfile::tempdir().unwrap();
    let path = config_path(&dir);
    // A [sandbox] table with no entries gets the first one right below it.
    write_file(&path, "[sandbox]\ncache_days = 7\n\n[shell]\nidle_minutes = 30\n");
    let file = ConfigFile::open(&path).unwrap();
    let mut edit = file.edit().unwrap();
    assert_eq!(edit.add_table_entry("sandbox.allow", &notes()).unwrap(), 0);
    file.write(&edit).unwrap();
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        "[sandbox]\ncache_days = 7\n\n[[sandbox.allow]]\nkind = \"write\"\nunder = \
         \"~/notes\"\n\n[shell]\nidle_minutes = 30\n"
    );

    // A file with no [dbus] table gets one at its end.
    write_file(&path, ORIGINAL);
    let file = ConfigFile::open(&path).unwrap();
    let mut edit = file.edit().unwrap();
    let entry = efr_permissions::BusRule {
        bus: efr_protocol::BusKind::Session,
        name: "org.freedesktop.Notifications".to_owned(),
        call: vec!["org.freedesktop.Notifications.GetServerInformation".to_owned()],
    };
    assert_eq!(edit.add_table_entry("dbus.allow", &entry).unwrap(), 0);
    let settings = file.write(&edit).unwrap();
    let text = fs::read_to_string(&path).unwrap();
    assert!(text.starts_with(ORIGINAL), "{text}");
    assert!(
        text.ends_with(
            "\n[[dbus.allow]]\nbus = \"session\"\nname = \"org.freedesktop.Notifications\"\ncall \
             = [\"org.freedesktop.Notifications.GetServerInformation\"]\n"
        ),
        "{text}"
    );
    assert_eq!(settings.dbus.allow, [entry]);

    // The example, where every key is a comment, reads back with the entry.
    let dir = tempfile::tempdir().unwrap();
    let path = config_path(&dir);
    let file = ConfigFile::open(&path).unwrap();
    let mut edit = file.edit().unwrap();
    edit.add_table_entry("sandbox.allow", &notes()).unwrap();
    let settings = file.write(&edit).unwrap();
    assert_eq!(settings.sandbox.allow, [notes()]);
}

#[test]
fn add_table_entry_inline_and_dotted_forms() {
    let dir = tempfile::tempdir().unwrap();
    let path = config_path(&dir);
    let cases = [
        // An inline array keeps its form.
        "[sandbox]\nallow = [{ kind = \"write\", under = \"~/notes\" }]\n",
        // An inline parent table keeps its form.
        "sandbox = { allow = [{ kind = \"write\", under = \"~/notes\" }] }\n",
        "sandbox = { cache_days = 7 }\n",
        // A dotted key keeps its form.
        "sandbox.allow = [{ kind = \"write\", under = \"~/notes\" }]\n",
        "sandbox.cache_days = 7\n",
    ];
    for text in cases {
        write_file(&path, text);
        let had = Settings::parse(&path, Some(text)).unwrap().sandbox.allow;
        let file = ConfigFile::open(&path).unwrap();
        let mut edit = file.edit().unwrap();
        let index = edit.add_table_entry("sandbox.allow", &download_host()).unwrap();
        let settings = file.write(&edit).unwrap();

        assert_eq!(index, had.len(), "{text}");
        let mut expected = had.clone();
        expected.push(download_host());
        assert_eq!(settings.sandbox.allow, expected, "{text}");
        let written = fs::read_to_string(&path).unwrap();
        let tables = written.contains("[[sandbox.allow]]");
        let inline = text.contains("allow = [") || text.starts_with("sandbox = {");
        assert_eq!(tables, !inline, "{text} -> {written}");
        if text.starts_with("sandbox = {") {
            assert!(written.starts_with("sandbox = {"), "{written}");
        }
        if text.starts_with("sandbox.") {
            assert!(!written.contains("[sandbox]"), "{written}");
        }
    }
}

#[test]
fn add_table_entry_through_symlink() {
    let dir = tempfile::tempdir().unwrap();
    let real = dir.path().join("dotfiles").join("efr.toml");
    write_file(&real, WITH_ALLOW);
    let path = config_path(&dir);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    symlink(&real, &path).unwrap();

    let file = ConfigFile::open(&path).unwrap();
    let mut edit = file.edit().unwrap();
    edit.add_table_entry("sandbox.allow", &download_host()).unwrap();
    file.write(&edit).unwrap();

    assert!(fs::symlink_metadata(&path).unwrap().file_type().is_symlink());
    let text = fs::read_to_string(&real).unwrap();
    assert!(text.contains("host = \"download.example.org\""), "{text}");
    assert!(text.contains("# The notes of the build."), "{text}");
}

#[test]
fn add_table_entry_detects_change() {
    let dir = tempfile::tempdir().unwrap();
    let path = config_path(&dir);
    write_file(&path, WITH_ALLOW);

    let file = ConfigFile::open(&path).unwrap();
    let mut edit = file.edit().unwrap();
    edit.add_table_entry("sandbox.allow", &download_host()).unwrap();
    let theirs = WITH_ALLOW.replace("cache_days = 7", "cache_days = 9");
    fs::write(&path, &theirs).unwrap();

    let error = file.write(&edit).unwrap_err();
    assert!(matches!(error, ConfigError::Changed { .. }), "{error:?}");
    assert_eq!(fs::read_to_string(&path).unwrap(), theirs);
}

#[test]
fn add_table_entry_refuses_invalid_before_write() {
    let dir = tempfile::tempdir().unwrap();
    let path = config_path(&dir);
    write_file(&path, WITH_ALLOW);

    let file = ConfigFile::open(&path).unwrap();
    let mut edit = file.edit().unwrap();
    let pattern = efr_permissions::AllowRule::Host { host: "*.example.org".to_owned(), port: 443 };
    assert_eq!(edit.add_table_entry("sandbox.allow", &pattern).unwrap(), 1);
    let error = file.write(&edit).unwrap_err();
    assert!(
        matches!(error, ConfigError::InvalidEntry { key: "sandbox.allow", index: 1, .. }),
        "{error:?}"
    );
    assert_eq!(error.key().as_deref(), Some("sandbox.allow[1]"));
    assert_eq!(fs::read_to_string(&path).unwrap(), WITH_ALLOW);

    let mut edit = file.edit().unwrap();
    let wide = efr_permissions::BusRule {
        bus: efr_protocol::BusKind::System,
        name: "org.freedesktop.systemd1".to_owned(),
        call: vec!["org.freedesktop.systemd1.Manager.*".to_owned()],
    };
    edit.add_table_entry("dbus.allow", &wide).unwrap();
    let error = file.write(&edit).unwrap_err();
    assert!(
        matches!(error, ConfigError::InvalidEntry { key: "dbus.allow", index: 0, .. }),
        "{error:?}"
    );
    assert_eq!(fs::read_to_string(&path).unwrap(), WITH_ALLOW);

    // Only the arrays of tables take entries.
    for key in ["sandbox.write_roots", "sandbox", "dbus.proxy", "nothing.allow", "allow"] {
        let error = edit.add_table_entry(key, &notes()).unwrap_err();
        assert!(matches!(error, ConfigError::UnknownKey { .. }), "{key}: {error:?}");
        let error = edit.remove_table_entry(key, 0).unwrap_err();
        assert!(matches!(error, ConfigError::UnknownKey { .. }), "{key}: {error:?}");
    }
    // An array of tables is never a value that `set` writes.
    for key in ["sandbox.allow", "dbus.allow"] {
        let error = edit.set_text(key, "[]").unwrap_err();
        assert!(matches!(error, ConfigError::UnknownKey { .. }), "{key}: {error:?}");
        assert!(matches!(edit.unset(key), Err(ConfigError::UnknownKey { .. })), "{key}");
    }
}

#[test]
fn remove_table_entry_by_index() {
    let dir = tempfile::tempdir().unwrap();
    let path = config_path(&dir);
    write_file(&path, WITH_ALLOW);
    let file = ConfigFile::open(&path).unwrap();
    let mut edit = file.edit().unwrap();
    edit.add_table_entry("sandbox.allow", &download_host()).unwrap();
    file.write(&edit).unwrap();

    let file = ConfigFile::open(&path).unwrap();
    let mut edit = file.edit().unwrap();
    let error = edit.remove_table_entry("sandbox.allow", 2).unwrap_err();
    assert!(
        matches!(error, ConfigError::NoEntry { key: "sandbox.allow", index: 2, count: 2 }),
        "{error:?}"
    );
    assert_eq!(error.key().as_deref(), Some("sandbox.allow[2]"));
    edit.remove_table_entry("sandbox.allow", 0).unwrap();
    let settings = file.write(&edit).unwrap();
    assert_eq!(settings.sandbox.allow, [download_host()]);
    let text = fs::read_to_string(&path).unwrap();
    assert!(text.contains("cache_days = 7 # a week"), "{text}");
    assert!(!text.contains("~/notes"), "{text}");

    let file = ConfigFile::open(&path).unwrap();
    let mut edit = file.edit().unwrap();
    edit.remove_table_entry("sandbox.allow", 0).unwrap();
    file.write(&edit).unwrap();
    let text = fs::read_to_string(&path).unwrap();
    assert!(!text.contains("sandbox.allow"), "the last entry takes the key with it: {text}");

    let mut edit = ConfigFile::open(&path).unwrap().edit().unwrap();
    assert!(matches!(
        edit.remove_table_entry("sandbox.allow", 0),
        Err(ConfigError::NoEntry { index: 0, count: 0, .. })
    ));
    assert!(matches!(
        edit.remove_table_entry("dbus.allow", 0),
        Err(ConfigError::NoEntry { key: "dbus.allow", index: 0, count: 0 })
    ));
}

#[test]
fn the_table_keys_are_the_keys_of_tables() {
    let mut tables: Vec<String> = crate::keys()
        .into_iter()
        .filter(|key| crate::kind(key) == Some(crate::Kind::Rules))
        .collect();
    tables.sort_unstable();
    let mut known: Vec<String> = super::TABLE_KEYS.iter().map(|key| (*key).to_owned()).collect();
    known.sort_unstable();
    assert_eq!(tables, known);
}

#[test]
fn allow_rule_round_trips_through_toml_edit() {
    let text = "\
[sandbox]
allow = [
    { kind = \"host\", host = \"download.example.org\" },
    { kind = \"host\", host = \"mirror.example.org\", port = 8443 },
    { kind = \"write\", under = \"~/p/app/out\", project = \"~/p/app\" },
    { kind = \"socket\", path = \"/run/user/1000/mpd/socket\" },
]

[dbus]
allow = [{ bus = \"system\", name = \"org.freedesktop.timedate1\", call = [\"org.freedesktop.DBus.Properties.GetAll\"] }]
";
    let path = Path::new("/home/u/.config/efr/config.toml");
    let settings = Settings::parse(path, Some(text)).unwrap();
    assert_eq!(settings.sandbox.allow.len(), 4);
    assert_eq!(settings.dbus.allow.len(), 1);

    // The encoder of the writer and of a reload gives back the same rules.
    let document = toml_edit::ser::to_document(&settings).unwrap();
    let back: Settings = toml::from_str(&document.to_string()).unwrap();
    assert_eq!(back.sandbox.allow, settings.sandbox.allow);
    assert_eq!(back.dbus.allow, settings.dbus.allow);

    // A reload that keeps an overridden key encodes and reads the settings again.
    let mut running = Settings::parse(path, None).unwrap();
    running.apply_override("log", "debug", crate::Source::Flag("--log")).unwrap();
    let reloaded = running.reloaded(settings.clone()).unwrap();
    assert_eq!(reloaded.settings.log, "debug");
    assert_eq!(reloaded.settings.sandbox.allow, settings.sandbox.allow);
    assert_eq!(reloaded.settings.dbus.allow, settings.dbus.allow);

    // An override of another key keeps the rules too.
    let mut overridden = settings.clone();
    overridden.apply_override("dbus.proxy", "true", crate::Source::Flag("--dbus-proxy")).unwrap();
    assert!(overridden.dbus.proxy);
    assert_eq!(overridden.sandbox.allow, settings.sandbox.allow);
}

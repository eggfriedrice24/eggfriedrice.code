use crate::Settings;

use super::check;

#[test]
fn the_defaults_pass_every_check() {
    assert_eq!(check(&Settings::default()), Ok(()));
}

#[test]
fn an_effort_is_one_lowercase_word_of_at_most_32_bytes() {
    let with = |effort: &str| {
        let mut settings = Settings::default();
        settings.model.effort = Some(effort.to_owned());
        check(&settings)
    };
    let longest = "a".repeat(32);
    for effort in
        ["low", "medium", "high", "xhigh", "minimal", "none", "very-high", "max_2", &longest]
    {
        assert_eq!(with(effort), Ok(()), "{effort}");
    }
    let too_long = "a".repeat(33);
    for effort in ["", "High", "very high", "high!", "\u{e9}", &too_long] {
        assert_eq!(with(effort).map_err(|invalid| invalid.key), Err("model.effort"), "{effort}");
    }
}

#[test]
fn a_secret_path_below_home_must_start_with_the_tilde_component() {
    let mut settings = Settings::default();
    settings.permissions.secret_paths = vec!["~/.vault".into(), "/srv/vault".into()];
    assert_eq!(check(&settings), Ok(()));

    settings.permissions.secret_paths = vec!["~vault".into()];
    let invalid = check(&settings).unwrap_err();
    assert_eq!(invalid.key, "permissions.secret_paths");
    assert_eq!(invalid.value, "\"~vault\"");
}

#[test]
fn the_interactive_timeout_is_between_a_minute_and_a_day() {
    let with = |minutes: u64| {
        let mut settings = Settings::default();
        settings.shell.interactive_timeout_minutes = minutes;
        check(&settings)
    };
    assert_eq!(Settings::default().shell.interactive_timeout_minutes, 60);
    for minutes in [1, 60, 1_440] {
        assert_eq!(with(minutes), Ok(()), "{minutes}");
    }
    for minutes in [0, 1_441] {
        assert_eq!(
            with(minutes).map_err(|invalid| invalid.key),
            Err("shell.interactive_timeout_minutes"),
            "{minutes}"
        );
    }
}

#[test]
fn sandbox_paths_are_absolute_or_below_home() {
    let mut settings = Settings::default();
    settings.sandbox.write_roots = vec!["~/notes".into(), "/srv/data".into()];
    settings.sandbox.mask = vec!["~/.config/private".into()];
    assert_eq!(check(&settings), Ok(()));

    for (key, set) in [
        (
            "sandbox.write_roots",
            (|s: &mut Settings| s.sandbox.write_roots = vec!["notes".into()]) as fn(&mut Settings),
        ),
        ("sandbox.caches", |s: &mut Settings| s.sandbox.caches = vec!["cache".into()]),
        ("sandbox.mask", |s: &mut Settings| s.sandbox.mask = vec!["~x".into()]),
        ("sandbox.protect", |s: &mut Settings| s.sandbox.protect = vec!["./a".into()]),
        ("sandbox.synced_dirs", |s: &mut Settings| s.sandbox.synced_dirs = vec!["Sync".into()]),
        ("sandbox.bwrap", |s: &mut Settings| s.sandbox.bwrap = Some("bwrap".into())),
    ] {
        let mut settings = Settings::default();
        set(&mut settings);
        assert_eq!(check(&settings).map_err(|invalid| invalid.key), Err(key), "{key}");
    }
}

#[test]
fn the_home_directory_and_the_root_are_never_extra_write_roots() {
    for root in ["~", "/"] {
        let mut settings = Settings::default();
        settings.sandbox.write_roots = vec![root.into()];
        let invalid = check(&settings).unwrap_err();
        assert_eq!(invalid.key, "sandbox.write_roots", "{root}");
    }
}

#[test]
fn sandbox_variable_lists_take_names_and_star_patterns_only() {
    let mut settings = Settings::default();
    settings.sandbox.env_deny = vec!["MY_*".to_owned(), "_X".to_owned()];
    assert_eq!(check(&settings), Ok(()));
    for name in ["", "1X", "A-B", "A B", "A=B"] {
        let mut settings = Settings::default();
        settings.sandbox.promote_env = vec![name.to_owned()];
        assert_eq!(
            check(&settings).map_err(|invalid| invalid.key),
            Err("sandbox.promote_env"),
            "{name:?}"
        );
    }
}

#[test]
fn sandbox_patterns_and_names_are_relative() {
    for (pattern, ok) in [
        (".env.*", true),
        ("!.env.example", true),
        (".vscode/tasks.json", true),
        ("/etc/x", false),
        ("../x", false),
        ("!", false),
        ("a//b", false),
    ] {
        let mut settings = Settings::default();
        settings.sandbox.surface_files = vec![pattern.to_owned()];
        assert_eq!(check(&settings).is_ok(), ok, "{pattern}");
    }
    for (name, ok) in
        [("target", true), (".venv", true), ("a/b", false), ("..", false), ("", false)]
    {
        let mut settings = Settings::default();
        settings.sandbox.rebuildable = vec![name.to_owned()];
        assert_eq!(check(&settings).is_ok(), ok, "{name:?}");
    }
}

#[test]
fn snapshot_limits_and_diff_lines_have_ranges() {
    type Change = fn(&mut Settings);
    let cases: [(Change, &str); 5] = [
        (|s| s.snapshot.max_file_mib = 0, "snapshot.max_file_mib"),
        (|s| s.snapshot.max_files = 99, "snapshot.max_files"),
        (|s| s.snapshot.keep_turns = 0, "snapshot.keep_turns"),
        (|s| s.snapshot.max_age_days = 3_651, "snapshot.max_age_days"),
        (|s| s.render.diff_lines = 1_001, "render.diff_lines"),
    ];
    for (change, key) in cases {
        let mut settings = Settings::default();
        change(&mut settings);
        assert_eq!(check(&settings).map_err(|invalid| invalid.key), Err(key));
    }
    let mut settings = Settings::default();
    settings.render.diff_lines = 0;
    assert!(check(&settings).is_ok(), "0 shows no inline diff");
}

#[test]
fn sandbox_cache_limits_have_ranges() {
    let mut settings = Settings::default();
    settings.sandbox.cache_days = 0;
    assert_eq!(check(&settings).map_err(|invalid| invalid.key), Err("sandbox.cache_days"));
    let mut settings = Settings::default();
    settings.sandbox.cache_max_gib = 10_001;
    assert_eq!(check(&settings).map_err(|invalid| invalid.key), Err("sandbox.cache_max_gib"));
}

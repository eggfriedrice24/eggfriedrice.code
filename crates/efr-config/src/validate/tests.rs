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
fn recording_limits_have_ranges_and_zero_turns_them_off() {
    let defaults = Settings::default().recording;
    assert_eq!((defaults.max_age_days, defaults.max_gib), (14, 2));
    let mut settings = Settings::default();
    settings.recording.max_age_days = 3_651;
    assert_eq!(check(&settings).map_err(|invalid| invalid.key), Err("recording.max_age_days"));
    let mut settings = Settings::default();
    settings.recording.max_gib = 10_001;
    assert_eq!(check(&settings).map_err(|invalid| invalid.key), Err("recording.max_gib"));
    let mut settings = Settings::default();
    (settings.recording.max_age_days, settings.recording.max_gib) = (0, 0);
    assert!(check(&settings).is_ok(), "0 turns each limit off");
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

#[test]
fn classifier_keys_have_defaults_and_checks() {
    let defaults = Settings::default().classifier;
    assert!(defaults.enabled);
    assert_eq!(defaults.model, None);
    assert!(defaults.fallback_to_turn_model);
    assert_eq!(defaults.effort, "low");
    assert_eq!(defaults.timeout_secs, 20);

    type Change = fn(&mut Settings);
    let refused: [(Change, &str); 5] = [
        (|s| s.classifier.model = Some(String::new()), "classifier.model"),
        (|s| s.classifier.effort = "Low".to_owned(), "classifier.effort"),
        (|s| s.classifier.effort = String::new(), "classifier.effort"),
        (|s| s.classifier.timeout_secs = 0, "classifier.timeout_secs"),
        (|s| s.classifier.timeout_secs = 301, "classifier.timeout_secs"),
    ];
    for (change, key) in refused {
        let mut settings = Settings::default();
        change(&mut settings);
        assert_eq!(check(&settings).map_err(|invalid| invalid.key), Err(key));
    }
    let accepted: [Change; 4] = [
        |s| s.classifier.model = Some("gpt-6-luna".to_owned()),
        |s| s.classifier.effort = "minimal".to_owned(),
        |s| s.classifier.timeout_secs = 1,
        |s| s.classifier.timeout_secs = 300,
    ];
    for change in accepted {
        let mut settings = Settings::default();
        change(&mut settings);
        assert_eq!(check(&settings), Ok(()));
    }
}

#[test]
fn presence_keys_have_defaults_and_checks() {
    let defaults = Settings::default().presence;
    assert_eq!(defaults.away_after_minutes, 10);
    assert_eq!(defaults.away_wait_secs, 300);
    assert!(defaults.push);

    type Change = fn(&mut Settings);
    let refused: [(Change, &str); 4] = [
        (|s| s.presence.away_after_minutes = 0, "presence.away_after_minutes"),
        (|s| s.presence.away_after_minutes = 1_441, "presence.away_after_minutes"),
        (|s| s.presence.away_wait_secs = 0, "presence.away_wait_secs"),
        (|s| s.presence.away_wait_secs = 86_401, "presence.away_wait_secs"),
    ];
    for (change, key) in refused {
        let mut settings = Settings::default();
        change(&mut settings);
        assert_eq!(check(&settings).map_err(|invalid| invalid.key), Err(key));
    }
    let mut settings = Settings::default();
    settings.presence.away_after_minutes = 1_440;
    settings.presence.away_wait_secs = 86_400;
    assert_eq!(check(&settings), Ok(()));
}

#[test]
fn the_recent_commands_key_has_its_default_and_its_range() {
    assert_eq!(Settings::default().context.recent_commands, 8);
    let mut settings = Settings::default();
    settings.context.recent_commands = 11;
    assert_eq!(check(&settings).map_err(|invalid| invalid.key), Err("context.recent_commands"));
    for count in [0, 10] {
        settings.context.recent_commands = count;
        assert_eq!(check(&settings), Ok(()));
    }
    let path = std::path::Path::new("/c/config.toml");
    let parsed = Settings::parse(path, Some("[context]\nrecent_commands = 3\n")).unwrap();
    assert_eq!(parsed.context.recent_commands, 3);
    assert!(Settings::parse(path, Some("[context]\nrecent = 3\n")).is_err());
}

#[test]
fn undo_keys_have_defaults_and_checks() {
    use std::path::Path;

    assert!(Settings::default().undo.enabled);
    assert_eq!(crate::kind("undo.enabled"), Some(crate::Kind::Boolean));
    let path = Path::new("/home/u/.config/efr/config.toml");
    let off = Settings::parse(path, Some("[undo]\nenabled = false\n")).unwrap();
    assert!(!off.undo.enabled);
    assert_eq!(check(&off), Ok(()));
    assert!(Settings::parse(path, Some("[undo]\nenabled = \"no\"\n")).is_err());
    assert!(
        Settings::parse(path, Some("[undo]\nforget = true\n")).is_err(),
        "an unknown key of [undo] is an error"
    );
}

#[test]
fn snapshot_mirror_and_home_keys_have_checks() {
    let defaults = Settings::default().snapshot;
    assert_eq!(defaults.mirror_max_mib, 512);
    assert_eq!(defaults.home_max_files, 2_000);
    assert_eq!(defaults.home_max_mib, 256);

    type Change = fn(&mut Settings);
    let refused: [(Change, &str); 3] = [
        (|s| s.snapshot.mirror_max_mib = 65_537, "snapshot.mirror_max_mib"),
        (|s| s.snapshot.home_max_files = 100_001, "snapshot.home_max_files"),
        (|s| s.snapshot.home_max_mib = 16_385, "snapshot.home_max_mib"),
    ];
    for (change, key) in refused {
        let mut settings = Settings::default();
        change(&mut settings);
        assert_eq!(check(&settings).map_err(|invalid| invalid.key), Err(key));
    }
    let accepted: [Change; 2] = [
        |s| {
            s.snapshot.mirror_max_mib = 0;
            s.snapshot.home_max_files = 0;
            s.snapshot.home_max_mib = 0;
        },
        |s| {
            s.snapshot.mirror_max_mib = 65_536;
            s.snapshot.home_max_files = 100_000;
            s.snapshot.home_max_mib = 16_384;
        },
    ];
    for change in accepted {
        let mut settings = Settings::default();
        change(&mut settings);
        assert_eq!(check(&settings), Ok(()), "0 turns a copy off, and each top is allowed");
    }
}

#[test]
fn ask_destructive_has_two_values() {
    use std::path::Path;

    use crate::AskDestructive;

    assert_eq!(Settings::default().sandbox.ask_destructive, AskDestructive::UnlessUndoable);
    assert_eq!(
        crate::kind("sandbox.ask_destructive"),
        Some(crate::Kind::Choice(vec!["always".to_owned(), "unless_undoable".to_owned()]))
    );
    let path = Path::new("/home/u/.config/efr/config.toml");
    for value in [AskDestructive::Always, AskDestructive::UnlessUndoable] {
        let text = format!("[sandbox]\nask_destructive = \"{}\"\n", value.as_str());
        let settings = Settings::parse(path, Some(&text)).unwrap();
        assert_eq!(settings.sandbox.ask_destructive, value);
    }
    assert!(Settings::parse(path, Some("[sandbox]\nask_destructive = \"never\"\n")).is_err());
}

#[test]
fn jobs_keys_have_defaults_and_checks() {
    use std::path::Path;

    let defaults = Settings::default().jobs;
    assert!(defaults.enabled);
    assert_eq!(defaults.default_minutes, 60);
    assert_eq!(defaults.max_minutes, 480);
    assert_eq!(defaults.max_running, 4);
    assert_eq!(defaults.max_running_daemon, 16);
    assert_eq!(defaults.max_wakes_per_hour, 6);
    assert_eq!(defaults.max_wakes_per_hour_daemon, 12);
    assert_eq!(defaults.max_wakes_per_watch, 3);
    assert_eq!(defaults.nice, 19);
    assert_eq!(crate::kind("jobs.enabled"), Some(crate::Kind::Boolean));

    type Change = fn(&mut Settings);
    let refused: [(Change, &str); 12] = [
        (|s| s.jobs.default_minutes = 0, "jobs.default_minutes"),
        (|s| s.jobs.default_minutes = 1_441, "jobs.default_minutes"),
        (|s| s.jobs.max_minutes = 0, "jobs.max_minutes"),
        (|s| s.jobs.max_minutes = 1_441, "jobs.max_minutes"),
        (|s| s.jobs.max_running = 0, "jobs.max_running"),
        (|s| s.jobs.max_running = 65, "jobs.max_running"),
        (|s| s.jobs.max_running_daemon = 0, "jobs.max_running_daemon"),
        (|s| s.jobs.max_running_daemon = 257, "jobs.max_running_daemon"),
        (|s| s.jobs.max_wakes_per_hour = 61, "jobs.max_wakes_per_hour"),
        (|s| s.jobs.max_wakes_per_hour_daemon = 121, "jobs.max_wakes_per_hour_daemon"),
        (|s| s.jobs.max_wakes_per_watch = 11, "jobs.max_wakes_per_watch"),
        (|s| s.jobs.nice = 20, "jobs.nice"),
    ];
    for (change, key) in refused {
        let mut settings = Settings::default();
        change(&mut settings);
        assert_eq!(check(&settings).map_err(|invalid| invalid.key), Err(key));
    }
    let mut settings = Settings::default();
    settings.jobs.max_minutes = 1_440;
    settings.jobs.default_minutes = 1_440;
    settings.jobs.max_running = 64;
    settings.jobs.max_running_daemon = 256;
    settings.jobs.max_wakes_per_hour = 0;
    settings.jobs.max_wakes_per_hour_daemon = 0;
    settings.jobs.max_wakes_per_watch = 0;
    settings.jobs.nice = 0;
    assert_eq!(check(&settings), Ok(()), "0 turns wakes off, and each top is allowed");

    let path = Path::new("/home/u/.config/efr/config.toml");
    let off = Settings::parse(path, Some("[jobs]\nenabled = false\nnice = 10\n")).unwrap();
    assert!(!off.jobs.enabled);
    assert_eq!(off.jobs.nice, 10);
    assert!(
        Settings::parse(path, Some("[jobs]\nmax_jobs = 3\n")).is_err(),
        "an unknown key of [jobs] is an error"
    );
}

#[test]
fn jobs_default_minutes_at_most_max_minutes() {
    let mut settings = Settings::default();
    settings.jobs.max_minutes = 30;
    let refused = check(&settings).unwrap_err();
    assert_eq!(refused.key, "jobs.default_minutes");
    assert_eq!(refused.value, "60");
    settings.jobs.default_minutes = 30;
    assert_eq!(check(&settings), Ok(()));
    settings.jobs.default_minutes = 1;
    assert_eq!(check(&settings), Ok(()));
}

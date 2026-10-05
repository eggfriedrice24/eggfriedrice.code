use std::collections::BTreeMap;
use std::path::Path;

use pretty_assertions::assert_eq;

use super::shell_env;
use crate::ShellConfig;

fn config(vars: &[(&str, &str)]) -> ShellConfig {
    let env = vars.iter().map(|(name, value)| ((*name).to_owned(), (*value).to_owned())).collect();
    ShellConfig::new("/run/user/1000/efr/zsh", env)
}

#[test]
fn the_users_variables_are_inherited() {
    let env = shell_env(
        &config(&[("PATH", "/usr/bin"), ("HOME", "/home/u"), ("LANG", "C.UTF-8")]),
        Path::new("/home/u/p"),
        true,
    );
    assert_eq!(env["PATH"], "/usr/bin");
    assert_eq!(env["HOME"], "/home/u");
    assert_eq!(env["LANG"], "C.UTF-8");
}

#[test]
fn daemon_and_terminal_variables_are_removed() {
    let env = shell_env(
        &config(&[
            ("NOTIFY_SOCKET", "/run/systemd/notify"),
            ("JOURNAL_STREAM", "8:1234"),
            ("EFR_DATA_DIR", "/tmp/d"),
            ("EFR_LOG", "debug"),
            ("GHOSTTY_RESOURCES_DIR", "/usr/share/ghostty"),
            ("TERM_PROGRAM", "ghostty"),
            ("TMUX", "/tmp/tmux-1000/default,1,0"),
            ("COLUMNS", "80"),
            ("SHLVL", "2"),
            ("KEEP", "1"),
        ]),
        Path::new("/"),
        true,
    );
    let names: Vec<&str> = env.keys().map(String::as_str).collect();
    assert_eq!(
        names,
        [
            "AWS_PAGER",
            "BAT_PAGER",
            "COLORTERM",
            "EDITOR",
            "EFR_HIDDEN_SHELL",
            "GH_PAGER",
            "GIT_EDITOR",
            "GIT_PAGER",
            "GIT_SEQUENCE_EDITOR",
            "KEEP",
            "MANPAGER",
            "PAGER",
            "PWD",
            "SUDO_EDITOR",
            "SYSTEMD_EDITOR",
            "SYSTEMD_PAGER",
            "TERM",
            "VISUAL",
            "ZDOTDIR"
        ],
        "{env:?}"
    );
}

#[test]
fn efr_sets_the_terminal_the_directory_and_the_marker() {
    let env = shell_env(
        &config(&[("TERM", "xterm-ghostty"), ("COLORTERM", "24bit")]),
        Path::new("/etc"),
        true,
    );
    assert_eq!(env["TERM"], "xterm-256color");
    assert_eq!(env["COLORTERM"], "truecolor");
    assert_eq!(env["PWD"], "/etc");
    assert_eq!(env["EFR_HIDDEN_SHELL"], "1");
}

#[test]
fn the_shim_takes_zdotdir_and_keeps_the_users_value() {
    let env = shell_env(&config(&[("ZDOTDIR", "/home/u/.config/zsh")]), Path::new("/"), true);
    assert_eq!(env["ZDOTDIR"], "/run/user/1000/efr/zsh");
    assert_eq!(env["_EFR_USER_ZDOTDIR"], "/home/u/.config/zsh");
}

#[test]
fn without_a_user_zdotdir_the_shim_leaves_no_saved_value() {
    let env = shell_env(&config(&[]), Path::new("/"), true);
    assert_eq!(env["ZDOTDIR"], "/run/user/1000/efr/zsh");
    assert!(!env.contains_key("_EFR_USER_ZDOTDIR"));
}

#[test]
fn a_shell_without_integration_keeps_the_users_zdotdir() {
    let env = shell_env(&config(&[("ZDOTDIR", "/home/u/.zsh")]), Path::new("/"), false);
    assert_eq!(env["ZDOTDIR"], "/home/u/.zsh");
    assert!(!env.contains_key("_EFR_USER_ZDOTDIR"));
}

#[test]
fn a_saved_zdotdir_from_the_base_environment_is_not_trusted() {
    // A stale value from an outer efr shell must not redirect this one's startup files.
    let mut vars = BTreeMap::new();
    vars.insert("_EFR_USER_ZDOTDIR".to_owned(), "/elsewhere".to_owned());
    let env = shell_env(&ShellConfig::new("/z", vars), Path::new("/"), true);
    assert!(!env.contains_key("_EFR_USER_ZDOTDIR"));
}

#[test]
fn every_pager_is_cat_whatever_the_user_set() {
    let env = shell_env(
        &config(&[("PAGER", "less"), ("GIT_PAGER", "delta"), ("MANPAGER", "nvim +Man!")]),
        Path::new("/"),
        false,
    );
    assert_eq!(env["PAGER"], "cat");
    assert_eq!(env["GIT_PAGER"], "cat");
    assert_eq!(env["SYSTEMD_PAGER"], "cat");
    assert_eq!(env["MANPAGER"], "cat");
    assert_eq!(env["AWS_PAGER"], "");
    assert_eq!(env["GH_PAGER"], "cat");
    assert_eq!(env["BAT_PAGER"], "cat");
}

#[test]
fn every_editor_is_the_stub_whatever_the_user_set() {
    for integration in [true, false] {
        let env = shell_env(
            &config(&[("EDITOR", "nvim"), ("VISUAL", "code --wait"), ("GIT_EDITOR", "vim")]),
            Path::new("/"),
            integration,
        );
        for name in [
            "EDITOR",
            "VISUAL",
            "GIT_EDITOR",
            "GIT_SEQUENCE_EDITOR",
            "SUDO_EDITOR",
            "SYSTEMD_EDITOR",
        ] {
            assert_eq!(env[name], "/run/user/1000/efr/zsh/efr-editor", "{name}");
        }
    }
}

#[test]
fn the_trusted_programs_reach_a_zsh_with_the_integration_only() {
    let mut config = config(&[("_EFR_HS_TRUSTED_PROGRAMS", "inherited")]);
    let without = shell_env(&config, Path::new("/"), true);
    assert!(!without.contains_key("_EFR_HS_TRUSTED_PROGRAMS"), "{without:?}");

    config.trusted_programs = vec!["ls".to_owned(), "git".to_owned()];
    let zsh = shell_env(&config, Path::new("/"), true);
    assert_eq!(zsh["_EFR_HS_TRUSTED_PROGRAMS"], "ls git");
    let other = shell_env(&config, Path::new("/"), false);
    assert!(!other.contains_key("_EFR_HS_TRUSTED_PROGRAMS"), "{other:?}");
}

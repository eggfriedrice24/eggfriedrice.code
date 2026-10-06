use std::collections::BTreeMap;
use std::time::Duration;

use super::ShellConfig;

#[test]
fn debug_names_the_environment_without_its_values() {
    let env = BTreeMap::from([("OPENAI_API_KEY".to_owned(), "sk-secret".to_owned())]);
    let config = ShellConfig::new("/run/user/1000/efr/zsh", env);
    let debug = format!("{config:?}");
    assert!(debug.contains("OPENAI_API_KEY"), "{debug}");
    assert!(!debug.contains("sk-secret"), "{debug}");
}

#[test]
fn the_defaults_start_an_interactive_login_zsh() {
    let config = ShellConfig::new("/tmp/zsh", BTreeMap::new());
    assert!(config.login);
    assert!(config.program.is_none());
    assert_eq!(config.term, "xterm-256color");
    assert_eq!(config.size, ShellConfig::DEFAULT_SIZE);
}

#[test]
fn visible_input_needs_a_longer_quiet_than_hidden_input() {
    let config = ShellConfig::new("/tmp/zsh", BTreeMap::new());
    assert_eq!(config.quiet_period, Duration::from_secs(1));
    assert_eq!(config.visible_input_quiet, Duration::from_secs(3));
    assert!(format!("{config:?}").contains("visible_input_quiet: 3s"));
}

#[test]
fn a_visible_prompt_that_reads_like_a_question_needs_the_shortest_quiet() {
    let config = ShellConfig::new("/tmp/zsh", BTreeMap::new());
    assert_eq!(config.question_input_quiet, Duration::from_millis(500));
    assert!(config.question_input_quiet < config.quiet_period);
    assert!(format!("{config:?}").contains("question_input_quiet: 500ms"));
}

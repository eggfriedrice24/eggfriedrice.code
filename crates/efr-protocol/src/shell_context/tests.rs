use pretty_assertions::assert_eq;
use serde_json::json;

use crate::ShellContext;

fn full() -> ShellContext {
    ShellContext {
        pwd: "/etc".into(),
        oldpwd: Some("/home/me".into()),
        tty: Some("/dev/pts/3".into()),
        shell_pid: Some(4242),
        last_status: Some(1),
        shlvl: Some(1),
        ssh_connection: None,
        hostname: Some("desk".into()),
    }
}

#[test]
fn a_last_command_sent_inside_a_context_is_dropped() {
    let context: ShellContext = serde_json::from_value(json!({
        "pwd": "/etc",
        "last_status": 0,
        "last_command": "export TOKEN=hunter2",
    }))
    .unwrap();
    let text = serde_json::to_string(&context).unwrap();
    assert!(!text.contains("hunter2"), "{text}");
    assert!(!text.contains("last_command"), "{text}");
    assert!(!format!("{context:?}").contains("hunter2"));
}

#[test]
fn a_context_with_only_a_pwd_is_a_one_member_object() {
    assert_eq!(serde_json::to_value(ShellContext::new("/etc")).unwrap(), json!({ "pwd": "/etc" }));
}

#[test]
fn missing_optional_members_read_as_none() {
    let context: ShellContext = serde_json::from_value(json!({ "pwd": "/etc" })).unwrap();
    assert_eq!(context, ShellContext::new("/etc"));
}

#[test]
fn the_pwd_is_required() {
    assert!(serde_json::from_value::<ShellContext>(json!({ "tty": "/dev/pts/1" })).is_err());
}

#[test]
fn unknown_members_are_ignored() {
    let context: ShellContext =
        serde_json::from_value(json!({ "pwd": "/etc", "future": [1, 2] })).unwrap();
    assert_eq!(context, ShellContext::new("/etc"));
}

#[test]
fn a_full_context_reads_back_as_itself() {
    let back: ShellContext = serde_json::from_value(serde_json::to_value(full()).unwrap()).unwrap();
    assert_eq!(back, full());
}

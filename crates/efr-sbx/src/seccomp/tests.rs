use efr_sandbox::{EAFNOSUPPORT, ENOSYS, EPERM, TIOCSTI};

use super::*;

#[test]
fn rules_are_grouped_by_errno() {
    let json = filters_json(&SeccompProfile::phase1(), SeccompArch::X86_64).unwrap();
    let names: Vec<&String> = json.as_object().unwrap().keys().collect();
    assert_eq!(
        names,
        [format!("errno_{EPERM}"), format!("errno_{ENOSYS}"), format!("errno_{EAFNOSUPPORT}")]
            .iter()
            .collect::<Vec<_>>()
    );
    let enosys = &json[format!("errno_{ENOSYS}")]["filter"];
    assert_eq!(enosys, &json!([{ "syscall": "clone3" }]));
}

#[test]
fn ioctl_rule_compares_the_low_word_of_the_request() {
    let json = filters_json(&SeccompProfile::phase1(), SeccompArch::X86_64).unwrap();
    let rules = json[format!("errno_{EPERM}")]["filter"].as_array().unwrap();
    let ioctl = rules.iter().find(|rule| rule["syscall"] == "ioctl").unwrap();
    assert_eq!(
        ioctl["args"][0],
        json!({ "index": 1, "type": "dword", "op": "eq", "val": TIOCSTI })
    );
}

#[test]
fn socket_rule_lists_every_allowed_family_as_not_equal() {
    let json = filters_json(&SeccompProfile::phase1(), SeccompArch::X86_64).unwrap();
    let rules = json[format!("errno_{EAFNOSUPPORT}")]["filter"].as_array().unwrap();
    assert_eq!(rules.len(), 2);
    assert_eq!(rules[0]["args"].as_array().unwrap().len(), 4);
}

#[test]
fn clone_rule_has_one_condition_per_namespace_flag() {
    let json = filters_json(&SeccompProfile::phase1(), SeccompArch::X86_64).unwrap();
    let rules = json[format!("errno_{EPERM}")]["filter"].as_array().unwrap();
    let clones = rules.iter().filter(|rule| rule["syscall"] == "clone").count();
    assert_eq!(clones, 8);
}

#[test]
fn port_calls_are_left_out_on_aarch64_only() {
    let x86 = filters_json(&SeccompProfile::phase1(), SeccompArch::X86_64).unwrap();
    let arm = filters_json(&SeccompProfile::phase1(), SeccompArch::Aarch64).unwrap();
    let has_iopl = |json: &Value| {
        json[format!("errno_{EPERM}")]["filter"]
            .as_array()
            .unwrap()
            .iter()
            .any(|rule| rule["syscall"] == "iopl")
    };
    assert!(has_iopl(&x86));
    assert!(!has_iopl(&arm));
}

#[test]
fn the_profile_compiles_for_this_machine() {
    let programs = compile(&SeccompProfile::phase1()).unwrap();
    let expected = if std::env::consts::ARCH == "x86_64" { 4 } else { 3 };
    assert_eq!(programs.len(), expected);
    assert!(programs.iter().all(|program| !program.is_empty()));
}

#[test]
fn x32_filter_kills_only_x32_numbers() {
    let program = x32_filter();
    assert_eq!(program.len(), 6);
    assert_eq!(program[3].k, X32_SYSCALL_BIT);
    assert_eq!(program[4].k, RET_KILL_PROCESS);
    assert_eq!(program[5].k, RET_ALLOW);
}

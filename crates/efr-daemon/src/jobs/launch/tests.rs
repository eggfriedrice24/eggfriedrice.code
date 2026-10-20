use std::collections::BTreeMap;

use pretty_assertions::assert_eq;

use super::{root_vars, with_roots};

#[test]
fn root_vars_are_the_set_roots_of_efrd() {
    let env = BTreeMap::from([
        ("EFR_HOME".to_owned(), "/srv/efr".to_owned()),
        ("EFR_DATA_DIR".to_owned(), String::new()),
        ("EFR_LOG".to_owned(), "debug".to_owned()),
        ("PATH".to_owned(), "/usr/bin".to_owned()),
    ]);
    assert_eq!(root_vars(&env), [("EFR_HOME".to_owned(), "/srv/efr".to_owned())]);
    assert_eq!(root_vars(&BTreeMap::new()), []);
}

#[test]
fn exit_child_job_env_gets_the_roots_of_efrd() {
    let env = b"PATH=/usr/bin\0EFR_HOME=/home/u/other\0EFR_RUNTIME_DIR=/x\0LANG=C\0";
    let roots = [("EFR_HOME".to_owned(), "/srv/efr".to_owned())];

    let bytes = with_roots(env, &roots);

    assert_eq!(bytes, b"PATH=/usr/bin\0LANG=C\0EFR_HOME=/srv/efr\0");
    // NOTE: with no root set in efrd, the job finds the default roots, as efrd did.
    assert_eq!(with_roots(env, &[]), b"PATH=/usr/bin\0LANG=C\0");
}

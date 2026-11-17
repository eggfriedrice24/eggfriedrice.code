/// The conversation owns the tool `ask_user`, and the registry of `efr-tools` keeps
/// its name. The two crates may not depend on each other, so efrd, which links both,
/// checks that they keep the same name.
#[test]
fn the_registry_keeps_the_name_of_the_conversation_s_tool() {
    assert_eq!(efr_conversation::ASK_USER, efr_tools::ASK_USER);
}

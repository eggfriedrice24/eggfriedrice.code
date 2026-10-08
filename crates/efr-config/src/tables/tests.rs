use super::DEFAULT_SYSTEM_PROMPT;

/// The user sees one dim line per tool call, never the output, so the model must not
/// point at output as if the user had read it.
#[test]
fn the_system_prompt_says_the_user_does_not_see_tool_output() {
    assert!(DEFAULT_SYSTEM_PROMPT.contains("The user does not see your tools' output."));
    assert!(DEFAULT_SYSTEM_PROMPT.contains("For each tool call they see one dim line"));
    assert!(
        DEFAULT_SYSTEM_PROMPT.contains("never refer to output as \"above\" or \"shown\""),
        "{DEFAULT_SYSTEM_PROMPT}"
    );
}

/// The model edits files with `apply_patch`, not with `sed -i` or a whole-file write.
#[test]
fn the_system_prompt_tells_the_model_to_edit_with_apply_patch() {
    assert!(DEFAULT_SYSTEM_PROMPT.contains("To change a file, use the apply_patch tool."));
    assert!(
        DEFAULT_SYSTEM_PROMPT.contains("Do not use sed -i or perl -pi"),
        "{DEFAULT_SYSTEM_PROMPT}"
    );
    assert!(DEFAULT_SYSTEM_PROMPT.contains("use write_file only"), "{DEFAULT_SYSTEM_PROMPT}");
}

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

/// The model edits files with its edit tool, not with `sed -i` or a whole-file write.
/// A request offers each model one edit tool, `apply_patch` or `edit`, and the prompt
/// stays the same for every model, so a user's own prompt need not follow the model.
#[test]
fn the_system_prompt_tells_the_model_to_edit_with_the_edit_tool_that_it_has() {
    assert!(
        DEFAULT_SYSTEM_PROMPT
            .contains("To change a file, use the edit tool that you have: apply_patch or edit."),
        "{DEFAULT_SYSTEM_PROMPT}"
    );
    assert!(
        DEFAULT_SYSTEM_PROMPT.contains("Do not use sed -i or perl -pi"),
        "{DEFAULT_SYSTEM_PROMPT}"
    );
    assert!(DEFAULT_SYSTEM_PROMPT.contains("use write_file only"), "{DEFAULT_SYSTEM_PROMPT}");
}

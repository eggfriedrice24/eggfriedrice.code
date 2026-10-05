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

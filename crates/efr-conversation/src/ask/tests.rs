use efr_protocol::{QuestionOption, QuestionReply};
use efr_provider::EditTool;
use pretty_assertions::assert_eq;
use serde_json::{Value, json};

use super::{ASK_USER, Question, SKIPPED, definition, definitions, refusal, replied, unanswered};
use crate::testing::FakeToolbox;
use crate::{ConversationConfig, Toolbox as _};

fn option(label: &str) -> Value {
    json!({ "label": label })
}

fn refused(input: &Value) -> String {
    Question::parse(input).expect_err("the input is refused")
}

#[test]
fn a_question_with_two_to_four_options_passes() {
    let input = json!({
        "question": "Which database?",
        "options": [
            { "label": " Postgres ", "description": "Recommended: the team knows it." },
            { "label": "SQLite", "description": "  " },
        ],
    });

    let question = Question::parse(&input).unwrap();

    assert_eq!(question.question, "Which database?");
    assert_eq!(
        question.options,
        vec![
            QuestionOption {
                label: "Postgres".to_owned(),
                description: Some("Recommended: the team knows it.".to_owned()),
            },
            QuestionOption { label: "SQLite".to_owned(), description: None },
        ],
        "a label loses its blanks at the ends, and a blank description is none"
    );
}

#[test]
fn a_bad_input_is_refused_and_nobody_is_asked() {
    let two = [option("a"), option("b")];
    let cases = [
        (json!({ "question": "Q?" }), "the input must be an object"),
        (json!({ "question": "Q?", "options": two, "multi": true }), "the input must be"),
        (json!({ "question": " ", "options": two }), "the question is empty."),
        (
            json!({ "question": "x".repeat(301), "options": two }),
            "the question is longer than 300 characters.",
        ),
        (json!({ "question": "Q?", "options": [option("a")] }), "give 2 to 4 options, not 1."),
        (
            json!({ "question": "Q?", "options": [option("a"), option("b"), option("c"),
                option("d"), option("e")] }),
            "give 2 to 4 options, not 5.",
        ),
        (json!({ "question": "Q?", "options": [option("a"), option(" ")] }), "option 2 has no"),
        (
            json!({ "question": "Q?", "options": [option("a\nb"), option("c")] }),
            "the label of option 1 is not one line.",
        ),
        (
            json!({ "question": "Q?", "options": [option(&"y".repeat(61)), option("c")] }),
            "the label of option 1 is longer than 60 characters.",
        ),
        (
            json!({ "question": "Q?", "options": [option("a"), option("b"), option(" a")] }),
            "option 3 has the same label as option 1.",
        ),
        (
            json!({ "question": "Q?", "options": [option("a"),
                { "label": "b", "description": "z".repeat(201) }] }),
            "the description of option 2 is longer than 200 characters.",
        ),
    ];
    for (input, wanted) in cases {
        let text = refused(&input);
        assert!(text.starts_with("The question was not asked: "), "{text}");
        assert!(text.contains(wanted), "{input}: {text}");
    }
}

#[test]
fn the_limits_count_characters_not_bytes() {
    let input = json!({
        "question": "\u{e9}".repeat(300),
        "options": [option(&"\u{fc}".repeat(60)), option("b")],
    });
    assert!(Question::parse(&input).is_ok());
}

#[test]
fn a_reply_outside_the_question_is_refused() {
    assert_eq!(refusal(&QuestionReply::Option { index: 0 }, 2), None);
    assert_eq!(refusal(&QuestionReply::Option { index: 1 }, 2), None);
    assert!(refusal(&QuestionReply::Option { index: 2 }, 2).is_some(), "index is from 0");
    assert_eq!(refusal(&QuestionReply::Skipped, 2), None);
    assert_eq!(refusal(&QuestionReply::Text { text: "use port 8080".to_owned() }, 2), None);
    let long = "x".repeat(QuestionReply::MAX_TEXT_BYTES + 1);
    let longest = "x".repeat(QuestionReply::MAX_TEXT_BYTES);
    assert_eq!(refusal(&QuestionReply::Text { text: longest }, 2), None);
    assert_eq!(
        refusal(&QuestionReply::Text { text: long.clone() }, 2),
        Some(format!("the text is longer than {} bytes", QuestionReply::MAX_TEXT_BYTES).as_str()),
        "the reason names the limit of the wire"
    );
    for text in [String::new(), "  ".to_owned(), long, "a\nb".to_owned(), "\u{1b}[2J".to_owned()] {
        assert!(refusal(&QuestionReply::Text { text: text.clone() }, 2).is_some(), "{text:?}");
    }
    assert!(refusal(&QuestionReply::Unknown, 2).is_some());
}

#[test]
fn the_model_reads_each_answer_as_the_spec_says() {
    let options = [
        QuestionOption { label: "SQLite".to_owned(), description: None },
        QuestionOption { label: "Postgres".to_owned(), description: None },
    ];
    assert_eq!(
        replied(&QuestionReply::Option { index: 1 }, &options),
        "The user chose option 2: \"Postgres\"."
    );
    assert_eq!(
        replied(&QuestionReply::Text { text: "neither, use files".to_owned() }, &options),
        "The user answered in their own words: \"neither, use files\""
    );
    assert_eq!(replied(&QuestionReply::Skipped, &options), SKIPPED);
    assert_eq!(
        SKIPPED,
        "The user skipped the question. Do not ask it again in this turn. Go on with a choice \
         that is safe and easy to undo, or stop and ask in your final answer."
    );
    assert_eq!(
        unanswered(false),
        "Nobody answered the question. Do not ask again in this turn. Do only what is safe \
         and easy to undo, and name your choice in your final answer."
    );
    assert_eq!(
        unanswered(true),
        "Nobody answered the question (the user is away). Do not ask again in this turn. Do \
         only what is safe and easy to undo, and name your choice in your final answer."
    );
}

#[test]
fn the_definition_follows_the_schema_of_the_spec() {
    let tool = definition();
    assert_eq!(tool.name, ASK_USER);
    assert_eq!(tool.name, "ask_user");
    assert!(tool.grammar.is_none());
    assert_eq!(tool.input_schema["required"], json!(["question", "options"]));
    assert_eq!(tool.input_schema["properties"]["question"]["maxLength"], json!(300));
    let options = &tool.input_schema["properties"]["options"];
    assert_eq!((options["minItems"].clone(), options["maxItems"].clone()), (json!(2), json!(4)));
    assert_eq!(options["items"]["properties"]["label"]["maxLength"], json!(60));
    assert_eq!(options["items"]["properties"]["description"]["maxLength"], json!(200));
    assert!(tool.description.starts_with("Ask the user one question and wait for the answer."));
    assert!(tool.description.contains("an answer approves nothing"));
}

#[test]
fn the_config_switch_adds_the_tool_after_the_toolbox() {
    let toolbox = FakeToolbox::default();
    let mut config = ConversationConfig::new("m", "/scratch");
    let on = definitions(&toolbox, EditTool::ApplyPatch, &config);
    config.ask_user = false;
    let off = definitions(&toolbox, EditTool::ApplyPatch, &config);

    assert_eq!(off, toolbox.definitions(EditTool::ApplyPatch));
    assert_eq!(on.len(), off.len() + 1);
    assert_eq!(on.last().map(|tool| tool.name.as_str()), Some(ASK_USER));
}

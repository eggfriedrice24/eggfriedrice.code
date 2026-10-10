use std::collections::VecDeque;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use efr_config::Settings;
use efr_conversation::{ExitJudge, JudgeInput, JudgeOutcome};
use efr_protocol::{
    ActionFacts, CallId, ChainStep, ConversationId, ExitFacts, ExitKind, ExitRecord, ExitSource,
    Judgement, Risk, Scope, TurnId, UserAuthorization, Verdict,
};
use efr_provider::{
    ModelInfo, Provider, ProviderError, ProviderEvent, ProviderId, ProviderStream, Request,
    StopReason,
};
use efr_stdx::id::uuid_v7;
use efr_test_support::{TestClock, TestRng};
use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use tokio::sync::watch;

use super::{ClassifierJudge, MAX_CATEGORY_CHARS, MAX_RATIONALE_CHARS, PROMPT, SCHEMA_NAME, parse};
use crate::catalog::Vendor;

mod wire;

/// The small model of the OpenAI provider, the first step of the chain.
const SMALL: &str = "gpt-6-luna";

/// The turn's model, the second step of the chain.
const TURN: &str = "gpt-6.1-sol";

/// An answer that allows.
const ALLOW: &str = r#"{"verdict":"allow","risk":"low","user_authorization":"high","category":"fits the request","rationale":"the user asked to copy the report"}"#;

/// What a scripted provider answers to one request.
enum Reply {
    /// The text of the answer, then the end of the turn.
    Text(&'static str),
    /// An error before any event.
    Error(fn() -> ProviderError),
    /// No answer at all, until the request is dropped.
    Hang,
    /// A refusal text, stopped for its content.
    Filtered,
}

/// A provider that answers each request with the next scripted reply and keeps every
/// request.
#[derive(Debug)]
struct Scripted {
    id: ProviderId,
    models: Vec<ModelInfo>,
    replies: Mutex<VecDeque<Reply>>,
    requests: Mutex<Vec<Request>>,
}

impl std::fmt::Debug for Reply {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Reply::Text(text) => write!(f, "Text({text})"),
            Reply::Error(_) => f.write_str("Error"),
            Reply::Hang => f.write_str("Hang"),
            Reply::Filtered => f.write_str("Filtered"),
        }
    }
}

impl Scripted {
    fn new(replies: impl IntoIterator<Item = Reply>) -> Arc<Self> {
        Self::listing(Vec::new(), replies)
    }

    fn listing(models: Vec<ModelInfo>, replies: impl IntoIterator<Item = Reply>) -> Arc<Self> {
        Arc::new(Scripted {
            id: ProviderId::new("test").unwrap(),
            models,
            replies: Mutex::new(replies.into_iter().collect()),
            requests: Mutex::new(Vec::new()),
        })
    }

    fn requests(&self) -> Vec<Request> {
        self.requests.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    fn models_asked(&self) -> Vec<String> {
        self.requests().into_iter().map(|request| request.model).collect()
    }
}

#[async_trait]
impl Provider for Scripted {
    fn id(&self) -> &ProviderId {
        &self.id
    }

    fn models(&self) -> Vec<ModelInfo> {
        self.models.clone()
    }

    async fn stream(&self, request: Request) -> Result<ProviderStream, ProviderError> {
        self.requests.lock().unwrap_or_else(PoisonError::into_inner).push(request);
        let reply = self.replies.lock().unwrap_or_else(PoisonError::into_inner).pop_front();
        let done = |stop_reason| Ok(ProviderEvent::Done { stop_reason, provider_raw: None });
        let events = match reply {
            Some(Reply::Text(text)) => {
                vec![
                    Ok(ProviderEvent::TextDelta { text: text.to_owned() }),
                    done(StopReason::EndTurn),
                ]
            }
            Some(Reply::Error(error)) => return Err(error()),
            Some(Reply::Hang) => return Ok(Box::pin(futures::stream::pending())),
            Some(Reply::Filtered) => vec![
                Ok(ProviderEvent::TextDelta { text: "I can't help with that.".to_owned() }),
                done(StopReason::ContentFilter),
            ],
            None => panic!("a request after the last scripted reply"),
        };
        Ok(Box::pin(futures::stream::iter(events)))
    }
}

/// The parts of one judge test.
struct Setup {
    provider: Arc<Scripted>,
    judge: ClassifierJudge,
    clock: TestClock,
    settings: watch::Sender<Arc<Settings>>,
}

fn setup(provider: Arc<Scripted>) -> Setup {
    setup_with(provider, |_| {})
}

fn setup_with(provider: Arc<Scripted>, change: impl FnOnce(&mut Settings)) -> Setup {
    let mut settings = Settings::default();
    change(&mut settings);
    let (sender, receiver) = watch::channel(Arc::new(settings));
    let clock = TestClock::new();
    let judge = ClassifierJudge::new(
        Arc::clone(&provider) as Arc<dyn Provider>,
        receiver,
        Vendor::OpenAi,
        clock.shared(),
    );
    Setup { provider, judge, clock, settings: sender }
}

fn record() -> ExitRecord {
    ExitRecord {
        version: ExitRecord::VERSION,
        user_messages: vec!["copy the report to ~/Documents".to_owned()],
        action: ActionFacts {
            tool: "shell".to_owned(),
            line: "cp report.pdf ~/Documents/".to_owned(),
            cwd: "/home/u/p/app".into(),
            scope: Scope::Machine,
            exits: vec![ExitKind::Write],
            grants: Vec::new(),
            source: ExitSource::Predicted,
            edit: None,
        },
        facts: ExitFacts::default(),
    }
}

/// The ids of one exit: the same conversation and turn for the same seed.
struct Ids {
    conversation: ConversationId,
    turn: TurnId,
    call: CallId,
}

fn ids(seed: u64) -> Ids {
    let clock = TestClock::new();
    let id = |offset: u64| uuid_v7(&clock, &TestRng::new(seed * 10 + offset));
    Ids {
        conversation: ConversationId::from_uuid(id(1)),
        turn: TurnId::from_uuid(id(2)),
        call: CallId::from_uuid(id(3)),
    }
}

fn input<'a>(ids: &Ids, record: &'a ExitRecord) -> JudgeInput<'a> {
    JudgeInput {
        conversation_id: ids.conversation,
        turn_id: ids.turn,
        call_id: ids.call,
        turn_model: TURN,
        turn_effort: Some("high"),
        record,
    }
}

async fn judge(setup: &Setup, record: &ExitRecord) -> JudgeOutcome {
    setup.judge.judge(input(&ids(1), record)).await
}

fn allowed() -> Judgement {
    Judgement {
        verdict: Verdict::Allow,
        risk: Risk::Low,
        user_authorization: UserAuthorization::High,
        category: "fits the request".to_owned(),
        rationale: "the user asked to copy the report".to_owned(),
    }
}

fn unknown_model() -> ProviderError {
    ProviderError::UnknownModel { model: SMALL.to_owned() }
}

fn server_error() -> ProviderError {
    ProviderError::api(Some(500), Some("server_error".to_owned()), "down".to_owned())
}

/// The model on offer with the efforts `efforts`.
fn model(id: &str, efforts: &[&str]) -> ModelInfo {
    ModelInfo::new(id).with_efforts(efforts.iter().copied(), None)
}

#[tokio::test]
async fn the_classifier_model_answers_first() {
    let setup = setup(Scripted::new([Reply::Text(ALLOW)]));
    let record = record();
    let outcome = judge(&setup, &record).await;
    assert_eq!(outcome.judgement, Some(allowed()));
    assert_eq!(outcome.model.as_deref(), Some(SMALL));
    assert_eq!(outcome.chain_step, ChainStep::ClassifierModel);
    assert_eq!(outcome.skipped, Vec::<String>::new());
    assert!(!outcome.cached);
    assert_eq!(outcome.record_sha256.as_ref().map(String::len), Some(64));
    assert_eq!(setup.provider.models_asked(), [SMALL]);
}

#[tokio::test]
async fn the_request_holds_the_prompt_the_record_and_the_schema() {
    let setup = setup(Scripted::new([Reply::Text(ALLOW)]));
    let record = record();
    judge(&setup, &record).await;
    let request = setup.provider.requests().remove(0);
    assert_eq!(request.system.as_deref(), Some(PROMPT));
    assert_eq!(request.messages.len(), 1);
    let sent: ExitRecord = serde_json::from_str(&request.messages[0].text()).unwrap();
    assert_eq!(sent, record);
    assert!(request.tools.is_empty());
    assert!(request.side_call);
    assert!(request.one_shot);
    let output = request.output.unwrap();
    assert_eq!(output.name, SCHEMA_NAME);
    assert_eq!(output.schema["additionalProperties"], json!(false));
    assert_eq!(
        output.schema["required"],
        json!(["verdict", "risk", "user_authorization", "category", "rationale"])
    );
    assert_eq!(request.effort.as_deref(), Some("low"));
    assert_eq!(request.provider_options.get("reasoning_summary"), Some(&Value::Null));
}

#[test]
fn the_schema_has_no_length_keyword() {
    fn lengths(value: &Value, found: &mut Vec<String>) {
        match value {
            Value::Object(members) => {
                for (key, member) in members {
                    if key.contains("Length") {
                        found.push(key.clone());
                    }
                    lengths(member, found);
                }
            }
            Value::Array(items) => items.iter().for_each(|item| lengths(item, found)),
            _ => {}
        }
    }
    let mut found = Vec::new();
    lengths(&super::schema(), &mut found);
    assert_eq!(found, Vec::<String>::new());
}

#[tokio::test]
async fn classifier_request_has_no_max_output_tokens() {
    let setup = setup(Scripted::new([Reply::Error(server_error), Reply::Text(ALLOW)]));
    let record = record();
    judge(&setup, &record).await;
    let requests = setup.provider.requests();
    assert_eq!(requests.len(), 2);
    assert!(requests.iter().all(|request| request.max_output_tokens.is_none()));
}

#[tokio::test]
async fn classifier_request_has_no_cache_key() {
    let setup = setup(Scripted::new([Reply::Error(server_error), Reply::Text(ALLOW)]));
    let record = record();
    judge(&setup, &record).await;
    for request in setup.provider.requests() {
        assert_eq!(request.provider_options.get("prompt_cache_key"), None, "{}", request.model);
    }
}

#[tokio::test]
async fn classifier_effort_left_out_when_model_lacks_it() {
    let models = vec![model(SMALL, &["medium", "high"]), model(TURN, &["low", "high"])];
    let setup = setup(Scripted::listing(models, [Reply::Error(server_error), Reply::Text(ALLOW)]));
    let record = record();
    judge(&setup, &record).await;
    let efforts: Vec<Option<String>> =
        setup.provider.requests().into_iter().map(|request| request.effort).collect();
    assert_eq!(efforts, [None, Some("low".to_owned())]);
}

#[tokio::test]
async fn an_effort_goes_to_a_model_that_lists_none() {
    let models = vec![model(SMALL, &[])];
    let setup = setup_with(Scripted::listing(models, [Reply::Text(ALLOW)]), |settings| {
        settings.classifier.effort = "medium".to_owned();
    });
    let record = record();
    judge(&setup, &record).await;
    assert_eq!(setup.provider.requests()[0].effort.as_deref(), Some("medium"));
}

#[tokio::test]
async fn classifier_error_asks() {
    let setup = setup(Scripted::new([Reply::Error(server_error), Reply::Error(server_error)]));
    let record = record();
    let outcome = judge(&setup, &record).await;
    assert_eq!(outcome.judgement, None);
    assert_eq!(outcome.model, None);
    assert_eq!(outcome.chain_step, ChainStep::User);
    assert_eq!(outcome.skipped, [format!("{SMALL}: error"), format!("{TURN}: error")]);
}

#[tokio::test]
async fn classifier_timeout_asks() {
    let setup = setup(Scripted::new([Reply::Hang, Reply::Hang]));
    let record = record();
    let judging = judge(&setup, &record);
    let clock = &setup.clock;
    let drive = async {
        for _ in 0..2 {
            clock.wait_for_sleeps(1).await;
            clock.advance(Duration::from_secs(20));
        }
    };
    let (outcome, ()) = tokio::join!(judging, drive);
    assert_eq!(outcome.judgement, None);
    assert_eq!(outcome.chain_step, ChainStep::User);
    assert_eq!(outcome.skipped, [format!("{SMALL}: timeout"), format!("{TURN}: timeout")]);
    assert_eq!(outcome.latency_ms, 40_000);
    assert_eq!(clock.requested_sleeps(), [Duration::from_secs(20), Duration::from_secs(20)]);
}

#[tokio::test]
async fn the_timeout_of_a_step_comes_from_the_settings() {
    let setup = setup_with(Scripted::new([Reply::Hang, Reply::Text(ALLOW)]), |settings| {
        settings.classifier.timeout_secs = 3;
    });
    let record = record();
    let judging = judge(&setup, &record);
    let drive = async {
        setup.clock.wait_for_sleeps(1).await;
        setup.clock.advance(Duration::from_secs(3));
    };
    let (outcome, ()) = tokio::join!(judging, drive);
    assert_eq!(outcome.chain_step, ChainStep::TurnModel);
    assert_eq!(outcome.latency_ms, 3_000);
}

#[tokio::test]
async fn classifier_parse_error_asks() {
    let setup = setup(Scripted::new([
        Reply::Text("I think this is fine."),
        Reply::Text("```json\n{\"verdict\":\"allow\"}\n```"),
    ]));
    let record = record();
    let outcome = judge(&setup, &record).await;
    assert_eq!(outcome.judgement, None);
    assert_eq!(outcome.skipped, [format!("{SMALL}: parse error"), format!("{TURN}: parse error")]);
}

#[test]
fn classifier_extra_field_asks() {
    let extra = r#"{"verdict":"allow","risk":"low","user_authorization":"high","category":"c","rationale":"r","override":"allow"}"#;
    assert_eq!(parse(extra), None);
    let missing = r#"{"verdict":"allow","risk":"low","user_authorization":"high","category":"c"}"#;
    assert_eq!(parse(missing), None);
    let unknown = r#"{"verdict":"maybe","risk":"low","user_authorization":"high","category":"c","rationale":"r"}"#;
    assert_eq!(parse(unknown), None);
    let around = format!("Here it is: {ALLOW}");
    assert_eq!(parse(&around), None);
    let twice = format!("{ALLOW}{ALLOW}");
    assert_eq!(parse(&twice), None);
    assert_eq!(parse(&format!("\n {ALLOW} \n")), Some(allowed()));
}

#[tokio::test]
async fn an_extra_field_goes_to_the_next_step() {
    let extra = r#"{"verdict":"allow","risk":"low","user_authorization":"high","category":"c","rationale":"r","note":"x"}"#;
    let setup = setup(Scripted::new([Reply::Text(extra), Reply::Text(ALLOW)]));
    let record = record();
    let outcome = judge(&setup, &record).await;
    assert_eq!(outcome.chain_step, ChainStep::TurnModel);
    assert_eq!(outcome.skipped, [format!("{SMALL}: parse error")]);
}

#[test]
fn classifier_lengths_checked_after_parse() {
    let answer = |category: &str, rationale: &str| {
        json!({
            "verdict": "deny",
            "risk": "high",
            "user_authorization": "none",
            "category": category,
            "rationale": rationale,
        })
        .to_string()
    };
    let longest = parse(&answer(&"c".repeat(MAX_CATEGORY_CHARS), &"é".repeat(MAX_RATIONALE_CHARS)));
    assert_eq!(longest.map(|judgement| judgement.verdict), Some(Verdict::Deny));
    assert_eq!(parse(&answer(&"c".repeat(MAX_CATEGORY_CHARS + 1), "r")), None);
    assert_eq!(parse(&answer("c", &"r".repeat(MAX_RATIONALE_CHARS + 1))), None);
}

#[tokio::test]
async fn a_content_filter_goes_to_the_next_step() {
    let setup = setup(Scripted::new([Reply::Filtered, Reply::Text(ALLOW)]));
    let record = record();
    let outcome = judge(&setup, &record).await;
    assert_eq!(outcome.chain_step, ChainStep::TurnModel);
    assert_eq!(outcome.skipped, [format!("{SMALL}: content filter")]);
}

#[tokio::test]
async fn classifier_chain_falls_back_to_turn_model() {
    let setup = setup(Scripted::new([Reply::Error(unknown_model), Reply::Text(ALLOW)]));
    let record = record();
    let outcome = judge(&setup, &record).await;
    assert_eq!(outcome.judgement, Some(allowed()));
    assert_eq!(outcome.model.as_deref(), Some(TURN));
    assert_eq!(outcome.chain_step, ChainStep::TurnModel);
    assert_eq!(outcome.skipped, [format!("{SMALL}: missing")]);
    assert_eq!(setup.provider.models_asked(), [SMALL, TURN]);
}

#[tokio::test]
async fn classifier_missing_model_never_denies_every_exit() {
    // The provider lists its models, and the small model is not one of them: no
    // request goes to it, and the turn's model decides.
    let models = vec![model(TURN, &["low"])];
    let setup = setup(Scripted::listing(models, [Reply::Text(ALLOW), Reply::Text(ALLOW)]));
    let record = record();
    for seed in [1, 2] {
        let outcome = setup.judge.judge(input(&ids(seed), &record)).await;
        assert_eq!(outcome.judgement, Some(allowed()));
        assert_eq!(outcome.chain_step, ChainStep::TurnModel);
        assert_eq!(outcome.skipped, [format!("{SMALL}: missing")]);
    }
    assert_eq!(setup.provider.models_asked(), [TURN, TURN]);
}

#[tokio::test]
async fn a_model_that_the_config_names_is_tried_without_the_list() {
    let models = vec![model(TURN, &["low"])];
    let setup = setup_with(Scripted::listing(models, [Reply::Text(ALLOW)]), |settings| {
        settings.classifier.model = Some("codex-auto-review".to_owned());
    });
    let record = record();
    let outcome = judge(&setup, &record).await;
    assert_eq!(outcome.model.as_deref(), Some("codex-auto-review"));
    assert_eq!(outcome.chain_step, ChainStep::ClassifierModel);
}

#[tokio::test]
async fn classifier_chain_ends_at_user() {
    // Without the fallback, a failed first step goes to the user.
    let setup = setup_with(Scripted::new([Reply::Error(unknown_model)]), |settings| {
        settings.classifier.fallback_to_turn_model = false;
    });
    let record = record();
    let outcome = judge(&setup, &record).await;
    assert_eq!(outcome.judgement, None);
    assert_eq!(outcome.chain_step, ChainStep::User);
    assert_eq!(outcome.skipped, [format!("{SMALL}: missing")]);
    assert_eq!(setup.provider.models_asked(), [SMALL]);
}

#[tokio::test]
async fn refused_credentials_end_the_chain_at_once() {
    let unauthorized = || ProviderError::Unauthorized { message: None };
    let setup = setup(Scripted::new([Reply::Error(unauthorized)]));
    let record = record();
    let outcome = judge(&setup, &record).await;
    assert_eq!(outcome.chain_step, ChainStep::User);
    assert_eq!(outcome.skipped, [format!("{SMALL}: credentials refused")]);
    assert_eq!(setup.provider.models_asked(), [SMALL]);
}

#[tokio::test]
async fn a_used_up_quota_ends_the_chain_at_once() {
    let quota =
        || ProviderError::api(Some(429), Some("insufficient_quota".to_owned()), "quota".to_owned());
    let setup = setup(Scripted::new([Reply::Error(quota)]));
    let record = record();
    let outcome = judge(&setup, &record).await;
    assert_eq!(outcome.skipped, [format!("{SMALL}: quota")]);
    assert_eq!(setup.provider.models_asked(), [SMALL]);
}

#[tokio::test]
async fn the_turn_model_is_asked_once_when_it_is_the_classifier_model() {
    let setup = setup_with(Scripted::new([Reply::Error(server_error)]), |settings| {
        settings.classifier.model = Some(TURN.to_owned());
    });
    let record = record();
    let outcome = judge(&setup, &record).await;
    assert_eq!(outcome.chain_step, ChainStep::User);
    assert_eq!(setup.provider.models_asked(), [TURN]);
}

#[tokio::test]
async fn classifier_schema_refused_resends_without_format() {
    let refused = || {
        ProviderError::api(
            Some(400),
            Some("invalid_request_error".to_owned()),
            "Invalid parameter: 'text.format' is not supported with this model.".to_owned(),
        )
    };
    let setup = setup(Scripted::new([Reply::Error(refused), Reply::Text(ALLOW)]));
    let record = record();
    let outcome = judge(&setup, &record).await;
    assert_eq!(outcome.judgement, Some(allowed()));
    assert_eq!(outcome.chain_step, ChainStep::ClassifierModel);
    assert_eq!(outcome.skipped, Vec::<String>::new());
    let requests = setup.provider.requests();
    assert_eq!(requests.len(), 2);
    assert!(requests[0].output.is_some());
    assert_eq!(requests[1].output, None);
    assert_eq!(requests[1].model, SMALL);
}

#[tokio::test]
async fn a_refused_request_without_the_format_goes_to_the_next_step() {
    let refused = || ProviderError::api(Some(400), None, "the schema is not supported".to_owned());
    let setup =
        setup(Scripted::new([Reply::Error(refused), Reply::Error(refused), Reply::Text(ALLOW)]));
    let record = record();
    let outcome = judge(&setup, &record).await;
    assert_eq!(outcome.chain_step, ChainStep::TurnModel);
    assert_eq!(outcome.skipped, [format!("{SMALL}: refused")]);
}

#[tokio::test]
async fn a_settings_change_applies_from_the_next_exit() {
    let setup = setup(Scripted::new([Reply::Text(ALLOW), Reply::Text(ALLOW)]));
    let record = record();
    judge(&setup, &record).await;
    setup.settings.send_modify(|settings| {
        let mut changed = Settings::clone(settings);
        changed.classifier.model = Some("gpt-5.6-luna".to_owned());
        *settings = Arc::new(changed);
    });
    let outcome = setup.judge.judge(input(&ids(2), &record)).await;
    assert_eq!(outcome.model.as_deref(), Some("gpt-5.6-luna"));
}

#[tokio::test]
async fn the_record_hash_is_the_hash_of_what_the_model_read() {
    let setup = setup(Scripted::new([Reply::Text(ALLOW)]));
    let record = record();
    let outcome = judge(&setup, &record).await;
    let sent = setup.provider.requests()[0].messages[0].text();
    assert_eq!(outcome.record_sha256, Some(super::sha256_hex(sent.as_bytes())));
    assert_eq!(
        super::sha256_hex(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

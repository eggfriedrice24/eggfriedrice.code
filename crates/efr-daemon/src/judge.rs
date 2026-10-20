//! The daemon's judge of an exit (efr's auto spec, sections 9.2 to 9.5, phase A): the
//! classifier request of each exit of the `auto` sandbox, behind
//! [`efr_conversation::ExitJudge`].
//!
//! One request per step of the chain: the static prompt (`judge/prompt.md`) as the
//! system prompt, the record as the one user message, no tools, no history, no output
//! limit, the verdict schema as the output form, `side_call` and `one_shot`, and no
//! `prompt_cache_key`, so an OpenAI request goes over HTTP and never touches the
//! WebSocket of the turn. The effort is `classifier.effort` when the model lists it or
//! lists no efforts. Each step has `classifier.timeout_secs` on the daemon's clock, for
//! all its requests: a step that sends its request again, without the output form or
//! for a placeholder rationale, stays within that one time.
//!
//! The chain: the classifier model (`classifier.model`, else the small model of the
//! provider's company, [`Vendor::classifier_model`]), then the turn's model when
//! `classifier.fallback_to_turn_model` is on, then the user. A step goes to the next one
//! when its model is missing, refuses, times out or gives an answer that does not fit
//! the schema. A provider that refuses the credentials, or an account without quota,
//! fails for every model, so the chain goes to the user at once. A judge never allows
//! by failing: a chain that ends at the user has no judgement.
//!
//! The answer must be exactly one JSON object with the five members of the schema and
//! no other; efrd checks the lengths after the parse, because the providers take no
//! length keyword. The schema's `required` list names `rationale` first, and its
//! description and the prompt tell the model to write its reason before its verdict. The
//! members of `properties` go on the wire in the order of their names, because
//! serde_json keeps no insertion order (no `preserve_order`, which would change every
//! JSON map of the workspace): `category` comes first there, `rationale` before
//! `verdict`. A `rationale` that is shorter than [`MIN_RATIONALE_CHARS`] or a
//! placeholder (`x`, `n/a`, `tbd`) makes the answer unreadable: the classifier model
//! gets the same request once more, and a second such answer goes to the next step. When
//! a step answers that it does not take the output form, the judge sends the step once
//! more without it, and leaves the form out for that model for the daemon's life; the
//! prompt still asks for the JSON.
//!
//! What the judge keeps (`memory`): a model that is missing or refuses is skipped for
//! the rest of the conversation; a timeout or another error is tried again at the next
//! exit. An `allow` or a `deny` serves every later exit of the same turn whose record
//! differs only in its counters (`previous_exits_this_turn` and the refusal and denial
//! counts), with `cached` set and no request. The provider and the small model stay the
//! same for the daemon's life, so each conversation keeps one first model until
//! `classifier.model` changes. The SHA-256 of the record that the model read goes into
//! the outcome, so `efr-conversation` needs no hash function.
//!
//! The judge returns the model's judgement as it is. The turn applies the allow rule and
//! the naming rule of 9.4 (`efr-conversation`).
//!
//! efrd builds one judge over the provider of every conversation (`run.rs`) and passes
//! it as `ConversationDeps::judge`. `classifier.enabled = false` keeps the turn from
//! asking it.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use efr_config::{ClassifierSettings, Settings};
use efr_conversation::{ExitJudge, JudgeInput, JudgeOutcome};
use efr_permissions::Egress;
use efr_protocol::{ChainStep, ExitRecord, Judgement, Risk, UserAuthorization, Verdict};
use efr_provider::{
    Completion, Message, ModelInfo, OutputSchema, Provider, ProviderError, Request, StopReason,
};
use efr_stdx::time::Clock;
use jiff::Timestamp;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use tokio::sync::watch;
use tracing::Instrument as _;

use self::memory::{Cached, Memory};
use crate::catalog::Vendor;

mod memory;

/// The static prompt of every classifier request (9.3), the system prompt, for a
/// machine whose sandbox has no network.
pub(crate) const PROMPT: &str = include_str!("judge/prompt.md");

/// The prompt for a machine whose contained commands reach the network through efr's
/// proxy (phase D2): the same text with the network lines of the proxy.
pub(crate) const PROMPT_PROXY: &str = include_str!("judge/prompt-proxy.md");

/// The classifier prompt for the network mode of the engine (DP28). Each text is
/// static, so the prompt cache holds while the mode stays.
pub(crate) fn prompt(egress: Egress) -> &'static str {
    match egress {
        Egress::Proxy => PROMPT_PROXY,
        Egress::None => PROMPT,
    }
}

/// The name of the verdict schema in a request.
pub(crate) const SCHEMA_NAME: &str = "exit_verdict";

/// The most characters of the verdict's `category`.
pub(crate) const MAX_CATEGORY_CHARS: usize = 40;

/// The most characters of the verdict's `rationale`.
pub(crate) const MAX_RATIONALE_CHARS: usize = 300;

/// The fewest characters of the verdict's `rationale`, without the spaces around it: a
/// shorter one names no user message and is a placeholder.
pub(crate) const MIN_RATIONALE_CHARS: usize = 12;

/// Rationales that say nothing, which a model writes only to fill the member. They are
/// compared without case and without the spaces and punctuation around them.
const PLACEHOLDERS: &[&str] = &["x", "placeholder", "n/a", "none", "...", "tbd", "-"];

/// What the schema says of `rationale`, the same words as the prompt.
const RATIONALE_RULE: &str = "Write the rationale first, then decide. In one or two sentences, name the user message that relates to the action, or say that none does. Never leave it empty and never write a placeholder. At most 300 characters.";

/// The judge of the daemon: one value serves every conversation.
pub(crate) struct ClassifierJudge {
    provider: Arc<dyn Provider>,
    settings: watch::Receiver<Arc<Settings>>,
    /// The first model of the chain when `classifier.model` names none.
    small_model: &'static str,
    clock: Arc<dyn Clock>,
    memory: Memory,
}

impl ClassifierJudge {
    /// A judge that asks `provider`, the provider of every conversation, with the
    /// `[classifier]` settings of `settings` at each exit. `vendor` is the company of
    /// the provider, which names its small model.
    pub(crate) fn new(
        provider: Arc<dyn Provider>,
        settings: watch::Receiver<Arc<Settings>>,
        vendor: Vendor,
        clock: Arc<dyn Clock>,
    ) -> Self {
        ClassifierJudge {
            provider,
            settings,
            small_model: vendor.classifier_model(),
            clock,
            memory: Memory::default(),
        }
    }

    /// The steps of the chain for `input`, in order: each model with its step.
    fn steps(&self, settings: &ClassifierSettings, input: &JudgeInput<'_>) -> Vec<Step> {
        let first = settings.model.clone().unwrap_or_else(|| self.small_model.to_owned());
        let mut steps = vec![Step {
            model: first,
            chain_step: ChainStep::ClassifierModel,
            // NOTE: a model that the config names is tried even when the list does
            // not hold it, such as a hidden model of the catalog; its answer decides.
            check_list: settings.model.is_none(),
        }];
        let turn_model = input.turn_model;
        if settings.fallback_to_turn_model
            && !turn_model.is_empty()
            && steps.iter().all(|step| step.model != turn_model)
        {
            steps.push(Step {
                model: turn_model.to_owned(),
                chain_step: ChainStep::TurnModel,
                check_list: false,
            });
        }
        steps
    }

    /// Runs one step within `classifier.timeout_secs`: the request, then once more
    /// without the output form when the model does not take it, and once more when the
    /// classifier model wrote a placeholder as its rationale.
    async fn run_step(
        &self,
        step: &Step,
        settings: &ClassifierSettings,
        models: &[ModelInfo],
        record: &str,
        egress: Egress,
    ) -> Result<Judgement, Failure> {
        let info = models.iter().find(|info| info.id == step.model);
        if step.check_list && !models.is_empty() && info.is_none() {
            return Err(Failure::Missing);
        }
        let effort = effort(settings, info);
        let mut with_format = self.memory.takes_format(&step.model);
        // NOTE: only the classifier model is asked again, once, before the turn's model.
        let mut ask_again = step.chain_step == ChainStep::ClassifierModel;
        let requests = async {
            loop {
                let request = request(&step.model, effort.clone(), record, with_format, egress);
                match self.send(request).await {
                    Err(Failure::FormatRefused) if with_format => {
                        tracing::info!(model = %step.model, "the classifier model refused the output form; efrd sends it without the form from now on");
                        self.memory.refuses_format(&step.model);
                        with_format = false;
                    }
                    Err(Failure::FormatRefused) => return Err(Failure::Refused),
                    Err(Failure::Placeholder) if ask_again => {
                        tracing::debug!(model = %step.model, "the classifier model wrote no real rationale; efrd asks it once more");
                        ask_again = false;
                    }
                    done => return done,
                }
            }
        };
        let timeout = Duration::from_secs(u64::from(settings.timeout_secs));
        self.clock.timeout(timeout, requests).await.unwrap_or(Err(Failure::Timeout))
    }

    /// Sends `request` and reads the answer.
    async fn send(&self, request: Request) -> Result<Judgement, Failure> {
        let span = tracing::debug_span!(
            "provider_request",
            provider = %self.provider.id(),
            model = %request.model,
            purpose = "classifier",
        );
        let with_format = request.output.is_some();
        let answer = self.provider.complete(request).instrument(span);
        match answer.await {
            Err(error) => Err(Failure::of(&error, with_format)),
            Ok(completion) => read(&completion),
        }
    }

    fn outcome(
        &self,
        started: Timestamp,
        judgement: Option<(Judgement, &Step)>,
        skipped: Vec<String>,
        record_sha256: Option<String>,
    ) -> JudgeOutcome {
        let (judgement, model, chain_step) = match judgement {
            Some((judgement, step)) => (Some(judgement), Some(step.model.clone()), step.chain_step),
            None => (None, None, ChainStep::User),
        };
        JudgeOutcome {
            judgement,
            model,
            chain_step,
            skipped,
            latency_ms: millis(started, self.clock.now()),
            record_sha256,
            cached: false,
        }
    }
}

#[async_trait]
impl ExitJudge for ClassifierJudge {
    async fn judge(&self, input: JudgeInput<'_>) -> JudgeOutcome {
        let started = self.clock.now();
        let settings = self.settings.borrow().classifier.clone();
        let Ok(record) = serde_json::to_string(input.record) else {
            // NOTE: a record always serializes, since its maps have string keys; this
            // arm only keeps the code free of a panic.
            return self.outcome(started, None, Vec::new(), None);
        };
        let record_sha256 = Some(sha256_hex(record.as_bytes()));
        let conversation = input.conversation_id;
        let key = cache_key(input.record);
        if let Some(key) = &key
            && let Some(hit) = self.memory.verdict(conversation, input.turn_id, key)
        {
            return JudgeOutcome {
                judgement: Some(hit.judgement),
                model: Some(hit.model),
                chain_step: hit.chain_step,
                skipped: Vec::new(),
                latency_ms: millis(started, self.clock.now()),
                record_sha256,
                cached: true,
            };
        }
        let models = self.provider.models();
        let mut skipped = Vec::new();
        for step in self.steps(&settings, &input) {
            if let Some(reason) = self.memory.skipped(conversation, &step.model) {
                skipped.push(format!("{}: {reason} earlier in this conversation", step.model));
                continue;
            }
            match self.run_step(&step, &settings, &models, &record, input.egress).await {
                Ok(judgement) => {
                    if let Some(key) = key
                        && matches!(judgement.verdict, Verdict::Allow | Verdict::Deny)
                    {
                        let cached = Cached {
                            judgement: judgement.clone(),
                            model: step.model.clone(),
                            chain_step: step.chain_step,
                        };
                        self.memory.keep_verdict(conversation, input.turn_id, key, cached);
                    }
                    return self.outcome(started, Some((judgement, &step)), skipped, record_sha256);
                }
                Err(failure) => {
                    if failure.skips_model() {
                        self.memory.skip(conversation, &step.model, failure.reason());
                    }
                    tracing::debug!(
                        conversation_id = %input.conversation_id,
                        call_id = %input.call_id,
                        model = %step.model,
                        reason = failure.reason(),
                        "a step of the classifier chain gave no verdict"
                    );
                    skipped.push(format!("{}: {}", step.model, failure.reason()));
                    if failure.ends_chain() {
                        break;
                    }
                }
            }
        }
        self.outcome(started, None, skipped, record_sha256)
    }
}

impl fmt::Debug for ClassifierJudge {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClassifierJudge")
            .field("provider", &self.provider.id())
            .field("small_model", &self.small_model)
            .finish_non_exhaustive()
    }
}

/// One step of the chain.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Step {
    model: String,
    chain_step: ChainStep,
    /// True when a model list that does not hold the model skips it without a request.
    check_list: bool,
}

/// Why a step gave no verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Failure {
    /// The provider does not serve the model.
    Missing,
    /// The provider refused the request for this model: a model or account error.
    Refused,
    /// The provider refused the output form; the step goes once more without it.
    FormatRefused,
    /// The step took longer than `classifier.timeout_secs`.
    Timeout,
    /// A failure that can pass: a rate limit, an overload, the transport, a broken
    /// stream or a request too large for the model.
    Failed,
    /// The answer was not one JSON object of the schema, or a member was too long.
    Unreadable,
    /// The answer fits the schema, but its `rationale` is empty, too short or a
    /// placeholder. It is unreadable too; the classifier model gets one more request
    /// first.
    Placeholder,
    /// The provider stopped the answer for its content.
    Filtered,
    /// The provider refused the credentials, or has none: every model fails the same.
    Credentials,
    /// The account has no quota left: every model fails the same.
    Quota,
}

impl Failure {
    /// What a provider error means for the chain. `with_format` is true when the
    /// request asked for the output form.
    fn of(error: &ProviderError, with_format: bool) -> Failure {
        match error {
            ProviderError::UnknownModel { .. } => Failure::Missing,
            ProviderError::Unauthorized { .. }
            | ProviderError::NotLoggedIn
            | ProviderError::Token { .. } => Failure::Credentials,
            ProviderError::Api { status: Some(429), .. } => Failure::Quota,
            ProviderError::Api { status: Some(400), message, .. }
                if with_format && names_the_format(message) =>
            {
                Failure::FormatRefused
            }
            ProviderError::Api { status: Some(400 | 402 | 403 | 404), .. } => Failure::Refused,
            _ => Failure::Failed,
        }
    }

    /// The reason as `skipped` names it, after the model.
    fn reason(self) -> &'static str {
        match self {
            Failure::Missing => "missing",
            Failure::Refused | Failure::FormatRefused => "refused",
            Failure::Timeout => "timeout",
            Failure::Failed => "error",
            Failure::Unreadable | Failure::Placeholder => "parse error",
            Failure::Filtered => "content filter",
            Failure::Credentials => "credentials refused",
            Failure::Quota => "quota",
        }
    }

    /// True when every later step fails the same way, so the chain goes to the user.
    fn ends_chain(self) -> bool {
        matches!(self, Failure::Credentials | Failure::Quota)
    }

    /// True when the model fails the same way at every later exit, so the conversation
    /// skips it from now on. A timeout or an error that can pass is tried again.
    fn skips_model(self) -> bool {
        matches!(self, Failure::Missing | Failure::Refused)
    }
}

/// The key of a record in the cache of a turn: the SHA-256 of the record without the
/// facts that change from exit to exit of one turn, the earlier exits and the counters.
fn cache_key(record: &ExitRecord) -> Option<String> {
    let mut record = record.clone();
    record.facts.previous_exits_this_turn.clear();
    record.facts.refusals_in_a_row = 0;
    record.facts.denials_in_a_row = 0;
    record.facts.denials_this_turn = 0;
    serde_json::to_vec(&record).ok().map(|bytes| sha256_hex(&bytes))
}

/// True when an error message of a 400 names the output form: the backend does not
/// take `text.format` or `output_config.format`, or the schema.
fn names_the_format(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    ["format", "schema", "output_config"].iter().any(|word| message.contains(word))
}

/// The effort of a step: `classifier.effort` when the model lists it or lists no
/// efforts, else none, because a provider refuses an effort that the model does not
/// take.
fn effort(settings: &ClassifierSettings, info: Option<&ModelInfo>) -> Option<String> {
    let effort = settings.effort.as_str();
    let fits = info.is_none_or(|info| {
        info.efforts.is_empty() || info.efforts.iter().any(|listed| listed == effort)
    });
    (fits && !effort.is_empty()).then(|| effort.to_owned())
}

/// The request of one step.
fn request(
    model: &str,
    effort: Option<String>,
    record: &str,
    with_format: bool,
    egress: Egress,
) -> Request {
    let mut request = Request::new(model);
    request.system = Some(prompt(egress).to_owned());
    request.messages = vec![Message::user(record)];
    request.effort = effort;
    request.side_call = true;
    request.one_shot = true;
    request.output = with_format.then(|| OutputSchema::new(SCHEMA_NAME, schema()));
    // NOTE: nobody reads the summary of a review, and no `prompt_cache_key`: on the
    // OpenAI path that key would take the WebSocket of the turn.
    request.provider_options.insert("reasoning_summary".to_owned(), Value::Null);
    request
}

/// The JSON schema of the verdict (9.4): enums, every member required and no other
/// member, and no length keyword, which the Messages API does not take.
///
/// `required` names `rationale` first, and its description asks the model to write
/// it first, so the verdict follows from it. serde_json keeps the members of an object
/// in the order of their names, so `properties` goes on the wire as `category`,
/// `rationale`, `risk`, `user_authorization`, `verdict`: `rationale` still comes before
/// `verdict`, but not first.
pub(crate) fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "rationale": {"type": "string", "description": RATIONALE_RULE},
            "category": {
                "type": "string",
                "description": "The category of the risk, at most 40 characters.",
            },
            "risk": {"type": "string", "enum": ["low", "medium", "high", "critical"]},
            "user_authorization": {
                "type": "string",
                "enum": ["none", "low", "medium", "high"],
            },
            "verdict": {"type": "string", "enum": ["allow", "deny", "ask_user"]},
        },
        "required": ["rationale", "category", "risk", "user_authorization", "verdict"],
        "additionalProperties": false,
    })
}

/// The verdict as the model must write it: exactly these members.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Answer {
    rationale: String,
    category: String,
    risk: Risk,
    user_authorization: UserAuthorization,
    verdict: Verdict,
}

/// The judgement in a completed answer.
fn read(completion: &Completion) -> Result<Judgement, Failure> {
    if matches!(completion.stop_reason, StopReason::ContentFilter | StopReason::Refusal) {
        return Err(Failure::Filtered);
    }
    parse(&completion.message.text())
}

/// The judgement in `text`, which must be one JSON object of the schema and nothing
/// else, with a `category` and a `rationale` within their lengths and a `rationale`
/// that is not a placeholder; else why it has none, [`Failure::Placeholder`] when the
/// `rationale` is the only fault of the answer.
fn parse(text: &str) -> Result<Judgement, Failure> {
    let answer: Answer = serde_json::from_str(text.trim()).map_err(|_| Failure::Unreadable)?;
    if answer.category.chars().count() > MAX_CATEGORY_CHARS
        || answer.rationale.chars().count() > MAX_RATIONALE_CHARS
    {
        return Err(Failure::Unreadable);
    }
    if is_placeholder(&answer.rationale) {
        return Err(Failure::Placeholder);
    }
    Ok(Judgement {
        verdict: answer.verdict,
        risk: answer.risk,
        user_authorization: answer.user_authorization,
        category: answer.category,
        rationale: answer.rationale,
    })
}

/// True when `rationale` says nothing: shorter than [`MIN_RATIONALE_CHARS`] without the
/// spaces around it, only punctuation, or one of [`PLACEHOLDERS`].
fn is_placeholder(rationale: &str) -> bool {
    let text = rationale.trim();
    if text.chars().count() < MIN_RATIONALE_CHARS {
        return true;
    }
    let word =
        text.trim_matches(|c: char| c.is_whitespace() || c.is_ascii_punctuation()).to_lowercase();
    word.is_empty() || PLACEHOLDERS.contains(&word.as_str())
}

/// The SHA-256 of `bytes` in lowercase hex.
fn sha256_hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut hex = String::with_capacity(64);
    for byte in Sha256::digest(bytes) {
        hex.push(char::from(DIGITS[usize::from(byte >> 4)]));
        hex.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    hex
}

/// The milliseconds from `from` to `to`, zero when the clock went back.
fn millis(from: Timestamp, to: Timestamp) -> u64 {
    let elapsed = Duration::try_from(to.duration_since(from)).unwrap_or_default();
    u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests;

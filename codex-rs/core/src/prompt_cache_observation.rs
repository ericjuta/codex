use codex_api::ResponsesApiRequest;
use codex_protocol::models::ResponseItem;
use codex_protocol::protocol::TokenUsage;
use hmac::Hmac;
use hmac::Mac;
use serde::Serialize;
use serde_json::Value;
use sha2::Sha256;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::time::Instant;

const DIGEST_BYTES: usize = 16;
const MAX_INPUT_ITEMS: usize = 256;
const MAX_REPORTED_INPUT_ITEMS: usize = MAX_INPUT_ITEMS + 1;
const MAX_SERIALIZED_OBSERVATION_BYTES: usize = 4096;
const MAX_IDENTIFIER_CHARS: usize = 64;

type HmacSha256 = Hmac<Sha256>;

static DIGEST_KEY: OnceLock<[u8; 32]> = OnceLock::new();

#[derive(Clone, Copy, Debug)]
pub(crate) enum PromptCacheRequestClass {
    Normal,
    ToolHeavy,
    Prewarm,
    Retry,
}

impl PromptCacheRequestClass {
    fn as_str(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::ToolHeavy => "tool_heavy",
            Self::Prewarm => "prewarm",
            Self::Retry => "retry",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum PromptCacheTransport {
    Http,
    Websocket,
    WebsocketReused,
    Warmup,
    Fallback,
}

impl PromptCacheTransport {
    fn as_str(self) -> &'static str {
        match self {
            Self::Http => "http",
            Self::Websocket => "websocket",
            Self::WebsocketReused => "websocket_reused",
            Self::Warmup => "warmup",
            Self::Fallback => "fallback",
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct PromptCacheObservationLedger {
    previous: Mutex<Option<PreviousRequest>>,
}

#[derive(Debug)]
struct PreviousRequest {
    stable_surface_digest: String,
    input_item_digests: Vec<String>,
    input_item_count: usize,
    input_items_truncated: bool,
}

#[derive(Debug, Serialize)]
pub(crate) struct PromptCacheObservation {
    request_class: &'static str,
    model: String,
    provider: String,
    transport: &'static str,
    prompt_cache_key_scope: &'static str,
    prompt_cache_key_digest: String,
    instructions_digest: String,
    tools_digest: String,
    tools_set_digest: String,
    input_prefix_digest: String,
    first_divergent_input_index: String,
    input_item_count: usize,
    context_transition: &'static str,
    previous_response_id_present: bool,
    connection_reused: bool,
    input_tokens: Option<i64>,
    cached_input_tokens: Option<i64>,
    cache_ratio: Option<f64>,
    output_tokens: Option<i64>,
    ttft_ms: Option<i64>,
    outcome: Option<&'static str>,
    ledger_truncated: bool,
    #[serde(skip)]
    started_at: Instant,
}

impl PromptCacheObservationLedger {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn observe_request(
        &self,
        request: &ResponsesApiRequest,
        provider: &str,
        prompt_cache_key: &str,
        prompt_cache_key_scope: &'static str,
        request_class: PromptCacheRequestClass,
        transport: PromptCacheTransport,
        previous_response_id_present: bool,
        connection_reused: bool,
    ) -> PromptCacheObservation {
        let instructions_digest = digest_bytes("instructions", request.instructions.as_bytes());
        let tools_digest = digest_json("tools/ordered", &request.tools);
        let tools_set_digest = tools_set_digest(request.tools.as_ref());
        let input_prefix_digest = digest_json("input/ordered", &request.input);
        let input_item_digests = request
            .input
            .iter()
            .take(MAX_INPUT_ITEMS)
            .map(|item| digest_json("input/item", item))
            .collect::<Vec<_>>();
        let input_items_truncated = request.input.len() > MAX_INPUT_ITEMS;
        let stable_surface_digest = digest_json(
            "request/stable-surface",
            &(
                &request.model,
                &request.instructions,
                &request.tools,
                &request.tool_choice,
                request.parallel_tool_calls,
                &request.reasoning,
                request.store,
                request.stream,
                &request.include,
                &request.service_tier,
                &request.prompt_cache_key,
                &request.text,
            ),
        );

        let mut previous = self
            .previous
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (first_divergent_input_index, context_transition) = previous
            .as_ref()
            .filter(|previous| previous.stable_surface_digest == stable_surface_digest)
            .map(|previous| {
                compare_input_prefix(
                    previous,
                    &input_item_digests,
                    request.input.len(),
                    input_items_truncated,
                )
            })
            .unwrap_or_else(|| {
                if previous.is_none() {
                    ("unknown".to_string(), "initial")
                } else {
                    ("unknown".to_string(), "unknown")
                }
            });
        *previous = Some(PreviousRequest {
            stable_surface_digest,
            input_item_digests,
            input_item_count: request.input.len().min(MAX_REPORTED_INPUT_ITEMS),
            input_items_truncated,
        });

        PromptCacheObservation {
            request_class: request_class.as_str(),
            model: bounded_identifier(&request.model),
            provider: bounded_identifier(provider),
            transport: transport.as_str(),
            prompt_cache_key_scope,
            prompt_cache_key_digest: digest_bytes("prompt-cache-key", prompt_cache_key.as_bytes()),
            instructions_digest,
            tools_digest,
            tools_set_digest,
            input_prefix_digest,
            first_divergent_input_index,
            input_item_count: request.input.len(),
            context_transition,
            previous_response_id_present,
            connection_reused,
            input_tokens: None,
            cached_input_tokens: None,
            cache_ratio: None,
            output_tokens: None,
            ttft_ms: None,
            outcome: None,
            ledger_truncated: input_items_truncated,
            started_at: Instant::now(),
        }
    }
}

impl PromptCacheObservation {
    pub(crate) fn record_completed(mut self, usage: Option<&TokenUsage>, ttft_ms: Option<i64>) {
        self.record_usage(usage, ttft_ms);
        self.outcome = Some("completed");
        self.emit();
    }

    pub(crate) fn record_failed(mut self, outcome: &'static str) {
        self.outcome = Some(outcome);
        self.emit();
    }

    fn record_usage(&mut self, usage: Option<&TokenUsage>, ttft_ms: Option<i64>) {
        if let Some(usage) = usage {
            self.input_tokens = Some(usage.input_tokens);
            self.cached_input_tokens = Some(usage.cached_input_tokens);
            self.cache_ratio = (usage.input_tokens > 0)
                .then(|| usage.cached_input_tokens as f64 / usage.input_tokens as f64);
            self.output_tokens = Some(usage.output_tokens);
        }
        self.ttft_ms = ttft_ms;
    }

    /// Emits the local or sampled ledger plane; digest fields must not become metric labels.
    fn emit(mut self) {
        let serialized_size = serde_json::to_vec(&self)
            .map(|serialized| serialized.len())
            .unwrap_or(MAX_SERIALIZED_OBSERVATION_BYTES + 1);
        if serialized_size > MAX_SERIALIZED_OBSERVATION_BYTES {
            self.ledger_truncated = true;
        }

        tracing::debug!(
            target: "codex.prompt_cache",
            request_class = self.request_class,
            model = %self.model,
            provider = %self.provider,
            transport = self.transport,
            prompt_cache_key_scope = self.prompt_cache_key_scope,
            prompt_cache_key_digest = %self.prompt_cache_key_digest,
            instructions_digest = %self.instructions_digest,
            tools_digest = %self.tools_digest,
            tools_set_digest = %self.tools_set_digest,
            input_prefix_digest = %self.input_prefix_digest,
            first_divergent_input_index = %self.first_divergent_input_index,
            input_item_count = self.input_item_count,
            context_transition = self.context_transition,
            previous_response_id_present = self.previous_response_id_present,
            connection_reused = self.connection_reused,
            input_tokens = ?self.input_tokens,
            cached_input_tokens = ?self.cached_input_tokens,
            cache_ratio = ?self.cache_ratio,
            output_tokens = ?self.output_tokens,
            ttft_ms = ?self.ttft_ms,
            outcome = ?self.outcome,
            ledger_truncated = self.ledger_truncated,
            ledger_serialized_bytes = serialized_size,
            elapsed_ms = self.started_at.elapsed().as_millis(),
            "prompt cache observation"
        );
    }
}

fn compare_input_prefix(
    previous: &PreviousRequest,
    current_item_digests: &[String],
    current_item_count: usize,
    current_items_truncated: bool,
) -> (String, &'static str) {
    for (index, (previous, current)) in previous
        .input_item_digests
        .iter()
        .zip(current_item_digests)
        .enumerate()
    {
        if previous != current {
            return (index.to_string(), "rebuild");
        }
    }

    if previous.input_items_truncated || current_items_truncated {
        return ("unknown".to_string(), "unknown");
    }
    if current_item_count == previous.input_item_count {
        return ("none".to_string(), "unknown");
    }
    if current_item_count > previous.input_item_count {
        return (previous.input_item_count.to_string(), "delta");
    }
    (current_item_count.to_string(), "rebuild")
}

fn tools_set_digest(tools: Option<&Vec<Value>>) -> String {
    let mut tool_digests = tools
        .into_iter()
        .flatten()
        .map(|tool| digest_json("tools/item", tool))
        .collect::<Vec<_>>();
    tool_digests.sort_unstable();
    digest_json("tools/set", &tool_digests)
}

fn digest_json<T: Serialize>(domain: &str, value: &T) -> String {
    let bytes = serde_json::to_vec(value).unwrap_or_else(|_| b"<serialization-error>".to_vec());
    digest_bytes(domain, &bytes)
}

fn digest_bytes(domain: &str, value: &[u8]) -> String {
    let Ok(mut mac) = HmacSha256::new_from_slice(digest_key()) else {
        unreachable!("HMAC accepts a 32-byte key");
    };
    mac.update(domain.as_bytes());
    mac.update(&[0]);
    mac.update(value);
    let digest = mac.finalize().into_bytes();
    digest[..DIGEST_BYTES]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn digest_key() -> &'static [u8; 32] {
    DIGEST_KEY.get_or_init(rand::random)
}

fn bounded_identifier(value: &str) -> String {
    value.chars().take(MAX_IDENTIFIER_CHARS).collect()
}

pub(crate) fn request_class(
    request: &ResponsesApiRequest,
    warmup: bool,
    retry_after_unauthorized: bool,
) -> PromptCacheRequestClass {
    if warmup {
        PromptCacheRequestClass::Prewarm
    } else if retry_after_unauthorized {
        PromptCacheRequestClass::Retry
    } else if request.input.iter().any(is_tool_item) {
        PromptCacheRequestClass::ToolHeavy
    } else {
        PromptCacheRequestClass::Normal
    }
}

fn is_tool_item(item: &ResponseItem) -> bool {
    matches!(
        item,
        ResponseItem::FunctionCall { .. }
            | ResponseItem::FunctionCallOutput { .. }
            | ResponseItem::LocalShellCall { .. }
            | ResponseItem::CustomToolCall { .. }
            | ResponseItem::CustomToolCallOutput { .. }
            | ResponseItem::ToolSearchCall { .. }
            | ResponseItem::ToolSearchOutput { .. }
            | ResponseItem::WebSearchCall { .. }
            | ResponseItem::ImageGenerationCall { .. }
    )
}

#[cfg(test)]
#[path = "prompt_cache_observation_tests.rs"]
mod tests;

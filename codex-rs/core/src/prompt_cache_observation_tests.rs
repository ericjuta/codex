use super::*;
use codex_protocol::models::ContentItem;
use codex_protocol::models::ResponseItem;
use codex_protocol::protocol::TokenUsage;
use serde_json::json;

fn request(input: Vec<ResponseItem>, tools: Option<Vec<Value>>) -> ResponsesApiRequest {
    ResponsesApiRequest {
        model: "test-model".to_string(),
        instructions: "private instructions".to_string(),
        input,
        tools,
        tool_choice: "auto".to_string(),
        parallel_tool_calls: true,
        reasoning: None,
        store: false,
        stream: true,
        stream_options: None,
        include: Vec::new(),
        service_tier: None,
        prompt_cache_key: Some("thread-secret".to_string()),
        text: None,
        client_metadata: Some(std::collections::HashMap::from([(
            "secret-key".to_string(),
            "secret-value".to_string(),
        )])),
    }
}

fn message(text: &str) -> ResponseItem {
    ResponseItem::Message {
        id: None,
        role: "user".to_string(),
        content: vec![ContentItem::InputText {
            text: text.to_string(),
        }],
        phase: None,
        internal_chat_message_metadata_passthrough: None,
    }
}

#[test]
fn observations_are_redacted_and_keyed() {
    let ledger = PromptCacheObservationLedger::default();
    let observation = ledger.observe_request(
        &request(vec![message("secret prompt")], None),
        "private-provider",
        "thread-secret",
        "thread",
        PromptCacheRequestClass::Normal,
        PromptCacheTransport::Http,
        false,
        false,
    );
    let serialized = serde_json::to_string(&observation).expect("serialize observation");

    assert!(!serialized.contains("secret prompt"));
    assert!(!serialized.contains("private instructions"));
    assert!(!serialized.contains("thread-secret"));
    assert!(!serialized.contains("secret-value"));
    assert_eq!(observation.prompt_cache_key_digest.len(), DIGEST_BYTES * 2);
    assert!(!observation.ledger_truncated);
}

#[test]
fn ordered_tool_digest_changes_without_changing_tool_set_digest() {
    let ledger = PromptCacheObservationLedger::default();
    let first = ledger.observe_request(
        &request(
            vec![message("one")],
            Some(vec![json!({"name": "a"}), json!({"name": "b"})]),
        ),
        "provider",
        "key",
        "thread",
        PromptCacheRequestClass::Normal,
        PromptCacheTransport::Http,
        false,
        false,
    );
    let second = ledger.observe_request(
        &request(
            vec![message("one")],
            Some(vec![json!({"name": "b"}), json!({"name": "a"})]),
        ),
        "provider",
        "key",
        "thread",
        PromptCacheRequestClass::Normal,
        PromptCacheTransport::WebsocketReused,
        true,
        true,
    );

    assert_ne!(first.tools_digest, second.tools_digest);
    assert_eq!(first.tools_set_digest, second.tools_set_digest);
    assert_eq!(second.first_divergent_input_index, "unknown");
    assert_eq!(second.context_transition, "unknown");
    assert_eq!(second.transport, "websocket_reused");
    assert!(second.previous_response_id_present);
    assert!(second.connection_reused);
}

#[test]
fn input_delta_and_provider_usage_are_recorded() {
    let ledger = PromptCacheObservationLedger::default();
    let _first = ledger.observe_request(
        &request(vec![message("one")], None),
        "provider",
        "key",
        "thread",
        PromptCacheRequestClass::Normal,
        PromptCacheTransport::Http,
        false,
        false,
    );
    let mut second = ledger.observe_request(
        &request(vec![message("one"), message("two")], None),
        "provider",
        "key",
        "thread",
        PromptCacheRequestClass::Normal,
        PromptCacheTransport::Http,
        false,
        false,
    );
    second.record_usage(
        Some(&TokenUsage {
            input_tokens: 100,
            cached_input_tokens: 75,
            output_tokens: 10,
            reasoning_output_tokens: 0,
            total_tokens: 110,
        }),
        Some(42),
    );

    assert_eq!(second.first_divergent_input_index, "1");
    assert_eq!(second.context_transition, "delta");
    assert_eq!(second.input_tokens, Some(100));
    assert_eq!(second.cached_input_tokens, Some(75));
    assert_eq!(second.cache_ratio, Some(0.75));
    assert_eq!(second.ttft_ms, Some(42));
}

#[test]
fn stable_surface_change_starts_a_new_comparison() {
    let ledger = PromptCacheObservationLedger::default();
    let _first = ledger.observe_request(
        &request(vec![message("one")], None),
        "provider",
        "key",
        "thread",
        PromptCacheRequestClass::Normal,
        PromptCacheTransport::Http,
        false,
        false,
    );
    let mut changed_request = request(vec![message("one")], None);
    changed_request.service_tier = Some("flex".to_string());
    let second = ledger.observe_request(
        &changed_request,
        "provider",
        "key",
        "thread",
        PromptCacheRequestClass::Normal,
        PromptCacheTransport::Http,
        false,
        false,
    );

    assert_eq!(second.first_divergent_input_index, "unknown");
    assert_eq!(second.context_transition, "unknown");
}

#[test]
fn changed_input_item_reports_a_rebuild_boundary() {
    let ledger = PromptCacheObservationLedger::default();
    let _first = ledger.observe_request(
        &request(vec![message("one"), message("two")], None),
        "provider",
        "key",
        "thread",
        PromptCacheRequestClass::Normal,
        PromptCacheTransport::Http,
        false,
        false,
    );
    let second = ledger.observe_request(
        &request(vec![message("one"), message("changed")], None),
        "provider",
        "key",
        "thread",
        PromptCacheRequestClass::Normal,
        PromptCacheTransport::Http,
        false,
        false,
    );

    assert_eq!(second.first_divergent_input_index, "1");
    assert_eq!(second.context_transition, "rebuild");
}

#[test]
fn input_item_comparison_is_capped() {
    let input = (0..=MAX_INPUT_ITEMS)
        .map(|index| message(&format!("item-{index}")))
        .collect();
    let ledger = PromptCacheObservationLedger::default();
    let observation = ledger.observe_request(
        &request(input, None),
        "provider",
        "key",
        "thread",
        PromptCacheRequestClass::Normal,
        PromptCacheTransport::Http,
        false,
        false,
    );

    assert_eq!(observation.input_item_count, MAX_INPUT_ITEMS + 1);
    assert!(observation.ledger_truncated);
}

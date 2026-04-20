use anyhow::Result;
use codex_config::types::MemoryBackend;
use codex_features::Feature;
use core_test_support::responses::ev_assistant_message;
use core_test_support::responses::ev_completed;
use core_test_support::responses::ev_response_created;
use core_test_support::responses::mount_sse_once;
use core_test_support::responses::sse;
use core_test_support::responses::start_mock_server;
use core_test_support::test_codex::test_codex;
use serde_json::json;
use serial_test::serial;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::ResponseTemplate;
use wiremock::matchers::method;
use wiremock::matchers::path;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial(agentmemory_env)]
async fn agentmemory_session_lifecycle_is_registered_end_to_end() -> Result<()> {
    let model_server = start_mock_server().await;
    let agentmemory_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/agentmemory/session/start"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&agentmemory_server)
        .await;
    Mock::given(method("POST"))
        .and(path("/agentmemory/session/end"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&agentmemory_server)
        .await;
    Mock::given(method("POST"))
        .and(path("/agentmemory/summarize"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "success": false,
            "error": "no_observations"
        })))
        .expect(1)
        .mount(&agentmemory_server)
        .await;
    Mock::given(method("POST"))
        .and(path("/agentmemory/crystals/auto"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "success": true,
            "groupCount": 0,
            "crystalIds": [],
        })))
        .expect(1)
        .mount(&agentmemory_server)
        .await;
    Mock::given(method("POST"))
        .and(path("/agentmemory/consolidate-pipeline"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "success": true,
        })))
        .expect(1)
        .mount(&agentmemory_server)
        .await;

    let mut builder = test_codex().with_config({
        let agentmemory_base_url = agentmemory_server.uri();
        move |config| {
            config.memories.backend = MemoryBackend::Agentmemory;
            config.memories.agentmemory.base_url = agentmemory_base_url;
        }
    });
    let test = builder.build(&model_server).await?;

    test.codex.shutdown_and_wait().await?;

    let requests = agentmemory_server
        .received_requests()
        .await
        .unwrap_or_default();
    let session_id = test.session_configured.session_id.to_string();
    let cwd = test.config.cwd.display().to_string();
    let expected_start = json!({
        "sessionId": session_id,
        "project": cwd,
        "cwd": cwd,
    });
    let expected_end = json!({
        "sessionId": session_id,
    });
    let expected_summarize = json!({
        "sessionId": session_id,
    });
    let expected_stop_observe = json!({
        "sessionId": session_id,
        "hookType": "stop",
        "project": cwd,
        "cwd": cwd,
        "timestamp": request_summaries_placeholder_timestamp(),
        "source": "codex-native",
        "payload_version": "1",
        "event_id": request_summaries_placeholder_event_id(),
        "capabilities": [
            "assistant_result",
            "structured_post_tool_payload",
            "query_aware_context",
            "event_identity",
        ],
        "persistence_class": "diagnostics_only",
        "data": {
            "session_id": session_id,
            "cwd": cwd,
        },
    });
    let expected_session_end_observe = json!({
        "sessionId": session_id,
        "hookType": "session_end",
        "project": cwd,
        "cwd": cwd,
        "timestamp": request_summaries_placeholder_timestamp(),
        "source": "codex-native",
        "payload_version": "1",
        "event_id": request_summaries_placeholder_event_id(),
        "capabilities": [
            "assistant_result",
            "structured_post_tool_payload",
            "query_aware_context",
            "event_identity",
        ],
        "persistence_class": "ephemeral",
        "data": {
            "session_id": session_id,
            "cwd": cwd,
            "summary_success": false,
            "summary_error": "no_observations",
        },
    });

    let request_summaries = requests
        .iter()
        .map(|request| {
            (
                request.url.path().to_string(),
                serde_json::from_slice::<serde_json::Value>(&request.body)
                    .expect("agentmemory request body should be valid json"),
            )
        })
        .collect::<Vec<_>>();

    let normalized = normalize_request_summaries(request_summaries);
    assert!(
        normalized.contains(&("/agentmemory/session/start".to_string(), expected_start)),
        "session start request should be emitted",
    );
    assert!(
        normalized.contains(&("/agentmemory/summarize".to_string(), expected_summarize)),
        "session summarize request should be emitted",
    );
    assert!(
        normalized.contains(&("/agentmemory/observe".to_string(), expected_stop_observe)),
        "stop observe payload should be emitted",
    );
    assert!(
        normalized.contains(&(
            "/agentmemory/observe".to_string(),
            expected_session_end_observe
        )),
        "session end observe payload should be emitted",
    );
    assert!(
        normalized.contains(&("/agentmemory/session/end".to_string(), expected_end)),
        "session end request should be emitted",
    );
    assert!(
        normalized.contains(&(
            "/agentmemory/crystals/auto".to_string(),
            json!({ "olderThanDays": 0 }),
        )),
        "session end should trigger crystals/auto when consolidation is enabled",
    );
    assert!(
        normalized.contains(&(
            "/agentmemory/consolidate-pipeline".to_string(),
            json!({ "tier": "all", "force": true }),
        )),
        "session end should trigger consolidate-pipeline when consolidation is enabled",
    );
    agentmemory_server.verify().await;

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial(agentmemory_env)]
async fn session_start_context_is_injected_from_session_start_without_memory_tool() -> Result<()> {
    let model_server = start_mock_server().await;
    let response = mount_sse_once(
        &model_server,
        sse(vec![
            ev_response_created("resp-1"),
            ev_assistant_message("msg-1", "hello from memory"),
            ev_completed("resp-1"),
        ]),
    )
    .await;
    let agentmemory_server = MockServer::start().await;
    let startup_context = "<agentmemory-context>startup tide note</agentmemory-context>";

    Mock::given(method("POST"))
        .and(path("/agentmemory/session/start"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "context": startup_context,
        })))
        .expect(1)
        .mount(&agentmemory_server)
        .await;
    Mock::given(method("POST"))
        .and(path("/agentmemory/session/end"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&agentmemory_server)
        .await;
    Mock::given(method("POST"))
        .and(path("/agentmemory/summarize"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "success": false,
            "error": "no_observations"
        })))
        .expect(1)
        .mount(&agentmemory_server)
        .await;

    let mut builder = test_codex().with_config({
        let agentmemory_base_url = agentmemory_server.uri();
        move |config| {
            config.memories.backend = MemoryBackend::Agentmemory;
            config.memories.agentmemory.base_url = agentmemory_base_url;
            config.memories.agentmemory.inject_context = true;
            config.memories.use_memories = false;
            config
                .features
                .disable(Feature::MemoryTool)
                .expect("test config should allow feature update");
        }
    });
    let test = builder.build(&model_server).await?;

    test.submit_turn("ok").await?;
    test.codex.shutdown_and_wait().await?;

    let request = response.single_request();
    assert!(
        request
            .message_input_texts("developer")
            .contains(&startup_context.to_string()),
        "startup context should be injected into the first model turn",
    );

    let request_paths = agentmemory_server
        .received_requests()
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|request| request.url.path().to_string())
        .collect::<Vec<_>>();
    assert!(
        !request_paths.contains(&"/agentmemory/context".to_string()),
        "Claude parity startup injection should not fall back to /agentmemory/context",
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial(agentmemory_env)]
async fn session_start_context_respects_config_inject_context_flag() -> Result<()> {
    let model_server = start_mock_server().await;
    let response = mount_sse_once(
        &model_server,
        sse(vec![
            ev_response_created("resp-1"),
            ev_assistant_message("msg-1", "no startup context"),
            ev_completed("resp-1"),
        ]),
    )
    .await;
    let agentmemory_server = MockServer::start().await;
    let startup_context = "<agentmemory-context>hidden tide note</agentmemory-context>";

    Mock::given(method("POST"))
        .and(path("/agentmemory/session/start"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "context": startup_context,
        })))
        .expect(1)
        .mount(&agentmemory_server)
        .await;
    Mock::given(method("POST"))
        .and(path("/agentmemory/session/end"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&agentmemory_server)
        .await;
    Mock::given(method("POST"))
        .and(path("/agentmemory/summarize"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "success": false,
            "error": "no_observations"
        })))
        .expect(1)
        .mount(&agentmemory_server)
        .await;

    let mut builder = test_codex().with_config({
        let agentmemory_base_url = agentmemory_server.uri();
        move |config| {
            config.memories.backend = MemoryBackend::Agentmemory;
            config.memories.agentmemory.base_url = agentmemory_base_url;
            config.memories.agentmemory.inject_context = false;
            config.memories.use_memories = false;
            config
                .features
                .disable(Feature::MemoryTool)
                .expect("test config should allow feature update");
        }
    });
    let test = builder.build(&model_server).await?;

    test.submit_turn("hello").await?;
    test.codex.shutdown_and_wait().await?;

    let request = response.single_request();
    assert!(
        !request
            .message_input_texts("developer")
            .contains(&startup_context.to_string()),
        "startup context should stay out of the first model turn when inject_context is false",
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial(agentmemory_env)]
async fn prompt_submit_refresh_injects_context_even_when_startup_injection_is_disabled()
-> Result<()> {
    let model_server = start_mock_server().await;
    let response = mount_sse_once(
        &model_server,
        sse(vec![
            ev_response_created("resp-1"),
            ev_assistant_message("msg-1", "refreshed context visible"),
            ev_completed("resp-1"),
        ]),
    )
    .await;
    let agentmemory_server = MockServer::start().await;
    let refresh_context = "<agentmemory-context>prompt refresh tide note</agentmemory-context>";

    Mock::given(method("POST"))
        .and(path("/agentmemory/session/start"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "context": "",
        })))
        .expect(1)
        .mount(&agentmemory_server)
        .await;
    Mock::given(method("POST"))
        .and(path("/agentmemory/context/refresh"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "context": refresh_context,
            "skipped": false,
        })))
        .expect(1)
        .mount(&agentmemory_server)
        .await;
    Mock::given(method("POST"))
        .and(path("/agentmemory/session/end"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&agentmemory_server)
        .await;
    Mock::given(method("POST"))
        .and(path("/agentmemory/summarize"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "success": false,
            "error": "no_observations"
        })))
        .expect(1)
        .mount(&agentmemory_server)
        .await;

    let mut builder = test_codex().with_config({
        let agentmemory_base_url = agentmemory_server.uri();
        move |config| {
            config.memories.backend = MemoryBackend::Agentmemory;
            config.memories.agentmemory.base_url = agentmemory_base_url;
            config.memories.agentmemory.inject_context = false;
            config.memories.use_memories = false;
            config
                .features
                .disable(Feature::MemoryTool)
                .expect("test config should allow feature update");
        }
    });
    let test = builder.build(&model_server).await?;

    test.submit_turn("please review prompt refresh semantics carefully")
        .await?;
    test.codex.shutdown_and_wait().await?;

    let request = response.single_request();
    assert!(
        request
            .message_input_texts("developer")
            .contains(&refresh_context.to_string()),
        "prompt-submit refresh context should be injected into the model turn",
    );

    let request_paths = agentmemory_server
        .received_requests()
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|request| request.url.path().to_string())
        .collect::<Vec<_>>();
    assert!(
        request_paths.contains(&"/agentmemory/context/refresh".to_string()),
        "prompt submit should call /agentmemory/context/refresh",
    );

    Ok(())
}

fn normalize_request_summaries(
    mut summaries: Vec<(String, serde_json::Value)>,
) -> Vec<(String, serde_json::Value)> {
    for (path, body) in &mut summaries {
        if path == "/agentmemory/observe"
            && let Some(timestamp) = body.get_mut("timestamp")
        {
            *timestamp = serde_json::Value::String(request_summaries_placeholder_timestamp());
        }
        if path == "/agentmemory/observe"
            && let Some(event_id) = body.get_mut("event_id")
        {
            *event_id = serde_json::Value::String(request_summaries_placeholder_event_id());
        }
    }
    summaries
}

fn request_summaries_placeholder_timestamp() -> String {
    "__TIMESTAMP__".to_string()
}

fn request_summaries_placeholder_event_id() -> String {
    "__EVENT_ID__".to_string()
}

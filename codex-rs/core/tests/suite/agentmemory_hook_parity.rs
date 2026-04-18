use anyhow::Result;
use codex_config::types::MemoryBackend;
use codex_features::Feature;
use codex_models_manager::bundled_models_response;
use codex_protocol::items::MemoryOperationScope;
use codex_protocol::items::MemoryOperationStatus;
use codex_protocol::protocol::AskForApproval;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::MemoryOperationSource;
use codex_protocol::protocol::Op;
use codex_protocol::protocol::SandboxPolicy;
use codex_protocol::user_input::UserInput;
use core_test_support::responses::ev_apply_patch_function_call;
use core_test_support::responses::ev_assistant_message;
use core_test_support::responses::ev_completed;
use core_test_support::responses::ev_function_call;
use core_test_support::responses::ev_response_created;
use core_test_support::responses::mount_sse_sequence;
use core_test_support::responses::sse;
use core_test_support::responses::start_mock_server;
use core_test_support::test_codex::test_codex;
use pretty_assertions::assert_eq;
use serde_json::json;
use serial_test::serial;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::ResponseTemplate;
use wiremock::matchers::method;
use wiremock::matchers::path;

fn normalized_observe_payload(mut payload: serde_json::Value) -> serde_json::Value {
    if let Some(timestamp) = payload.get_mut("timestamp") {
        *timestamp = json!("__TIMESTAMP__");
    }
    if let Some(event_id) = payload.get_mut("event_id") {
        *event_id = json!("__EVENT_ID__");
    }
    payload
}

async fn mount_agentmemory_runtime(agentmemory_server: &MockServer) {
    Mock::given(method("POST"))
        .and(path("/agentmemory/session/start"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "context": "" })))
        .mount(agentmemory_server)
        .await;
    Mock::given(method("POST"))
        .and(path("/agentmemory/observe"))
        .respond_with(ResponseTemplate::new(200))
        .mount(agentmemory_server)
        .await;
    Mock::given(method("POST"))
        .and(path("/agentmemory/session/end"))
        .respond_with(ResponseTemplate::new(200))
        .mount(agentmemory_server)
        .await;
    Mock::given(method("POST"))
        .and(path("/agentmemory/summarize"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "success": false,
            "error": "no_observations",
        })))
        .mount(agentmemory_server)
        .await;
}

async fn submit_turn_and_collect_events(
    test: &core_test_support::test_codex::TestCodex,
    prompt: &str,
) -> Result<Vec<EventMsg>> {
    test.codex
        .submit(Op::UserTurn {
            items: vec![UserInput::Text {
                text: prompt.to_string(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
            cwd: test.config.cwd.to_path_buf(),
            approval_policy: AskForApproval::Never,
            approvals_reviewer: None,
            sandbox_policy: SandboxPolicy::DangerFullAccess,
            model: test.session_configured.model.clone(),
            effort: None,
            summary: None,
            service_tier: None,
            collaboration_mode: None,
            personality: None,
        })
        .await?;

    let mut turn_id = None;
    let mut events = Vec::new();
    loop {
        let event = test.codex.next_event().await?;
        let msg = event.msg;
        if turn_id.is_none()
            && let EventMsg::TurnStarted(turn_started) = &msg
        {
            turn_id = Some(turn_started.turn_id.clone());
        }
        let is_complete = matches!(
            (&turn_id, &msg),
            (Some(turn_id), EventMsg::TurnComplete(turn_complete))
                if turn_complete.turn_id == *turn_id
        );
        events.push(msg);
        if is_complete {
            break;
        }
    }
    Ok(events)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial(agentmemory_env)]
async fn pre_tool_enrichment_injects_context_for_write_lane() -> Result<()> {
    let model_server = start_mock_server().await;
    let agentmemory_server = MockServer::start().await;
    mount_agentmemory_runtime(&agentmemory_server).await;
    Mock::given(method("POST"))
        .and(path("/agentmemory/enrich"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "context": "<agentmemory-context>glob tide note</agentmemory-context>",
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
            config.include_apply_patch_tool = true;
            config
                .features
                .disable(Feature::MemoryTool)
                .expect("test config should allow feature update");
        }
    });
    let test = builder.build(&model_server).await?;
    let call_id = "agentmemory-apply-patch";
    let patch =
        "*** Begin Patch\n*** Add File: agentmemory_pretool.txt\n+harbor note\n*** End Patch";
    let responses = mount_sse_sequence(
        &model_server,
        vec![
            sse(vec![
                ev_response_created("resp-1"),
                ev_apply_patch_function_call(call_id, patch),
                ev_completed("resp-1"),
            ]),
            sse(vec![
                ev_response_created("resp-2"),
                ev_assistant_message("msg-1", "patched"),
                ev_completed("resp-2"),
            ]),
        ],
    )
    .await;

    test.submit_turn("list the current directory").await?;
    test.codex.shutdown_and_wait().await?;

    let requests = responses.requests();
    assert_eq!(requests.len(), 2);
    let agentmemory_requests = agentmemory_server
        .received_requests()
        .await
        .unwrap_or_default();
    let agentmemory_paths = agentmemory_requests
        .iter()
        .map(|request| request.url.path().to_string())
        .collect::<Vec<_>>();
    let observe_bodies = agentmemory_requests
        .iter()
        .filter(|request| request.url.path() == "/agentmemory/observe")
        .map(|request| String::from_utf8_lossy(&request.body).into_owned())
        .collect::<Vec<_>>();
    assert!(
        agentmemory_paths.contains(&"/agentmemory/enrich".to_string()),
        "write lane should call /agentmemory/enrich: {agentmemory_paths:?}; observe={observe_bodies:?}",
    );
    assert!(
        requests[1].body_contains_text("glob tide note"),
        "pre-tool enrichment should inject context into the follow-up model turn",
    );
    let post_tool_use = agentmemory_requests
        .into_iter()
        .filter(|request| request.url.path() == "/agentmemory/observe")
        .map(|request| {
            serde_json::from_slice::<serde_json::Value>(&request.body)
                .expect("agentmemory observe body should be valid json")
        })
        .find(|payload| {
            payload["hookType"] == "post_tool_use"
                && payload["data"]["tool_use_id"] == call_id
                && payload["data"]["tool_name"] == "Write"
        })
        .expect("apply_patch should emit post_tool_use observe payload");
    let normalized = normalized_observe_payload(post_tool_use);
    assert_eq!(
        normalized["sessionId"],
        test.session_configured.session_id.to_string()
    );
    assert_eq!(normalized["hookType"], "post_tool_use");
    assert_eq!(normalized["project"], test.config.cwd.display().to_string());
    assert_eq!(normalized["cwd"], test.config.cwd.display().to_string());
    assert_eq!(normalized["source"], "codex-native");
    assert_eq!(normalized["payload_version"], "1");
    assert_eq!(normalized["event_id"], "__EVENT_ID__");
    assert_eq!(
        normalized["capabilities"],
        json!([
            "structured_post_tool_payload",
            "query_aware_context",
            "event_identity",
        ]),
    );
    assert_eq!(normalized["persistence_class"], "persistent");
    assert_eq!(
        normalized["data"]["session_id"],
        test.session_configured.session_id.to_string(),
    );
    assert_eq!(
        normalized["data"]["cwd"],
        test.config.cwd.display().to_string()
    );
    assert_eq!(normalized["data"]["tool_name"], "Write");
    assert_eq!(normalized["data"]["tool_use_id"], call_id);
    assert_eq!(
        normalized["data"]["tool_input"],
        json!({
            "patch": patch,
            "paths": ["agentmemory_pretool.txt"],
        }),
    );
    assert_eq!(
        normalized["data"]["tool_output"]["metadata"]["exit_code"],
        0,
    );
    assert!(normalized["data"]["tool_output"]["metadata"]["duration_seconds"].is_number(),);
    assert_eq!(
        normalized["data"]["tool_output"]["output"],
        "Success. Updated the following files:\nA agentmemory_pretool.txt\n",
    );
    assert!(normalized["data"]["turn_id"].is_string());
    assert!(normalized["data"]["model"].is_string());

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial(agentmemory_env)]
async fn pre_tool_enrichment_skips_non_matching_tools() -> Result<()> {
    let model_server = start_mock_server().await;
    let agentmemory_server = MockServer::start().await;
    mount_agentmemory_runtime(&agentmemory_server).await;

    let mut builder = test_codex().with_config({
        let agentmemory_base_url = agentmemory_server.uri();
        move |config| {
            config.memories.backend = MemoryBackend::Agentmemory;
            config.memories.agentmemory.base_url = agentmemory_base_url;
            config.memories.agentmemory.inject_context = true;
            config
                .features
                .disable(Feature::MemoryTool)
                .expect("test config should allow feature update");
        }
    });
    let test = builder.build(&model_server).await?;
    let call_id = "agentmemory-update-plan";
    let args = json!({
        "plan": [{
            "step": "watch the tide",
            "status": "pending",
        }]
    });
    let _responses = mount_sse_sequence(
        &model_server,
        vec![
            sse(vec![
                ev_response_created("resp-1"),
                ev_function_call(call_id, "update_plan", &serde_json::to_string(&args)?),
                ev_completed("resp-1"),
            ]),
            sse(vec![
                ev_response_created("resp-2"),
                ev_assistant_message("msg-1", "updated"),
                ev_completed("resp-2"),
            ]),
        ],
    )
    .await;

    test.submit_turn("update the plan").await?;
    test.codex.shutdown_and_wait().await?;

    let request_paths = agentmemory_server
        .received_requests()
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|request| request.url.path().to_string())
        .collect::<Vec<_>>();
    assert!(
        !request_paths.contains(&"/agentmemory/enrich".to_string()),
        "non-matching tools should not trigger agentmemory enrichment",
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial(agentmemory_env)]
async fn glob_lane_enrichment_and_post_tool_capture_use_native_contract() -> Result<()> {
    let model_server = start_mock_server().await;
    let agentmemory_server = MockServer::start().await;
    mount_agentmemory_runtime(&agentmemory_server).await;
    Mock::given(method("POST"))
        .and(path("/agentmemory/enrich"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "context": "<agentmemory-context>glob lane note</agentmemory-context>",
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
            let mut model_catalog = bundled_models_response()
                .unwrap_or_else(|err| panic!("bundled models.json should parse: {err}"));
            let model = model_catalog
                .models
                .iter_mut()
                .find(|model| model.slug == "gpt-5.3-codex")
                .expect("gpt-5.3-codex exists in bundled models.json");
            model
                .experimental_supported_tools
                .push("list_dir".to_string());
            config.model_catalog = Some(model_catalog);
            config
                .features
                .disable(Feature::MemoryTool)
                .expect("test config should allow feature update");
        }
    });
    let test = builder.build(&model_server).await?;
    std::fs::write(test.config.cwd.join("glob_lane.txt"), "harbor note")?;
    let call_id = "agentmemory-list-dir";
    let args = json!({
        "dir_path": test.config.cwd.display().to_string(),
        "offset": 1,
        "limit": 5,
        "depth": 1,
    });
    let responses = mount_sse_sequence(
        &model_server,
        vec![
            sse(vec![
                ev_response_created("resp-1"),
                ev_function_call(call_id, "list_dir", &serde_json::to_string(&args)?),
                ev_completed("resp-1"),
            ]),
            sse(vec![
                ev_response_created("resp-2"),
                ev_assistant_message("msg-1", "listed"),
                ev_completed("resp-2"),
            ]),
        ],
    )
    .await;

    test.submit_turn("list the directory").await?;
    test.codex.shutdown_and_wait().await?;

    let requests = responses.requests();
    assert_eq!(requests.len(), 2);
    let agentmemory_requests = agentmemory_server
        .received_requests()
        .await
        .unwrap_or_default();
    let agentmemory_paths = agentmemory_requests
        .iter()
        .map(|request| request.url.path().to_string())
        .collect::<Vec<_>>();
    let observe_bodies = agentmemory_requests
        .iter()
        .filter(|request| request.url.path() == "/agentmemory/observe")
        .map(|request| String::from_utf8_lossy(&request.body).into_owned())
        .collect::<Vec<_>>();
    assert!(
        requests[1].body_contains_text("glob lane note"),
        "glob lane enrichment should inject context into the follow-up model turn: body={}; paths={agentmemory_paths:?}; observe={observe_bodies:?}",
        requests[1].body_json(),
    );
    let post_tool_use = agentmemory_requests
        .into_iter()
        .filter(|request| request.url.path() == "/agentmemory/observe")
        .map(|request| {
            serde_json::from_slice::<serde_json::Value>(&request.body)
                .expect("agentmemory observe body should be valid json")
        })
        .find(|payload| {
            payload["hookType"] == "post_tool_use" && payload["data"]["tool_name"] == "Glob"
        })
        .expect("list_dir should emit glob post_tool_use observe payload");
    assert_eq!(post_tool_use["source"], "codex-native");
    assert_eq!(post_tool_use["payload_version"], "1");
    assert_eq!(post_tool_use["persistence_class"], "persistent");

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial(agentmemory_env)]
async fn user_turn_retrieval_falls_back_to_context_and_emits_automatic_ready_event() -> Result<()> {
    let model_server = start_mock_server().await;
    let agentmemory_server = MockServer::start().await;
    mount_agentmemory_runtime(&agentmemory_server).await;
    Mock::given(method("POST"))
        .and(path("/agentmemory/context/refresh"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "context": "",
            "skipped": true,
        })))
        .expect(1)
        .mount(&agentmemory_server)
        .await;
    Mock::given(method("POST"))
        .and(path("/agentmemory/context"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "context": "<agentmemory-context>fallback harbor note</agentmemory-context>",
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
            config
                .features
                .disable(Feature::MemoryTool)
                .expect("test config should allow feature update");
        }
    });
    let test = builder.build(&model_server).await?;
    let responses = mount_sse_sequence(
        &model_server,
        vec![sse(vec![
            ev_response_created("resp-1"),
            ev_assistant_message("msg-1", "done"),
            ev_completed("resp-1"),
        ])],
    )
    .await;

    let events = submit_turn_and_collect_events(&test, "fix the harbor regression").await?;
    test.codex.shutdown_and_wait().await?;

    let requests = responses.requests();
    assert_eq!(requests.len(), 1);
    assert!(
        requests[0].body_contains_text("fallback harbor note"),
        "fallback /agentmemory/context result should be injected into the turn body",
    );
    let automatic_event = events
        .into_iter()
        .find_map(|event| match event {
            EventMsg::MemoryOperation(event)
                if event.source == MemoryOperationSource::Automatic =>
            {
                Some(event)
            }
            _ => None,
        })
        .expect("automatic memory event should be emitted");
    assert_eq!(automatic_event.status, MemoryOperationStatus::Ready);
    assert_eq!(automatic_event.scope, MemoryOperationScope::Turn);
    assert_eq!(automatic_event.context_injected, true);
    let detail = automatic_event
        .detail
        .expect("automatic event should include detail");
    assert!(
        detail.contains("\"fallback_endpoint\": \"context_refresh\""),
        "detail should record refresh->context fallback: {detail}",
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial(agentmemory_env)]
async fn assistant_memory_recall_thread_scope_persists_and_reports_scope() -> Result<()> {
    let model_server = start_mock_server().await;
    let agentmemory_server = MockServer::start().await;
    mount_agentmemory_runtime(&agentmemory_server).await;
    Mock::given(method("POST"))
        .and(path("/agentmemory/context"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "context": "<agentmemory-context>thread recall note</agentmemory-context>",
        })))
        .expect(1)
        .mount(&agentmemory_server)
        .await;

    let mut builder = test_codex().with_config({
        let agentmemory_base_url = agentmemory_server.uri();
        move |config| {
            config.memories.backend = MemoryBackend::Agentmemory;
            config.memories.agentmemory.base_url = agentmemory_base_url;
            let _ = config.features.enable(Feature::MemoryTool);
        }
    });
    let test = builder.build(&model_server).await?;
    let responses = mount_sse_sequence(
        &model_server,
        vec![
            sse(vec![
                ev_response_created("resp-1"),
                ev_function_call(
                    "memory-recall-1",
                    "memory_recall",
                    &serde_json::to_string(&json!({
                        "query": "thread recall note",
                        "scope": "thread",
                    }))?,
                ),
                ev_completed("resp-1"),
            ]),
            sse(vec![
                ev_response_created("resp-2"),
                ev_assistant_message("msg-1", "used recall"),
                ev_completed("resp-2"),
            ]),
        ],
    )
    .await;

    let events = submit_turn_and_collect_events(&test, "use memory recall").await?;
    test.codex.shutdown_and_wait().await?;

    let recall_event = events
        .into_iter()
        .find_map(|event| match event {
            EventMsg::MemoryOperation(event)
                if event.source == MemoryOperationSource::Assistant =>
            {
                Some(event)
            }
            _ => None,
        })
        .expect("assistant recall event should be emitted");
    assert_eq!(recall_event.status, MemoryOperationStatus::Ready);
    assert_eq!(recall_event.scope, MemoryOperationScope::Thread);
    assert_eq!(recall_event.context_injected, true);
    let requests = responses.requests();
    assert_eq!(requests.len(), 2);
    assert!(
        requests[1].body_contains_text("thread recall note"),
        "follow-up model request should include persisted recall context",
    );

    Ok(())
}

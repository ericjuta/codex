use anyhow::Result;
use codex_config::types::MemoryBackend;
use codex_features::Feature;
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

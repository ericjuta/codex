use anyhow::Result;
use app_test_support::McpProcess;
use app_test_support::create_mock_responses_server_repeating_assistant;
use app_test_support::to_response;
use codex_app_server_protocol::JSONRPCResponse;
use codex_app_server_protocol::RequestId;
use codex_app_server_protocol::ThreadCloseParams;
use codex_app_server_protocol::ThreadCloseResponse;
use codex_app_server_protocol::ThreadClosedNotification;
use codex_app_server_protocol::ThreadLoadedListParams;
use codex_app_server_protocol::ThreadLoadedListResponse;
use codex_app_server_protocol::ThreadStartParams;
use codex_app_server_protocol::ThreadStartResponse;
use pretty_assertions::assert_eq;
use serde_json::json;
use tempfile::TempDir;
use tokio::time::timeout;
use wiremock::Mock;
use wiremock::ResponseTemplate;
use wiremock::matchers::method;
use wiremock::matchers::path;

const DEFAULT_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

#[tokio::test]
async fn thread_close_shuts_down_and_unloads_thread() -> Result<()> {
    let server = create_mock_responses_server_repeating_assistant("Done").await;
    mount_agentmemory_lifecycle(&server).await;
    let codex_home = TempDir::new()?;
    create_config_toml(codex_home.path(), &server.uri())?;

    let mut mcp = McpProcess::new(codex_home.path()).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let start_id = mcp
        .send_thread_start_request(ThreadStartParams {
            model: Some("mock-model".to_string()),
            ..Default::default()
        })
        .await?;
    let start_resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(start_id)),
    )
    .await??;
    let ThreadStartResponse { thread, .. } = to_response::<ThreadStartResponse>(start_resp)?;

    let close_id = mcp
        .send_thread_close_request(ThreadCloseParams {
            thread_id: thread.id.clone(),
        })
        .await?;
    let close_resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(close_id)),
    )
    .await??;
    let _: ThreadCloseResponse = to_response::<ThreadCloseResponse>(close_resp)?;

    let closed = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_notification_message("thread/closed"),
    )
    .await??;
    let closed: ThreadClosedNotification =
        serde_json::from_value(closed.params.expect("thread/closed params"))?;
    assert_eq!(closed.thread_id, thread.id);

    let list_id = mcp
        .send_thread_loaded_list_request(ThreadLoadedListParams::default())
        .await?;
    let list_resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(list_id)),
    )
    .await??;
    let loaded = to_response::<ThreadLoadedListResponse>(list_resp)?;
    assert_eq!(
        loaded,
        ThreadLoadedListResponse {
            data: Vec::new(),
            next_cursor: None,
        }
    );

    let requests = server
        .received_requests()
        .await
        .expect("wiremock should record requests");
    let closeout_request = requests
        .iter()
        .find(|request| request.url.path() == "/agentmemory/session/closeout")
        .expect("thread close should close out agentmemory session");
    let closeout_body: serde_json::Value = serde_json::from_slice(&closeout_request.body)?;
    assert_eq!(closeout_body, json!({ "sessionId": thread.id }));
    server.verify().await;

    Ok(())
}

async fn mount_agentmemory_lifecycle(server: &wiremock::MockServer) {
    Mock::given(method("POST"))
        .and(path("/agentmemory/session/start"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "context": "" })))
        .expect(1)
        .mount(server)
        .await;
    Mock::given(method("POST"))
        .and(path("/agentmemory/observe"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .expect(2)
        .mount(server)
        .await;
    Mock::given(method("POST"))
        .and(path("/agentmemory/session/closeout"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "success": true,
            "steps": {
                "summarize": "ok",
                "endSession": "ok",
                "crystallize": "ok",
                "consolidate": "ok"
            }
        })))
        .expect(1)
        .mount(server)
        .await;
}

fn create_config_toml(codex_home: &std::path::Path, server_uri: &str) -> std::io::Result<()> {
    std::fs::write(
        codex_home.join("config.toml"),
        format!(
            r#"
model = "mock-model"
model_provider = "mock_provider"

[model_providers.mock_provider]
name = "Mock provider"
base_url = "{server_uri}/v1"
wire_api = "responses"
request_max_retries = 0
stream_max_retries = 0

[memories]
backend = "agentmemory"

[memories.agentmemory]
base_url = "{server_uri}"
inject_context = false
"#
        ),
    )
}

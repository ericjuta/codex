use std::ffi::OsStr;
use std::ffi::OsString;

use anyhow::Result;
use codex_config::types::MemoryBackend;
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

struct EnvVarGuard {
    key: &'static str,
    original: Option<OsString>,
}

impl EnvVarGuard {
    fn set(key: &'static str, value: &OsStr) -> Self {
        let original = std::env::var_os(key);
        unsafe {
            std::env::set_var(key, value);
        }
        Self { key, original }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        unsafe {
            match &self.original {
                Some(value) => std::env::set_var(self.key, value),
                None => std::env::remove_var(self.key),
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial(agentmemory_env)]
async fn agentmemory_session_lifecycle_is_registered_end_to_end() -> Result<()> {
    let model_server = start_mock_server().await;
    let agentmemory_server = MockServer::start().await;
    let agentmemory_port = agentmemory_server
        .uri()
        .rsplit(':')
        .next()
        .expect("mock server uri should include port")
        .to_string();
    let _agentmemory_port_guard = EnvVarGuard::set("III_REST_PORT", OsStr::new(&agentmemory_port));

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

    let mut builder = test_codex().with_config(|config| {
        config.memories.backend = MemoryBackend::Agentmemory;
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

    assert_eq!(
        request_summaries,
        vec![
            ("/agentmemory/session/start".to_string(), expected_start),
            ("/agentmemory/session/end".to_string(), expected_end),
        ]
    );
    agentmemory_server.verify().await;

    Ok(())
}

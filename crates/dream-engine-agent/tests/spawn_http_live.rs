//! Manual transport acceptance using the loopback fixture in tests/fixtures.
mod common;

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Arc;
use std::time::Duration;

use dream_engine_agent::spawner::{AgentSpawner, SubAgentConfig};
use dream_engine_agent::tool_policy::ToolPolicy;
use dream_engine_config::compat::ProviderCompat;
use dream_engine_config::config::ProviderType;
use dream_engine_providers::openai::OpenAIProvider;
use serde_json::Value;
use tokio::time::{sleep, timeout};

fn status() -> Value {
    let mut socket = TcpStream::connect("127.0.0.1:29529").expect("start tests/fixtures/spawn_cancel_upstream.py");
    socket.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    socket
        .write_all(b"GET /status HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .unwrap();
    let mut response = String::new();
    socket.read_to_string(&mut response).unwrap();
    serde_json::from_str(response.split_once("\r\n\r\n").unwrap().1).unwrap()
}

#[tokio::test]
#[ignore = "requires python tests/fixtures/spawn_cancel_upstream.py on loopback port 29529"]
async fn parent_cancel_closes_all_five_quiet_http_streams_while_host_stays_alive() {
    let run = format!("sdk_{}", std::process::id());
    let mut config = common::test_config();
    config.provider = ProviderType::OpenAI;
    config.provider_label = "openai".into();
    config.model = "qa-spawn".into();
    config.compat = ProviderCompat::openai_defaults();
    let provider = Arc::new(OpenAIProvider::new(
        "qa",
        "http://127.0.0.1:29529/v1",
        config.compat.clone(),
    ));
    let spawner = AgentSpawner::new(provider, config, std::env::temp_dir(), ToolPolicy::Unrestricted);
    let tasks = (0..5)
        .map(|i| SubAgentConfig {
            name: format!("child-{i}"),
            prompt: format!("QA_SPAWN_CHILD:{run}:{i}"),
            max_turns: 2,
            max_tokens: 64,
            system_prompt: None,
        })
        .collect();
    let parent = tokio::spawn(async move { spawner.spawn_parallel(tasks).await });
    timeout(Duration::from_secs(10), async {
        while status()[&run]["active"] != 5 {
            assert!(!parent.is_finished(), "parent failed before all child streams opened");
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("all five streams opened");
    parent.abort();
    assert!(parent.await.unwrap_err().is_cancelled());
    timeout(Duration::from_secs(3), async {
        while status()[&run]["active"] != 0 {
            sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("cancellation closes silent HTTP streams without waiting for another token");
    sleep(Duration::from_millis(250)).await;
    let final_state = status()[&run].clone();
    assert_eq!(final_state["closed"], 5);
    assert_eq!(final_state["started"], 5, "no retries after cancellation");
    println!("{run}: {final_state}");
}

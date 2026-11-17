//! Persistent launch preserves user-owned Hermes settings and refuses to
//! overwrite malformed configuration, through the public agent API.

use cua_agents::harness::Endpoint;
use cua_agents::{Agents, RunOptions};
use cua_spacesd_client::testing::{MockAuth, MockServer};
use cua_spacesd_client::{ConnectOptions, SpacesdClient};
use serde_yaml_ng::Value;

#[tokio::test]
async fn persistent_hermes_launch_preserves_settings_and_refuses_invalid_config() {
    let server = MockServer::start(MockAuth::default()).await;
    let guest =
        SpacesdClient::connect(ConnectOptions::parse(&format!("http://{}", server.addr)).unwrap())
            .await
            .unwrap();
    let agents = Agents::new(guest).await.unwrap();
    let path = format!("{}/hermes/config.yaml", agents.agent_home_dir("check"));
    let existing = b"model:\n  provider: custom\n  default: old-model\n  temperature: 0.4\nterminal:\n  backend: local\nmcp_servers:\n  documents:\n    command: document-server\n";
    server.state.put_file(&path, existing.to_vec());
    let options = || {
        RunOptions {
        home: Some("check".into()),
        install: Some(false),
        skills: Some(false),
        hermes_config: Some("model:\n  provider: custom\n  default: new-model\n  base_url: https://endpoint.example/v1\n".into()),
        ..Default::default()
    }
    };
    agents.start("hermes", "hello", options()).await.unwrap();
    let saved: Value = serde_yaml_ng::from_slice(&server.state.file(&path).unwrap()).unwrap();
    assert_eq!(saved["model"]["default"].as_str(), Some("new-model"));
    assert_eq!(saved["model"]["temperature"].as_f64(), Some(0.4));
    assert_eq!(saved["terminal"]["backend"].as_str(), Some("local"));
    assert_eq!(
        saved["mcp_servers"]["documents"]["command"].as_str(),
        Some("document-server")
    );

    let invalid = b"model: [unexpected-shape]\n";
    server.state.put_file(&path, invalid.to_vec());
    let error = agents
        .start("hermes", "hello again", options())
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        format!("invalid argument: cannot update Hermes config {path}: model must be a mapping")
    );
    assert_eq!(server.state.file(&path).unwrap(), invalid);
}

#[tokio::test]
async fn persistent_hermes_endpoint_refresh_keeps_context_and_tools() {
    let server = MockServer::start(MockAuth::default()).await;
    let guest =
        SpacesdClient::connect(ConnectOptions::parse(&format!("http://{}", server.addr)).unwrap())
            .await
            .unwrap();
    let agents = Agents::new(guest).await.unwrap();
    let path = format!("{}/hermes/config.yaml", agents.agent_home_dir("check"));
    server.state.put_file(&path, b"model:\n  provider: old-provider\n  default: old-model\n  context_length: 32768\nmcp_servers:\n  documents:\n    command: document-server\n".to_vec());
    agents
        .start(
            "hermes",
            "hello",
            RunOptions {
                home: Some("check".into()),
                install: Some(false),
                skills: Some(false),
                endpoint: Some(Endpoint {
                    base_url: "https://endpoint.example".into(),
                    model: Some("new-model".into()),
                    wire: None,
                }),
                // The explicit endpoint takes precedence over this option.
                hermes_config: Some("invalid: [".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let saved: Value = serde_yaml_ng::from_slice(&server.state.file(&path).unwrap()).unwrap();
    assert_eq!(saved["model"]["provider"].as_str(), Some("custom"));
    assert_eq!(saved["model"]["default"].as_str(), Some("new-model"));
    assert_eq!(
        saved["model"]["base_url"].as_str(),
        Some("https://endpoint.example/v1")
    );
    assert_eq!(saved["model"]["context_length"].as_u64(), Some(32768));
    assert_eq!(
        saved["mcp_servers"]["documents"]["command"].as_str(),
        Some("document-server")
    );
}

#[tokio::test]
async fn one_shot_hermes_keeps_supplied_config_verbatim() {
    let server = MockServer::start(MockAuth::default()).await;
    let guest =
        SpacesdClient::connect(ConnectOptions::parse(&format!("http://{}", server.addr)).unwrap())
            .await
            .unwrap();
    let agents = Agents::new(guest).await.unwrap();
    let supplied = "# one-shot settings\nmodel: {provider: custom, default: example}\nterminal: {backend: local}\n";
    let run = agents
        .start(
            "hermes",
            "hello",
            RunOptions {
                install: Some(false),
                skills: Some(false),
                hermes_config: Some(supplied.into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let path = format!("{}/hermes-home/config.yaml", agents.run_dir(&run.run_id));
    assert_eq!(server.state.file(&path).unwrap(), supplied.as_bytes());
}

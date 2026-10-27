// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Child-Space tools in the Space's own MCP server: `create_space`,
//! `fork_space`, `list_spaces`, `delete_space`, `stop_space`,
//! `start_space` and `space_bash`, for the Space's own children only.
//!
//! They call the Space's spawn channel (`TunnelService.StartSpawn`) with the
//! Space's capability, so every harness that uses this MCP server gets them
//! with no setup. The host answers them for this Space's subtree alone;
//! this server adds no authority of its own.

use std::sync::Arc;

use cua_driver_core::server::ToolProvider;
use cua_proto::daemon::v1 as dpb;
use cua_proto::daemon::v1::space_service_client::SpaceServiceClient;
use serde_json::{json, Value};

use crate::context::ServerContext;

/// Names this provider adds.
pub const TOOLS: &[&str] = &[
    "create_space",
    "fork_space",
    "list_spaces",
    "delete_space",
    "stop_space",
    "start_space",
    "space_bash",
];

const SCOPE: &str = "Children of this Space only: Spaces it made (and theirs). \
Other Spaces, including its parent and siblings, are not reachable.";

fn defs() -> Vec<Value> {
    let space = json!({"type": "string", "description": "A child's name or id."});
    vec![
        json!({"name": "create_space", "description": format!("Creates a child Space of this Space, on the same machine, and waits until it is ready. It keeps this Space's network rules and ends no later than this Space. {SCOPE}"),
        "inputSchema": {"type": "object", "properties": {
            "name": {"type": "string", "description": "Name (default: <this Space>.c<n>)."},
            "image": {"type": "string", "description": "Image (default: the Linux image)."},
            "kind": {"type": "string", "description": "\"container\" or \"vm\" (default: decided from the image)."},
            "cpus": {"type": "integer"},
            "memory_mb": {"type": "integer"},
            "headless": {"type": "boolean", "description": "No desktop: only commands and files."}
        }}}),
        json!({"name": "fork_space", "description": format!("Forks this Space (or one of its children) into a new child Space. {SCOPE}"),
        "inputSchema": {"type": "object", "properties": {
            "source": {"type": "string", "description": "A child to fork (default: this Space)."},
            "name": {"type": "string"}
        }}}),
        json!({"name": "list_spaces", "description": format!("Lists this Space's children. {SCOPE}"),
            "inputSchema": {"type": "object", "properties": {}}}),
        json!({"name": "delete_space", "description": format!("Deletes a child Space and everything below it. {SCOPE}"),
            "inputSchema": {"type": "object", "properties": {"space": space}, "required": ["space"]}}),
        json!({"name": "stop_space", "description": format!("Turns a child Space off (its disk is kept). {SCOPE}"),
            "inputSchema": {"type": "object", "properties": {"space": space}, "required": ["space"]}}),
        json!({"name": "start_space", "description": format!("Turns a child Space on again. {SCOPE}"),
            "inputSchema": {"type": "object", "properties": {"space": space}, "required": ["space"]}}),
        json!({"name": "space_bash", "description": format!("Runs a bash command in a child Space and returns its output and exit code. {SCOPE}"),
            "inputSchema": {"type": "object", "properties": {
                "space": space,
                "command": {"type": "string"},
                "timeout": {"type": "integer", "description": "Seconds (default 60)."}
            }, "required": ["space", "command"]}}),
    ]
}

/// The driver's tools plus the child-Space tools.
pub struct WithChildTools {
    inner: Arc<dyn ToolProvider>,
    ctx: ServerContext,
}

impl WithChildTools {
    /// Wraps `inner`.
    pub fn new(inner: Arc<dyn ToolProvider>, ctx: ServerContext) -> Self {
        Self { inner, ctx }
    }

    /// The spawn channel's URL and capability, while it is started.
    fn channel(&self) -> Result<(String, String), String> {
        let env = self.ctx.init_state().spawn_env;
        let url = env.get(super::spawn::ENV_ADDRESS).cloned();
        let file = env.get(super::spawn::ENV_TOKEN_FILE).cloned();
        let (Some(url), Some(file)) = (url, file) else {
            return Err(
                "this Space cannot make child Spaces: its owner has not turned \
                        children on for it"
                    .into(),
            );
        };
        let token = std::fs::read_to_string(&file)
            .map(|t| t.trim().to_owned())
            .map_err(|e| format!("the spawn capability is unreadable: {e}"))?;
        Ok((url, token))
    }

    async fn client(
        &self,
    ) -> Result<
        SpaceServiceClient<
            tonic::service::interceptor::InterceptedService<
                tonic::transport::Channel,
                impl tonic::service::Interceptor,
            >,
        >,
        String,
    > {
        let (url, token) = self.channel()?;
        let channel = tonic::transport::Endpoint::from_shared(url)
            .map_err(|e| e.to_string())?
            .connect()
            .await
            .map_err(|e| format!("the spawn channel is not reachable: {e}"))?;
        let bearer: tonic::metadata::MetadataValue<_> = format!("Bearer {token}")
            .parse()
            .map_err(|_| "the spawn capability is not a valid header".to_string())?;
        Ok(
            SpaceServiceClient::with_interceptor(channel, move |mut req: tonic::Request<()>| {
                req.metadata_mut().insert("authorization", bearer.clone());
                Ok(req)
            })
            .max_decoding_message_size(64 << 20),
        )
    }

    async fn call(&self, name: &str, a: Value) -> Result<Value, String> {
        let mut c = self.client().await?;
        let s = |k: &str| {
            a.get(k)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned()
        };
        let status = |e: tonic::Status| e.message().to_owned();
        Ok(match name {
            "create_space" => {
                let r = c
                    .create_space(dpb::CreateSpaceRequest {
                        name: s("name"),
                        image: s("image"),
                        kind: s("kind"),
                        cpus: a.get("cpus").and_then(Value::as_u64).unwrap_or(0) as u32,
                        memory_mb: a.get("memory_mb").and_then(Value::as_u64).unwrap_or(0),
                        headless: a.get("headless").and_then(Value::as_bool).unwrap_or(false),
                        wait: Some(true),
                        ..Default::default()
                    })
                    .await
                    .map_err(status)?
                    .into_inner();
                r.space.map(row).unwrap_or(Value::Null)
            }
            "fork_space" => c
                .fork_space(dpb::ForkSpaceRequest {
                    source: s("source"),
                    name: s("name"),
                })
                .await
                .map_err(status)?
                .into_inner()
                .space
                .map(row)
                .unwrap_or(Value::Null),
            "list_spaces" => {
                let r = c
                    .list_spaces(dpb::ListSpacesRequest::default())
                    .await
                    .map_err(status)?
                    .into_inner();
                json!({"spaces": r.spaces.into_iter().map(row).collect::<Vec<_>>()})
            }
            "delete_space" => {
                let r = c
                    .delete_space(dpb::DeleteSpaceRequest { space: s("space") })
                    .await
                    .map_err(status)?
                    .into_inner();
                json!({"deleted": s("space"), "message": r.message})
            }
            _ => {
                // stop_space, start_space, space_bash: the host's tools,
                // which the channel answers for children only.
                let r = c
                    .call_space_tool(dpb::CallSpaceToolRequest {
                        name: name.to_owned(),
                        arguments_json: a.to_string(),
                    })
                    .await
                    .map_err(status)?
                    .into_inner();
                let content: Value =
                    serde_json::from_str(&r.content_json).unwrap_or(Value::Array(vec![]));
                return Ok(json!({"content": content, "isError": r.is_error}));
            }
        })
    }
}

/// A child as the tools show it.
fn row(s: dpb::Space) -> Value {
    json!({
        "id": s.id,
        "name": if s.display_name.is_empty() { s.name } else { s.display_name },
        "parent": s.labels.get("cua.parent").cloned().unwrap_or_default(),
        "power": s.power,
        "image": s.image,
        "os": s.os,
        "expires_at_unix": s.expires_at_unix,
    })
}

fn text_result(text: String, is_error: bool) -> Value {
    json!({"content": [{"type": "text", "text": text}], "isError": is_error})
}

#[async_trait::async_trait]
impl ToolProvider for WithChildTools {
    fn tools_list(&self) -> Value {
        let mut list = self.inner.tools_list();
        let defs = defs();
        match list.get_mut("tools").and_then(Value::as_array_mut) {
            Some(tools) => {
                tools.retain(|t| {
                    t.get("name")
                        .and_then(Value::as_str)
                        .is_none_or(|n| !TOOLS.contains(&n))
                });
                tools.extend(defs);
            }
            None => list = json!({"tools": defs}),
        }
        list
    }

    fn is_known_tool(&self, name: &str) -> bool {
        TOOLS.contains(&name) || self.inner.is_known_tool(name)
    }

    async fn invoke_tool(&self, name: &str, arguments: Value) -> Result<Value, String> {
        if !TOOLS.contains(&name) {
            return self.inner.invoke_tool(name, arguments).await;
        }
        Ok(match self.call(name, arguments).await {
            Ok(v) if v.get("content").is_some() => v,
            Ok(v) => text_result(serde_json::to_string_pretty(&v).unwrap_or_default(), false),
            Err(e) => text_result(e, true),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Driver;

    #[async_trait::async_trait]
    impl ToolProvider for Driver {
        fn tools_list(&self) -> Value {
            json!({"tools": [{"name": "click"}]})
        }
        async fn invoke_tool(&self, name: &str, _: Value) -> Result<Value, String> {
            Ok(text_result(format!("driver {name}"), false))
        }
    }

    #[tokio::test]
    async fn child_tools_join_the_driver_and_say_when_children_are_off() {
        let ctx = ServerContext::new(crate::config::ServerConfig::default(), Some("t".into()));
        let p = WithChildTools::new(Arc::new(Driver), ctx);
        let names: Vec<String> = p.tools_list()["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_owned())
            .collect();
        assert!(names.contains(&"click".to_owned()));
        for t in TOOLS {
            assert!(names.contains(&t.to_string()), "{t}");
        }
        let out = p.invoke_tool("click", json!({})).await.unwrap();
        assert_eq!(out["content"][0]["text"], "driver click");
        // No channel: a plain refusal the agent can read.
        let out = p.invoke_tool("list_spaces", json!({})).await.unwrap();
        assert_eq!(out["isError"], true);
        assert!(out["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("not turned children on"));
    }
}

use anyhow::Result;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use rmcp::{
    ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, Icon, Implementation, ServerCapabilities, ServerConfig},
    tool, tool_handler, tool_router,
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::{features::decision::DecideRequest, orchestration::Engine};

#[derive(Clone)]
pub struct Mcp {
    engine: Engine,
    tool_router: ToolRouter<Self>,
}

impl Mcp {
    pub fn new(engine: Engine) -> Self {
        Self {
            engine,
            tool_router: Self::tool_router(),
        }
    }
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Start {
    /// Registered workflow name returned by workflow_list.
    workflow: String,
    /// The exact fields listed for this workflow.
    #[serde(default)]
    input: Map<String, Value>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RunId {
    /// ID returned by workflow_start or listed under the schedule's latest run.
    id: String,
}

#[tool_router]
impl Mcp {
    #[tool(
        description = "Classify or score evidence against explicit criteria. Confirm that OpenRouter may receive the evidence before setting allow_provider."
    )]
    async fn decide(&self, Parameters(args): Parameters<DecideRequest>) -> CallToolResult {
        reply(self.engine.decide(args).await)
    }

    #[tool(
        description = "Find registered workflows when a user asks to run a recurring procedure or an installed script. Lists the decision backend, input fields, and latest scheduled run IDs.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn workflow_list(&self) -> CallToolResult {
        reply(self.engine.list().await)
    }

    #[tool(
        description = "Start a registered workflow with validated inputs and return its run ID."
    )]
    async fn workflow_start(&self, Parameters(args): Parameters<Start>) -> CallToolResult {
        reply(self.engine.start(&args.workflow, args.input).await)
    }

    #[tool(
        description = "Read a workflow's current status and completed step results by run ID.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn workflow_get(&self, Parameters(args): Parameters<RunId>) -> CallToolResult {
        reply(self.engine.get(&args.id).await)
    }

    #[tool(
        description = "Cancel a queued or running workflow. Completed effects may already have occurred."
    )]
    async fn workflow_cancel(&self, Parameters(args): Parameters<RunId>) -> CallToolResult {
        reply(self.engine.cancel(&args.id).await)
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for Mcp {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(
                Implementation::new("decision-tools", env!("CARGO_PKG_VERSION"))
                    .with_title("Decision Tools")
                    .with_website_url("https://github.com/Kobbokkom/decision-tools")
                    .with_icons(vec![Icon::new(format!(
                        "data:image/png;base64,{}",
                        STANDARD.encode(include_bytes!("../assets/icon.png"))
                    ))
                    .with_mime_type("image/png")
                    .with_sizes(vec!["128x128".to_owned()])]),
            )
            .with_instructions("Call workflow_list for registered procedures, or decide for a judgment against explicit criteria.")
    }
}

fn reply(result: Result<Value>) -> CallToolResult {
    match result {
        Ok(value) => CallToolResult::structured(value),
        Err(error) => CallToolResult::structured_error(json!({"error":error.to_string()})),
    }
}

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

use crate::{
    features::{
        browse::{self, BrowseRequest},
        decision::{DecideRequest, Decision},
        search::{self, SearchRequest},
    },
    orchestration::Engine,
};

#[derive(Clone)]
pub struct Mcp {
    /// Workflow modes carry an engine; local mode has none and serves decide, grep and browse.
    engine: Option<Engine>,
    decision: Decision,
    /// The Jev-model provider for search and browsing, and the model that writes field text.
    jev: Decision,
    text_model: String,
    tool_router: ToolRouter<Self>,
}

impl Mcp {
    pub fn new(engine: Engine, decision: Decision) -> Self {
        let mut tool_router = Self::tool_router();
        tool_router.remove_route("grep");
        tool_router.remove_route("browse");
        Self {
            engine: Some(engine),
            jev: decision.clone(),
            decision,
            text_model: String::new(),
            tool_router,
        }
    }

    /// Local mode removes every route that calls this.
    fn engine(&self) -> &Engine {
        self.engine
            .as_ref()
            .expect("workflow tools are not served in local mode")
    }

    /// Tools that need this machine: its files, its browser and its localhost.
    pub fn local(decision: Decision, jev_model: &str, text_model: String) -> Self {
        let mut tool_router = Self::tool_router();
        for name in [
            "workflow_list",
            "workflow_start",
            "workflow_get",
            "workflow_cancel",
        ] {
            tool_router.remove_route(name);
        }
        Self {
            engine: None,
            jev: decision.with_model(jev_model),
            decision,
            text_model,
            tool_router,
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
        reply(match &self.engine {
            Some(engine) => engine.decide(args).await,
            None => self.decision.decide(args).await,
        })
    }

    #[tool(
        description = "Find where a described behavior is implemented when files or symbols are unknown. Pass one concrete behavior question and the smallest absolute subtree. Returns ranked file locations with verbatim excerpts; they are leads, so read the source before relying on them. Source excerpts go to OpenRouter after allow_provider. Key files, credential stores, lockfiles, hidden paths and ignored files are never read.",
        annotations(read_only_hint = true, open_world_hint = true)
    )]
    async fn grep(&self, Parameters(args): Parameters<SearchRequest>) -> CallToolResult {
        reply(search::search(&self.jev, args).await)
    }

    #[tool(
        description = "Check or operate a web page with a private headless Chrome. Give a starting URL (localhost works), the goal, and the exact content or URL that shows it is met. The browser acts only on observed controls and stops at the step budget. The task, URLs, page text and controls go to OpenRouter after allow_provider, so use synthetic or approved data. Typing is off unless allow_typing is set, but clicks and dropdown choices always run and can submit or change things, so name only safe actions in the goal. A public page that leads to a loopback or private-network address is refused. Check the returned final URL and text yourself.",
        annotations(open_world_hint = true)
    )]
    async fn browse(&self, Parameters(args): Parameters<BrowseRequest>) -> CallToolResult {
        reply(browse::browse(&self.jev, &self.text_model, args).await)
    }

    #[tool(
        description = "Find registered workflows when a user asks to run a recurring procedure or an installed script. Lists the decision backend, input fields, and latest scheduled run IDs.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn workflow_list(&self) -> CallToolResult {
        reply(self.engine().list().await)
    }

    #[tool(
        description = "Start a registered workflow with validated inputs and return its run ID."
    )]
    async fn workflow_start(&self, Parameters(args): Parameters<Start>) -> CallToolResult {
        reply(self.engine().start(&args.workflow, args.input).await)
    }

    #[tool(
        description = "Read a workflow's current status and completed step results by run ID.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn workflow_get(&self, Parameters(args): Parameters<RunId>) -> CallToolResult {
        reply(self.engine().get(&args.id).await)
    }

    #[tool(
        description = "Cancel a queued or running workflow. Completed effects may already have occurred."
    )]
    async fn workflow_cancel(&self, Parameters(args): Parameters<RunId>) -> CallToolResult {
        reply(self.engine().cancel(&args.id).await)
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
            .with_instructions("Call decide for a judgment against explicit criteria, grep to find where a behavior is implemented, or browse to check a web page. When workflow tools are listed, call workflow_list for registered procedures.")
    }
}

fn reply(result: Result<Value>) -> CallToolResult {
    match result {
        Ok(value) => CallToolResult::structured(value),
        Err(error) => CallToolResult::structured_error(json!({"error":error.to_string()})),
    }
}

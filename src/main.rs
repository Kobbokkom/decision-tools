mod features;
mod mcp;
mod orchestration;

use std::{
    fs::{File, OpenOptions},
    net::SocketAddr,
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context, Result, ensure};
use axum::{
    Router,
    extract::{Request, State},
    http::{HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
};
use clap::{Parser, Subcommand};
use rmcp::{
    ServiceExt,
    transport::{
        stdio,
        streamable_http_server::{
            StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
        },
    },
};
use subtle::ConstantTimeEq;

use crate::{
    features::{decision::Decision, registry::Registry},
    mcp::Mcp,
    orchestration::Engine,
};

#[derive(Parser)]
#[command(version, about)]
struct Args {
    /// Workflow registry. Not used by the local mode.
    #[arg(long, env = "DECISION_TOOLS_CONFIG")]
    config: Option<PathBuf>,
    #[arg(
        long,
        env = "DECISION_TOOLS_DB",
        default_value = "decision-tools.sqlite3"
    )]
    db: PathBuf,
    /// Use deterministic test answers without external requests.
    #[arg(long, env = "DECISION_TOOLS_DEMO")]
    demo: bool,
    #[arg(
        long,
        env = "DECISION_TOOLS_MODEL",
        default_value = "liquid/d1.2-ultra"
    )]
    model: String,
    /// Model that rates source and drives the browser in the local mode.
    #[arg(
        long,
        env = "DECISION_TOOLS_JEV_MODEL",
        default_value = "typesafe/jev-1.14"
    )]
    jev_model: String,
    /// Model that writes field values when the browser may type.
    #[arg(
        long,
        env = "DECISION_TOOLS_TEXT_MODEL",
        default_value = "inception/mercury-2.5"
    )]
    text_model: String,
    #[command(subcommand)]
    mode: Mode,
}

#[derive(Subcommand)]
enum Mode {
    /// Run MCP on standard input/output with workflows and the database.
    Stdio,
    /// Run MCP on standard input/output with decide, grep and browse for this machine.
    Local,
    /// Serve authenticated Streamable HTTP MCP.
    Serve {
        #[arg(long, env = "DECISION_TOOLS_LISTEN", default_value = "127.0.0.1:8789")]
        listen: SocketAddr,
    },
    /// Validate the registry without opening the database.
    CheckConfig,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    if matches!(args.mode, Mode::Local) {
        let decision = if args.demo {
            Decision::Demo
        } else {
            Decision::openrouter(args.model, secret("OPENROUTER_API_KEY")?)?
        };
        Mcp::local(decision, &args.jev_model, args.text_model)
            .serve(stdio())
            .await?
            .waiting()
            .await?;
        return Ok(());
    }
    let config = args
        .config
        .as_deref()
        .context("--config is required for this mode")?;
    let backend = if args.demo {
        "demo".to_owned()
    } else {
        format!("openrouter:{}", args.model)
    };
    let registry = Registry::load(config, &backend)?;
    if matches!(args.mode, Mode::CheckConfig) {
        println!("Workflow registry is valid.");
        return Ok(());
    }

    let decision = if args.demo {
        Decision::Demo
    } else {
        Decision::openrouter(args.model, secret("OPENROUTER_API_KEY")?)?
    };
    let tool_decision = decision.clone();
    let http = match args.mode {
        Mode::Serve { listen } => {
            let key = secret("DECISION_TOOLS_TOKEN")?;
            ensure!(
                key.len() >= 32 && key.bytes().all(|byte| byte.is_ascii_graphic()),
                "MCP token must be at least 32 visible ASCII characters"
            );
            let auth: Arc<[u8]> = format!("Bearer {key}").into_bytes().into();
            let listener = tokio::net::TcpListener::bind(listen).await?;
            Some((listener, auth))
        }
        _ => None,
    };
    let _lease = lock_database(&args.db)?;
    let engine = Engine::open(registry, &args.db, decision).await?;
    let transport = async {
        if let Some((listener, auth)) = http {
            serve_http(
                engine.clone(),
                tool_decision.clone(),
                listener,
                auth,
                args.demo,
            )
            .await
        } else {
            Mcp::new(engine.clone(), tool_decision.clone())
                .serve(stdio())
                .await?
                .waiting()
                .await?;
            Ok(())
        }
    };
    tokio::select! {
        result = transport => result,
        result = engine.work() => result,
        result = engine.schedule() => result,
        result = shutdown() => result,
    }
}

async fn serve_http(
    engine: Engine,
    decision: Decision,
    listener: tokio::net::TcpListener,
    auth: Arc<[u8]>,
    demo: bool,
) -> Result<()> {
    let mcp = Mcp::new(engine.clone(), decision);
    let config = StreamableHttpServerConfig::default()
        .with_legacy_session_mode(false)
        .with_json_response(true)
        .enforce_origin_validation()
        .with_max_request_body_bytes(1024 * 1024);
    let service = StreamableHttpService::new(
        move || Ok(mcp.clone()),
        LocalSessionManager::default().into(),
        config,
    );
    let app = Router::new()
        .route_service("/mcp", service)
        .route_layer(middleware::from_fn_with_state(auth, authenticate))
        .route("/healthz", get(|| async { "ok" }))
        .route(
            "/readyz",
            get(move || {
                let engine = engine.clone();
                async move {
                    match engine.ready().await {
                        Ok(()) => (
                            StatusCode::OK,
                            if demo {
                                "ready (demo)"
                            } else {
                                "ready (openrouter configured)"
                            },
                        ),
                        Err(_) => (StatusCode::SERVICE_UNAVAILABLE, "database unavailable"),
                    }
                }
            }),
        );
    eprintln!("Decision Tools listening on {}", listener.local_addr()?);
    axum::serve(listener, app).await?;
    Ok(())
}

async fn authenticate(State(auth): State<Arc<[u8]>>, request: Request, next: Next) -> Response {
    let valid = request
        .headers()
        .get(header::AUTHORIZATION)
        .is_some_and(|value| bool::from(value.as_bytes().ct_eq(&auth)));
    if valid {
        return next.run(request).await;
    }
    let mut response = StatusCode::UNAUTHORIZED.into_response();
    response
        .headers_mut()
        .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
    response
}

fn secret(name: &str) -> Result<String> {
    let value = match std::env::var_os(format!("{name}_FILE")) {
        Some(path) => std::fs::read_to_string(path).with_context(|| format!("read {name}_FILE"))?,
        None => std::env::var(name).with_context(|| format!("set {name} or {name}_FILE"))?,
    };
    let value = value.trim();
    ensure!(!value.is_empty(), "{name} is empty");
    Ok(value.to_owned())
}

fn lock_database(path: &Path) -> Result<File> {
    OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(path)
        .context("open state path (create its parent directory first)")?;
    let mut lock_path = path.canonicalize()?.into_os_string();
    lock_path.push(".lockfile");
    let lease = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .mode(0o600)
        .open(lock_path)
        .context("open state lock")?;
    lease
        .try_lock()
        .context("state database is locked by another service")?;
    Ok(lease)
}

async fn shutdown() -> Result<()> {
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    tokio::select! {
        result = tokio::signal::ctrl_c() => result?,
        _ = term.recv() => {}
    }
    Ok(())
}

pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 80
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

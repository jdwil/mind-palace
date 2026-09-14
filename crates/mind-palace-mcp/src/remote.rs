//! Mind Palace MCP server — remote (Streamable HTTP) transport.
//!
//! Serves the same tools as the stdio binary over HTTP so agent harnesses can
//! connect remotely (deploy to EC2/ECS/Lambda-behind-ALB). Reuses the shared
//! setup and the `MindPalaceMcpServer` tool implementation.
//!
//! Environment variables (in addition to the MIND_PALACE_* config the stdio
//! binary uses):
//!   MP_BIND_ADDR   - listen address (default "0.0.0.0:8080")
//!   MP_MCP_PATH    - HTTP path to serve MCP on (default "/mcp")
//!   MP_AUTH_TOKEN  - OPTIONAL. If set, requests must send
//!                    `Authorization: Bearer <token>`. If unset, auth is
//!                    DISABLED (intended for deployment behind a VPN/private
//!                    network). A warning is logged when auth is off.
//!   MP_ALLOWED_HOSTS - OPTIONAL comma-separated Host allowlist. If unset,
//!                    host validation is disabled (needed when reached via an
//!                    ALB/hostname). Set it to lock down the Host header.

use std::sync::Arc;

use axum::{
    Router,
    body::Body,
    extract::State,
    http::{Request, StatusCode, header},
    middleware::{self, Next},
    response::Response,
    routing::get,
};
use mind_palace_mcp::MindPalaceMcpServer;
use mind_palace_mcp::setup;
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

#[derive(Clone)]
struct AuthState {
    token: Option<Arc<String>>,
}

/// Optional bearer-token auth. When no token is configured, every request
/// passes (deployment is expected to be behind a VPN/private network).
async fn auth_middleware(
    State(auth): State<AuthState>,
    req: Request<Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    let Some(expected) = &auth.token else {
        return Ok(next.run(req).await);
    };
    let provided = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    match provided {
        Some(t) if t == expected.as_str() => Ok(next.run(req).await),
        _ => Err(StatusCode::UNAUTHORIZED),
    }
}

async fn health() -> &'static str {
    "ok"
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "mind_palace_mcp=info,info".into()),
        )
        .json()
        .init();

    setup::fix_sso_cache_timestamps();

    let bind_addr = env_or("MP_BIND_ADDR", "0.0.0.0:8080");
    let mcp_path = env_or("MP_MCP_PATH", "/mcp");

    // Build the shared WikiService once; the MCP service factory clones the Arc
    // per session so all sessions share the in-memory graph and AWS clients.
    let service = setup::build_service_from_env().await?;
    let ctx = setup::build_context_from_env();

    // Optional auth.
    let auth_token = std::env::var("MP_AUTH_TOKEN")
        .ok()
        .filter(|s| !s.is_empty());
    if auth_token.is_none() {
        tracing::warn!(
            "MP_AUTH_TOKEN not set — authentication is DISABLED. Only run this behind a VPN or private network."
        );
    } else {
        tracing::info!("Bearer-token authentication enabled.");
    }
    let auth_state = AuthState {
        token: auth_token.map(Arc::new),
    };

    // Host validation: relax by default (remote access via ALB/hostname needs
    // arbitrary Host). Lock down via MP_ALLOWED_HOSTS if desired.
    let mut config = StreamableHttpServerConfig::default();
    match std::env::var("MP_ALLOWED_HOSTS") {
        Ok(hosts) if !hosts.trim().is_empty() => {
            let list: Vec<String> = hosts.split(',').map(|s| s.trim().to_string()).collect();
            config = config.with_allowed_hosts(list);
        }
        _ => {
            config = config.disable_allowed_hosts();
        }
    }

    let mcp_service = StreamableHttpService::new(
        move || Ok(MindPalaceMcpServer::new(service.clone(), ctx.clone())),
        Arc::new(LocalSessionManager::default()),
        config,
    );

    let mcp_router = Router::new()
        .nest_service(&mcp_path, mcp_service)
        .layer(middleware::from_fn_with_state(auth_state, auth_middleware));

    let app = Router::new()
        .route("/health", get(health))
        .merge(mcp_router);

    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
    tracing::info!(addr = %bind_addr, path = %mcp_path, "Mind Palace remote MCP server listening");
    axum::serve(listener, app).await?;
    Ok(())
}

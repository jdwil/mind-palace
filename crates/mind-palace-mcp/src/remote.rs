//! Mind Palace MCP server — remote (Streamable HTTP) transport.
//!
//! Serves the same tools as the stdio binary over HTTP so agent harnesses can
//! connect remotely (deploy to EC2/ECS/Lambda-behind-ALB). Reuses the shared
//! setup and the `MindPalaceMcpServer` tool implementation.
//!
//! ## Authentication (Spec 1)
//!
//! `MP_AUTH_MODE` selects one of three generic, provider-agnostic modes:
//! - `none` (default): every request is Anonymous — it may see ONLY fully-open
//!   (public) pages. Safe to expose public knowledge on a VPN/private network.
//! - `token`: a single shared bearer token; a match maps to one configured
//!   shared user identity.
//! - `oidc`: per-request identity from a validated JWT. On a missing/invalid
//!   token, responds 401 with `WWW-Authenticate` and serves RFC 9728
//!   protected-resource metadata so compliant MCP clients (Kiro, Claude
//!   Desktop, …) run the OAuth browser flow.
//!
//! The validated [`Identity`] is inserted into the axum request extensions;
//! rmcp threads the request `Parts` into the MCP request, and each tool handler
//! reads the identity per request (see `MindPalaceMcpServer::ctx_for`).
//!
//! Other environment variables:
//!   MP_BIND_ADDR   - listen address (default "0.0.0.0:8080")
//!   MP_MCP_PATH    - HTTP path to serve MCP on (default "/mcp")
//!   MP_PUBLIC_URL  - this server's public URL, advertised as the "resource" in
//!                    the RFC 9728 metadata (default derived from bind addr).
//!   MP_ALLOWED_HOSTS - OPTIONAL comma-separated Host allowlist. If unset,
//!                    host validation is disabled (needed when reached via an
//!                    ALB/hostname). Set it to lock down the Host header.
//!
//! See §5 of the spec for the OIDC config vars (`MP_OIDC_*`). No provider
//! (Cognito/Google/etc.) is named in code — all specifics live in config.

use std::sync::Arc;

use axum::{
    Json, Router,
    body::Body,
    extract::State,
    http::{Request, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
};
use mind_palace_mcp::auth::{AuthMode, Authenticator, ProtectedResourceMetadata};
use mind_palace_mcp::setup;
use mind_palace_mcp::{MindPalaceMcpServer, auth::AuthConfig};
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

/// Well-known path for OAuth Protected Resource Metadata (RFC 9728).
const PROTECTED_RESOURCE_PATH: &str = "/.well-known/oauth-protected-resource";

#[derive(Clone)]
struct AuthLayerState {
    authenticator: Authenticator,
    /// This server's public root URL, used to build the `WWW-Authenticate`
    /// `resource_metadata` URL (`{public_url}/.well-known/oauth-protected-resource`).
    public_url: Arc<String>,
    /// The canonical resource identifier the client actually calls
    /// (`{public_url}{mcp_path}`, e.g. `https://host/mcp`). Per RFC 9728 this is
    /// the `resource` value in the protected-resource metadata; it MUST match the
    /// endpoint the client targets, or spec-compliant clients abort on mismatch.
    resource_url: Arc<String>,
}

/// Extract the bearer token from the Authorization header, if present.
fn bearer_from(req: &Request<Body>) -> Option<String> {
    req.headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::to_string)
}

/// Build the `WWW-Authenticate` challenge header for a 401 in OIDC mode.
///
/// Points clients at the protected-resource metadata document per the MCP
/// authorization spec / RFC 9728.
fn www_authenticate_value(public_url: &str) -> String {
    format!(
        "Bearer resource_metadata=\"{}{}\"",
        public_url.trim_end_matches('/'),
        PROTECTED_RESOURCE_PATH
    )
}

/// Auth middleware: resolves the request [`Identity`] per the configured mode
/// and inserts it into the request extensions for the tool handlers to read.
///
/// On authentication failure in `oidc`/`token` mode, returns 401. In `oidc`
/// mode the 401 carries a `WWW-Authenticate` header so compliant MCP clients
/// discover the metadata and start the browser sign-in flow.
async fn auth_middleware(
    State(state): State<AuthLayerState>,
    mut req: Request<Body>,
    next: Next,
) -> Response {
    let bearer = bearer_from(&req);
    match state.authenticator.authenticate(bearer.as_deref()).await {
        Ok(identity) => {
            req.extensions_mut().insert(identity);
            next.run(req).await
        }
        Err(e) => {
            tracing::debug!(error = %e, "request unauthenticated");
            let mut resp = StatusCode::UNAUTHORIZED.into_response();
            if state.authenticator.mode() == AuthMode::Oidc
                && let Ok(val) =
                    header::HeaderValue::from_str(&www_authenticate_value(&state.public_url))
            {
                resp.headers_mut().insert(header::WWW_AUTHENTICATE, val);
            }
            resp
        }
    }
}

/// RFC 9728 protected-resource metadata endpoint (OIDC mode).
async fn protected_resource_metadata(State(state): State<AuthLayerState>) -> Response {
    match state.authenticator.auth_server_url() {
        Some(auth_server) => Json(ProtectedResourceMetadata::new(
            state.resource_url.as_str(),
            auth_server,
        ))
        .into_response(),
        // Not in OIDC mode — no authorization server to advertise.
        None => StatusCode::NOT_FOUND.into_response(),
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
    let public_url = Arc::new(env_or("MP_PUBLIC_URL", &format!("http://{bind_addr}")));
    // The canonical resource identifier clients call = public root + MCP path.
    // Advertised as RFC 9728 `resource`; must match what the client targets.
    let resource_url = Arc::new(format!("{}{}", public_url.trim_end_matches('/'), mcp_path));

    // Build the shared WikiService once; the MCP service factory clones the Arc
    // per session so all sessions share the in-memory graph and AWS clients.
    let service = setup::build_service_from_env().await?;
    // Base context used only as a fallback when a request carries no per-request
    // identity (never the case on the HTTP path — the middleware always inserts
    // an Identity). Kept for symmetry with the stdio binary.
    let ctx = setup::build_context_from_env();

    // Resolve auth configuration and construct the authenticator.
    let auth_config = AuthConfig::from_env().map_err(|e| {
        tracing::error!(error = %e, "invalid auth configuration");
        e
    })?;
    match auth_config.mode {
        AuthMode::None => tracing::warn!(
            "MP_AUTH_MODE=none — requests are ANONYMOUS and may see only fully-open (public) pages. Run behind a VPN/private network."
        ),
        AuthMode::Token => {
            tracing::info!("MP_AUTH_MODE=token — shared bearer-token authentication enabled.")
        }
        AuthMode::Oidc => tracing::info!(
            "MP_AUTH_MODE=oidc — per-request JWT identity enabled; 401 discovery served at {PROTECTED_RESOURCE_PATH}."
        ),
    }
    let authenticator = Authenticator::new(auth_config);
    let auth_state = AuthLayerState {
        authenticator,
        public_url: public_url.clone(),
        resource_url: resource_url.clone(),
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

    let mcp_router =
        Router::new()
            .nest_service(&mcp_path, mcp_service)
            .layer(middleware::from_fn_with_state(
                auth_state.clone(),
                auth_middleware,
            ));

    let app = Router::new()
        .route("/health", get(health))
        .route(PROTECTED_RESOURCE_PATH, get(protected_resource_metadata))
        .merge(mcp_router)
        .with_state(auth_state);

    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
    tracing::info!(addr = %bind_addr, path = %mcp_path, "Mind Palace remote MCP server listening");
    axum::serve(listener, app).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn www_authenticate_points_at_root_wellknown_not_mcp_path() {
        // The resource_metadata URL in the challenge must be the ROOT
        // /.well-known/oauth-protected-resource — it must NOT include /mcp.
        let v = www_authenticate_value("https://mp.dev.example.com");
        assert_eq!(
            v,
            "Bearer resource_metadata=\"https://mp.dev.example.com/.well-known/oauth-protected-resource\""
        );
        assert!(v.ends_with("/.well-known/oauth-protected-resource\""));
        assert!(!v.contains("/mcp"));
    }

    #[test]
    fn www_authenticate_trims_trailing_slash_on_public_url() {
        let v = www_authenticate_value("https://mp.dev.example.com/");
        assert_eq!(
            v,
            "Bearer resource_metadata=\"https://mp.dev.example.com/.well-known/oauth-protected-resource\""
        );
    }

    #[test]
    fn resource_url_is_public_url_plus_mcp_path() {
        // Mirrors the construction in main(): the RFC 9728 `resource` value.
        let public_url = "https://mp.dev.example.com";
        let mcp_path = "/mcp";
        let resource_url = format!("{}{}", public_url.trim_end_matches('/'), mcp_path);
        assert_eq!(resource_url, "https://mp.dev.example.com/mcp");

        // Trailing slash on public_url must not produce a double slash.
        let resource_url2 = format!(
            "{}{}",
            "https://mp.dev.example.com/".trim_end_matches('/'),
            mcp_path
        );
        assert_eq!(resource_url2, "https://mp.dev.example.com/mcp");
    }
}

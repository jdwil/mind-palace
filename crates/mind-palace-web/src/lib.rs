//! Mind Palace web API server (Axum).
//!
//! Backend for the `mind-palace-ui` Svelte component library. Serves page CRUD,
//! search, graph, and the Spec 5 access-control management endpoints (page
//! access/grants, groups + managers, hierarchy, secret references, audit).
//!
//! ## Authentication (Spec 5 §4/§6)
//!
//! ONE identity path, shared with the remote MCP server (Spec 1). Auth is
//! configured via the SAME `MP_AUTH_MODE` / `MP_OIDC_*` environment variables
//! and validated by the SAME [`mind_palace_mcp::auth::Authenticator`], so the
//! email a human's JWT resolves to is identical to what an agent's token
//! resolves to — grants and group membership apply the same either way. The old
//! bespoke Google login has been removed.

pub mod app;
pub mod auth;
pub mod config;
pub mod routes;

pub use config::WebConfig;

use std::sync::Arc;

use mind_palace_core::domain::service::WikiService;
use mind_palace_mcp::auth::{AuthConfig, AuthMode, Authenticator};

/// Start the web API server.
///
/// Builds the shared [`Authenticator`] from the environment (`MP_AUTH_MODE` /
/// `MP_OIDC_*`), the same configuration the remote MCP server uses. `public_url`
/// is advertised as the `resource` in RFC 9728 discovery metadata; pass the
/// server's externally-reachable URL (falls back to the bind address).
pub async fn start_server(
    config: WebConfig,
    service: Arc<WikiService>,
) -> Result<(), Box<dyn std::error::Error>> {
    let port = config.port;
    let auth_config = AuthConfig::from_env().map_err(|e| {
        tracing::error!(error = %e, "invalid auth configuration");
        e
    })?;
    match auth_config.mode {
        AuthMode::None => tracing::warn!(
            "MP_AUTH_MODE=none — web API requests are ANONYMOUS and may see only fully-open (public) pages. Run behind a VPN/private network."
        ),
        AuthMode::Token => {
            tracing::info!("MP_AUTH_MODE=token — shared bearer-token authentication enabled.")
        }
        AuthMode::Oidc => tracing::info!(
            "MP_AUTH_MODE=oidc — per-request JWT identity enabled (same provider as the MCP server)."
        ),
    }
    let authenticator = Authenticator::new(auth_config);
    let public_url =
        std::env::var("MP_PUBLIC_URL").unwrap_or_else(|_| format!("http://0.0.0.0:{port}"));

    let app = app::build_app_with_authenticator(config, service, authenticator, public_url);
    let listener = tokio::net::TcpListener::bind(("0.0.0.0", port)).await?;
    tracing::info!("mind-palace-web listening on port {port}");
    axum::serve(listener, app).await?;
    Ok(())
}

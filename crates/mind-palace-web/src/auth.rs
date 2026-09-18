//! Web-API authentication (Spec 5 §4/§6).
//!
//! ONE identity path. The web UI authenticates against the SAME OIDC
//! provider/config as the remote MCP server (Spec 1), reusing
//! [`mind_palace_mcp::auth::Authenticator`] verbatim so the identity (email)
//! the web API derives is byte-for-byte the identity the MCP path derives.
//! Grants and group membership therefore apply identically whether an access
//! change is made by a human in the UI or by an agent via MCP.
//!
//! The old bespoke Google `/auth/login` + `/auth/callback` session flow is
//! removed. Provider specifics (issuer, JWKS, audiences, claims) live only in
//! the shared `MP_OIDC_*` config — never in this crate.
//!
//! Mechanism (mirrors `mind-palace-mcp-remote`): a middleware validates the
//! `Authorization: Bearer <jwt>` header, resolves an [`Identity`], and inserts
//! it into the request extensions. Handlers pull the identity out and build a
//! per-request [`TenantContext`] via [`ctx_from_parts`].

use std::sync::Arc;

use axum::{
    body::Body,
    extract::State,
    http::{Request, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};

use mind_palace_core::domain::tenant::{Identity, TenantContext};
use mind_palace_mcp::auth::{AuthMode, Authenticator};

/// Well-known path for OAuth Protected Resource Metadata (RFC 9728), served in
/// OIDC mode so browsers/clients can discover the authorization server.
pub const PROTECTED_RESOURCE_PATH: &str = "/.well-known/oauth-protected-resource";

/// Shared state for the auth middleware.
#[derive(Clone)]
pub struct AuthState {
    pub authenticator: Authenticator,
    /// This server's public URL, advertised as the `resource` in RFC 9728
    /// metadata and in the `WWW-Authenticate` challenge.
    pub public_url: Arc<String>,
}

/// Extract the bearer token from the `Authorization` header, if present.
fn bearer_from(req: &Request<Body>) -> Option<String> {
    req.headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::to_string)
}

/// Build the `WWW-Authenticate` challenge header value for a 401 in OIDC mode.
fn www_authenticate_value(public_url: &str) -> String {
    format!(
        "Bearer resource_metadata=\"{}{}\"",
        public_url.trim_end_matches('/'),
        PROTECTED_RESOURCE_PATH
    )
}

/// Auth middleware: resolve the request [`Identity`] per the configured mode and
/// insert it into the request extensions for handlers to read.
///
/// On failure in `oidc`/`token` mode → 401. In `oidc` mode the 401 carries a
/// `WWW-Authenticate` header pointing at the protected-resource metadata.
pub async fn auth_middleware(
    State(state): State<AuthState>,
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
            tracing::debug!(error = %e, "web request unauthenticated");
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

/// Build a per-request [`TenantContext`] from the [`Identity`] the middleware
/// inserted into the request extensions.
///
/// This is the SINGLE place the web API turns an authenticated principal into a
/// domain context. The service layer then resolves group membership per request
/// (`resolve_ctx`) so authorization matches the MCP path exactly (Spec 5 §5).
///
/// If no identity is present (should not happen when the middleware is wired),
/// we fall back to [`Identity::Anonymous`] — the safe default that can see only
/// fully-open pages.
pub fn ctx_from_extensions(ext: &axum::http::Extensions) -> TenantContext {
    let identity = ext
        .get::<Identity>()
        .cloned()
        .unwrap_or(Identity::Anonymous);
    TenantContext::from_identity(identity)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ctx_from_extensions_maps_injected_user_identity() {
        // The middleware inserts an Identity; the handler must derive the SAME
        // per-request context (email-keyed) so authorization matches the MCP
        // path (Spec 5 §5/§6).
        let mut ext = axum::http::Extensions::new();
        ext.insert(Identity::User {
            id: "alice@example.com".into(),
        });
        let ctx = ctx_from_extensions(&ext);
        assert_eq!(ctx.user_id(), Some("alice@example.com"));
    }

    #[test]
    fn ctx_from_extensions_defaults_to_anonymous_when_absent() {
        // Defensive: absent identity → Anonymous (public-only), never a
        // privileged fallback.
        let ext = axum::http::Extensions::new();
        let ctx = ctx_from_extensions(&ext);
        assert!(ctx.identity.is_anonymous());
        assert_eq!(ctx.user_id(), None);
    }
}

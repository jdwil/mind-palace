use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    middleware,
    response::{IntoResponse, Response},
    routing::{delete, get, post, put},
};
use std::sync::Arc;
use tower_http::cors::{Any, CorsLayer};

use mind_palace_core::domain::service::WikiService;
use mind_palace_mcp::auth::{Authenticator, ProtectedResourceMetadata};

use crate::auth::{self, AuthState, PROTECTED_RESOURCE_PATH};
use crate::config::WebConfig;
use crate::routes;

/// Application state shared by all handlers.
///
/// Note there is NO server-wide `TenantContext` here anymore (Spec 5 §4): the
/// identity is per-request, injected by the auth middleware and turned into a
/// `TenantContext` inside each handler. The service resolves group membership
/// per request so authorization matches the MCP path exactly.
pub struct AppState {
    pub config: WebConfig,
    pub service: Arc<WikiService>,
}

pub fn build_app(config: WebConfig, service: Arc<WikiService>, auth_state: AuthState) -> Router {
    let state = Arc::new(AppState { config, service });

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    // All /api routes require an authenticated identity (the middleware inserts
    // an `Identity` — Anonymous in `none` mode — into request extensions).
    let api = Router::new()
        // Page CRUD
        .route("/api/pages", get(routes::list_pages))
        .route("/api/pages", post(routes::create_page))
        .route("/api/pages/{slug}", get(routes::get_page))
        .route("/api/pages/{slug}", put(routes::update_page))
        .route("/api/pages/{slug}", delete(routes::delete_page))
        // Graph + search
        .route("/api/graph", get(routes::get_graph))
        .route("/api/search", get(routes::search))
        // Page access + grants (Spec 2)
        .route("/api/pages/{slug}/access", get(routes::get_access))
        .route("/api/pages/{slug}/access", put(routes::set_access))
        .route("/api/pages/{slug}/grants", post(routes::add_grant))
        .route("/api/pages/{slug}/grants", delete(routes::remove_grant))
        // Groups (Spec 2 + Spec 5 managers)
        .route("/api/groups", get(routes::list_groups))
        .route("/api/groups", post(routes::create_group))
        .route("/api/groups/{id}", get(routes::get_group))
        .route("/api/groups/{id}/members", post(routes::add_group_member))
        .route(
            "/api/groups/{id}/members",
            delete(routes::remove_group_member),
        )
        .route("/api/groups/{id}/managers", post(routes::add_group_manager))
        .route(
            "/api/groups/{id}/managers",
            delete(routes::remove_group_manager),
        )
        // Hierarchy (Spec 3)
        .route("/api/pages/{slug}/parent", put(routes::set_parent))
        .route("/api/pages/{slug}/tree", get(routes::get_tree))
        // Secret references (Spec 4) — names + refs only, never values
        .route(
            "/api/pages/{slug}/secret-refs",
            get(routes::list_secret_refs),
        )
        .route(
            "/api/pages/{slug}/secret-refs",
            post(routes::add_secret_ref),
        )
        .route(
            "/api/pages/{slug}/secret-refs",
            delete(routes::remove_secret_ref),
        )
        // Audit (read-only)
        .route("/api/audit", get(routes::get_audit))
        .layer(middleware::from_fn_with_state(
            auth_state.clone(),
            auth::auth_middleware,
        ))
        .with_state(state);

    // Public (unauthenticated) endpoints: health + RFC 9728 discovery metadata.
    let public = Router::new()
        .route("/health", get(health))
        .route(PROTECTED_RESOURCE_PATH, get(protected_resource_metadata))
        .with_state(auth_state);

    Router::new().merge(api).merge(public).layer(cors)
}

async fn health() -> &'static str {
    "ok"
}

/// RFC 9728 protected-resource metadata (OIDC mode). Advertises the shared
/// authorization server so browser clients can run the sign-in flow. 404 when
/// not in OIDC mode.
async fn protected_resource_metadata(State(state): State<AuthState>) -> Response {
    match state.authenticator.auth_server_url() {
        Some(auth_server) => Json(ProtectedResourceMetadata::new(
            state.public_url.as_str(),
            auth_server,
        ))
        .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// Convenience for callers that already have an [`Authenticator`] and public URL.
pub fn build_app_with_authenticator(
    config: WebConfig,
    service: Arc<WikiService>,
    authenticator: Authenticator,
    public_url: String,
) -> Router {
    let auth_state = AuthState {
        authenticator,
        public_url: Arc::new(public_url),
    };
    build_app(config, service, auth_state)
}

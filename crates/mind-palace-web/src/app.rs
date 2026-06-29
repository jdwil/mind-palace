use axum::{
    Router,
    routing::{delete, get, post, put},
};
use std::sync::Arc;
use tower_http::cors::{Any, CorsLayer};

use mind_palace_core::domain::{service::WikiService, tenant::TenantContext};

use crate::auth;
use crate::config::WebConfig;
use crate::routes;

pub struct AppState {
    pub config: WebConfig,
    pub service: Arc<WikiService>,
    pub ctx: TenantContext,
}

pub fn build_app(config: WebConfig, service: Arc<WikiService>, ctx: TenantContext) -> Router {
    let state = Arc::new(AppState {
        config,
        service,
        ctx,
    });

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let api = Router::new()
        .route("/api/pages", get(routes::list_pages))
        .route("/api/pages/{slug}", get(routes::get_page))
        .route("/api/pages/{slug}", put(routes::update_page))
        .route("/api/pages", post(routes::create_page))
        .route("/api/pages/{slug}", delete(routes::delete_page))
        .route("/api/graph", get(routes::get_graph))
        .route("/api/search", get(routes::search));

    let auth_routes = Router::new()
        .route("/auth/login", get(auth::login))
        .route("/auth/callback", get(auth::callback));

    Router::new()
        .merge(api)
        .merge(auth_routes)
        .layer(cors)
        .with_state(state)
}

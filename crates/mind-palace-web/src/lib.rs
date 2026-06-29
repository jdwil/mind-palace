pub mod app;
pub mod auth;
pub mod config;
pub mod routes;

pub use config::WebConfig;

use mind_palace_core::domain::{service::WikiService, tenant::TenantContext};
use std::sync::Arc;

pub async fn start_server(
    config: WebConfig,
    service: Arc<WikiService>,
    ctx: TenantContext,
) -> Result<(), Box<dyn std::error::Error>> {
    let port = config.port;
    let app = app::build_app(config, service, ctx);
    let listener = tokio::net::TcpListener::bind(("0.0.0.0", port)).await?;
    tracing::info!("mind-palace-web listening on port {port}");
    axum::serve(listener, app).await?;
    Ok(())
}

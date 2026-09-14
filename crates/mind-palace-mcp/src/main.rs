//! Mind Palace MCP server — stdio transport (local agents).

use mind_palace_mcp::MindPalaceMcpServer;
use mind_palace_mcp::setup;
use rmcp::ServiceExt;
use rmcp::transport::stdio;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Normalize any SSO cache tokens with timezone offsets the Rust SDK can't parse.
    setup::fix_sso_cache_timestamps();

    let service = setup::build_service_from_env().await?;
    let ctx = setup::build_context_from_env();

    if let Ok(user_name) = std::env::var("MIND_PALACE_USER_NAME") {
        eprintln!("Mind Palace MCP: user={user_name}");
    }

    let server = MindPalaceMcpServer::new(service, ctx);

    let running = server.serve(stdio()).await.inspect_err(|e| {
        eprintln!("MCP server error: {e:?}");
    })?;

    running.waiting().await?;
    Ok(())
}

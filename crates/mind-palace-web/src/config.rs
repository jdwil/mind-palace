use serde::Deserialize;

/// Web server configuration.
///
/// Spec 5 §4/§6: the web app authenticates via the SAME OIDC provider/config as
/// the remote MCP server (Spec 1). Identity/auth configuration therefore lives
/// entirely in the shared [`mind_palace_mcp::auth::AuthConfig`] (parsed from
/// `MP_AUTH_MODE` / `MP_OIDC_*` env vars) — NOT here. This struct holds only the
/// transport concern (which port to bind). No identity-provider specifics
/// (Cognito/Google/etc.) appear here or anywhere in this crate.
#[derive(Debug, Clone, Deserialize)]
pub struct WebConfig {
    pub port: u16,
}

impl WebConfig {
    pub fn new(port: u16) -> Self {
        Self { port }
    }
}

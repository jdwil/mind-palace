use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct WebConfig {
    pub port: u16,
    pub google_client_id: String,
    pub google_client_secret: String,
    pub allowed_emails: Vec<String>,
    pub session_secret: String,
    pub redirect_uri: String,
}

//! Generic authentication & identity for the remote MCP server.
//!
//! This module is provider-agnostic: it speaks "OIDC/JWT bearer auth" and
//! never mentions any specific identity provider. All provider specifics
//! (issuer URL, JWKS endpoint, audiences, claim names, authorization-server
//! URL) arrive through configuration (env vars parsed in [`AuthConfig::from_env`]).
//!
//! Three modes are supported (see [`AuthMode`]):
//! - `None`  — no auth; every request maps to [`Identity::Anonymous`].
//! - `Token` — a single shared bearer token; a match maps to a configured
//!   shared [`Identity::User`].
//! - `Oidc`  — per-request identity from a validated JWT (email claim).

use std::sync::Arc;
use std::time::{Duration, Instant};

use jsonwebtoken::jwk::JwkSet;
use jsonwebtoken::{DecodingKey, Validation, decode, decode_header};
use mind_palace_core::domain::tenant::Identity;
use serde::Serialize;
use tokio::sync::RwLock;

/// Which authentication mode the remote server runs in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthMode {
    /// No authentication — requests are anonymous (public-only).
    None,
    /// A single shared bearer token.
    Token,
    /// OIDC/JWT bearer auth with per-request identity.
    Oidc,
}

impl AuthMode {
    fn parse(s: &str) -> Result<Self, ConfigError> {
        match s.trim().to_ascii_lowercase().as_str() {
            "" | "none" => Ok(AuthMode::None),
            "token" => Ok(AuthMode::Token),
            "oidc" => Ok(AuthMode::Oidc),
            other => Err(ConfigError::InvalidMode(other.to_string())),
        }
    }
}

/// OIDC-specific configuration (only populated in `oidc` mode).
#[derive(Debug, Clone)]
pub struct OidcConfig {
    /// Expected `iss` claim value.
    pub issuer: String,
    /// JWKS endpoint URL (derived from issuer when not explicitly configured).
    pub jwks_url: String,
    /// Allowed `aud` / `client_id` values. Empty = do not validate audience.
    pub audiences: Vec<String>,
    /// Claim to use as the identity string (default `email`).
    pub email_claim: String,
    /// Fallback claim if the primary claim is absent (default `sub`).
    pub fallback_claim: String,
    /// Authorization-server base URL, advertised in the 401 discovery metadata.
    pub auth_server_url: String,
}

/// Fully-resolved auth configuration.
#[derive(Debug, Clone)]
pub struct AuthConfig {
    pub mode: AuthMode,
    /// Shared token (mode = `Token`).
    pub token: Option<String>,
    /// Identity string a valid shared-token request maps to (mode = `Token`).
    pub token_identity: String,
    /// OIDC config (mode = `Oidc`).
    pub oidc: Option<OidcConfig>,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("invalid MP_AUTH_MODE '{0}' (expected none|token|oidc)")]
    InvalidMode(String),
    #[error("MP_AUTH_MODE=token requires MP_AUTH_TOKEN to be set")]
    MissingToken,
    #[error("MP_AUTH_MODE=oidc requires {0}")]
    MissingOidc(&'static str),
}

fn env_opt(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|s| !s.trim().is_empty())
}

/// Derive a standard OIDC JWKS URL from an issuer when one is not configured.
///
/// Uses the OIDC convention `<issuer>/.well-known/jwks.json`. This is generic;
/// providers that publish JWKS elsewhere should set `MP_OIDC_JWKS_URL`.
fn default_jwks_url(issuer: &str) -> String {
    format!("{}/.well-known/jwks.json", issuer.trim_end_matches('/'))
}

impl AuthConfig {
    /// Parse auth configuration from environment variables.
    pub fn from_env() -> Result<Self, ConfigError> {
        let mode = AuthMode::parse(&env_opt("MP_AUTH_MODE").unwrap_or_default())?;
        let token = env_opt("MP_AUTH_TOKEN");

        let oidc = if mode == AuthMode::Oidc {
            let issuer =
                env_opt("MP_OIDC_ISSUER").ok_or(ConfigError::MissingOidc("MP_OIDC_ISSUER"))?;
            let jwks_url = env_opt("MP_OIDC_JWKS_URL").unwrap_or_else(|| default_jwks_url(&issuer));
            let audiences = env_opt("MP_OIDC_AUDIENCES")
                .map(|s| {
                    s.split(',')
                        .map(|a| a.trim().to_string())
                        .filter(|a| !a.is_empty())
                        .collect()
                })
                .unwrap_or_default();
            let email_claim = env_opt("MP_OIDC_EMAIL_CLAIM").unwrap_or_else(|| "email".to_string());
            let fallback_claim =
                env_opt("MP_OIDC_FALLBACK_CLAIM").unwrap_or_else(|| "sub".to_string());
            // Default the advertised authorization server to the issuer.
            let auth_server_url =
                env_opt("MP_OIDC_AUTH_SERVER_URL").unwrap_or_else(|| issuer.clone());
            Some(OidcConfig {
                issuer,
                jwks_url,
                audiences,
                email_claim,
                fallback_claim,
                auth_server_url,
            })
        } else {
            None
        };

        if mode == AuthMode::Token && token.is_none() {
            return Err(ConfigError::MissingToken);
        }

        Ok(Self {
            mode,
            token,
            token_identity: env_opt("MP_AUTH_TOKEN_IDENTITY")
                .unwrap_or_else(|| "shared-token".to_string()),
            oidc,
        })
    }
}

/// Errors that map to a 401 (unauthenticated) response.
#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("missing bearer token")]
    MissingToken,
    #[error("invalid bearer token")]
    InvalidToken,
    #[error("token header has no key id (kid)")]
    NoKid,
    #[error("no matching signing key for kid")]
    UnknownKid,
    #[error("jwt validation failed: {0}")]
    Validation(String),
    #[error("token has no usable identity claim")]
    NoIdentity,
    #[error("jwks fetch failed: {0}")]
    Jwks(String),
}

/// Derive a stable identity string from decoded JWT claims, preferring the
/// configured email claim and falling back to a secondary claim.
///
/// Pure and provider-agnostic — the unit of behavior tested for §3.3.
pub fn identity_from_claims(
    claims: &serde_json::Value,
    email_claim: &str,
    fallback_claim: &str,
) -> Option<String> {
    let pick = |name: &str| {
        claims
            .get(name)
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .filter(|s| !s.is_empty())
    };
    pick(email_claim).or_else(|| pick(fallback_claim))
}

struct CachedJwks {
    keys: JwkSet,
    fetched_at: Instant,
}

/// Validates OIDC/JWT bearer tokens against a cached JWKS and extracts identity.
pub struct OidcValidator {
    config: OidcConfig,
    http: reqwest::Client,
    cache: RwLock<Option<CachedJwks>>,
    /// Minimum interval between forced refreshes (on unknown kid).
    min_refresh_interval: Duration,
    /// Periodic refresh TTL.
    ttl: Duration,
}

impl OidcValidator {
    pub fn new(config: OidcConfig) -> Self {
        Self {
            config,
            http: reqwest::Client::new(),
            cache: RwLock::new(None),
            min_refresh_interval: Duration::from_secs(60),
            ttl: Duration::from_secs(3600),
        }
    }

    /// The advertised authorization-server URL for 401 discovery metadata.
    pub fn auth_server_url(&self) -> &str {
        &self.config.auth_server_url
    }

    async fn fetch_jwks(&self) -> Result<JwkSet, AuthError> {
        let resp = self
            .http
            .get(&self.config.jwks_url)
            .send()
            .await
            .map_err(|e| AuthError::Jwks(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(AuthError::Jwks(format!("status {}", resp.status())));
        }
        resp.json::<JwkSet>()
            .await
            .map_err(|e| AuthError::Jwks(e.to_string()))
    }

    /// Return the signing key for `kid`, refreshing the JWKS cache if the key is
    /// unknown or the cache is stale.
    async fn key_for_kid(&self, kid: &str) -> Result<DecodingKey, AuthError> {
        // Fast path: read lock, check cache freshness + presence.
        {
            let guard = self.cache.read().await;
            if let Some(cached) = guard.as_ref()
                && cached.fetched_at.elapsed() < self.ttl
                && let Some(jwk) = cached.keys.find(kid)
            {
                return DecodingKey::from_jwk(jwk)
                    .map_err(|e| AuthError::Validation(e.to_string()));
            }
        }

        // Slow path: refresh under write lock. Re-check to avoid a thundering herd.
        let mut guard = self.cache.write().await;
        let needs_refresh = match guard.as_ref() {
            None => true,
            Some(cached) => {
                cached.fetched_at.elapsed() >= self.ttl
                    || (cached.keys.find(kid).is_none()
                        && cached.fetched_at.elapsed() >= self.min_refresh_interval)
            }
        };
        if needs_refresh {
            let keys = self.fetch_jwks().await?;
            *guard = Some(CachedJwks {
                keys,
                fetched_at: Instant::now(),
            });
        }

        let cached = guard.as_ref().ok_or(AuthError::UnknownKid)?;
        let jwk = cached.keys.find(kid).ok_or(AuthError::UnknownKid)?;
        DecodingKey::from_jwk(jwk).map_err(|e| AuthError::Validation(e.to_string()))
    }

    /// Validate a bearer token and return the resulting [`Identity`].
    pub async fn validate(&self, token: &str) -> Result<Identity, AuthError> {
        let header = decode_header(token).map_err(|_| AuthError::InvalidToken)?;
        let kid = header.kid.ok_or(AuthError::NoKid)?;
        let key = self.key_for_kid(&kid).await?;

        validate_token_with_key(
            token,
            &key,
            header.alg,
            &self.config.issuer,
            &self.config.audiences,
            &self.config.email_claim,
            &self.config.fallback_claim,
        )
    }
}

/// Validate a JWT against a resolved decoding key and configured constraints,
/// returning the derived [`Identity`].
///
/// Key resolution (JWKS) is intentionally separated from claim validation so
/// the validation rules (issuer, audience, exp/nbf, identity extraction) can be
/// unit-tested with a locally-supplied key.
#[allow(clippy::too_many_arguments)]
pub fn validate_token_with_key(
    token: &str,
    key: &DecodingKey,
    alg: jsonwebtoken::Algorithm,
    issuer: &str,
    audiences: &[String],
    email_claim: &str,
    fallback_claim: &str,
) -> Result<Identity, AuthError> {
    let mut validation = Validation::new(alg);
    validation.set_issuer(&[issuer]);
    if audiences.is_empty() {
        validation.validate_aud = false;
    } else {
        validation.set_audience(audiences);
    }
    // exp/nbf validated by default; iat sanity is implied by exp/nbf.

    let data = decode::<serde_json::Value>(token, key, &validation)
        .map_err(|e| AuthError::Validation(e.to_string()))?;

    let id = identity_from_claims(&data.claims, email_claim, fallback_claim)
        .ok_or(AuthError::NoIdentity)?;

    Ok(Identity::User { id })
}

/// The runtime authenticator shared across requests.
///
/// Holds the resolved config and (in OIDC mode) the JWKS-caching validator.
#[derive(Clone)]
pub struct Authenticator {
    pub config: AuthConfig,
    validator: Option<Arc<OidcValidator>>,
}

impl Authenticator {
    pub fn new(config: AuthConfig) -> Self {
        let validator = config.oidc.clone().map(|c| Arc::new(OidcValidator::new(c)));
        Self { config, validator }
    }

    pub fn mode(&self) -> AuthMode {
        self.config.mode
    }

    /// The advertised authorization-server URL (OIDC mode only).
    pub fn auth_server_url(&self) -> Option<&str> {
        self.validator.as_ref().map(|v| v.auth_server_url())
    }

    /// Resolve the [`Identity`] for a request, given the optional bearer token.
    ///
    /// - `None`  mode → always [`Identity::Anonymous`].
    /// - `Token` mode → shared identity on match, else [`AuthError`].
    /// - `Oidc`  mode → validated per-request identity, else [`AuthError`].
    pub async fn authenticate(&self, bearer: Option<&str>) -> Result<Identity, AuthError> {
        match self.config.mode {
            AuthMode::None => Ok(Identity::Anonymous),
            AuthMode::Token => {
                let expected = self.config.token.as_deref().unwrap_or_default();
                match bearer {
                    Some(t) if t == expected => Ok(Identity::User {
                        id: self.config.token_identity.clone(),
                    }),
                    Some(_) => Err(AuthError::InvalidToken),
                    None => Err(AuthError::MissingToken),
                }
            }
            AuthMode::Oidc => {
                let token = bearer.ok_or(AuthError::MissingToken)?;
                let validator = self
                    .validator
                    .as_ref()
                    .ok_or_else(|| AuthError::Validation("oidc not configured".into()))?;
                validator.validate(token).await
            }
        }
    }
}

/// RFC 9728 OAuth Protected Resource Metadata document.
///
/// Served at `/.well-known/oauth-protected-resource` so MCP clients can
/// discover the authorization server and run the browser sign-in flow.
#[derive(Debug, Serialize)]
pub struct ProtectedResourceMetadata {
    /// The protected resource identifier (this server's public URL).
    pub resource: String,
    /// Authorization servers that issue tokens for this resource.
    pub authorization_servers: Vec<String>,
    /// Bearer token presentation method(s) supported.
    pub bearer_methods_supported: Vec<String>,
}

impl ProtectedResourceMetadata {
    pub fn new(resource: impl Into<String>, auth_server: impl Into<String>) -> Self {
        Self {
            resource: resource.into(),
            authorization_servers: vec![auth_server.into()],
            bearer_methods_supported: vec!["header".to_string()],
        }
    }
}

// Keep an explicit re-export point for the request-extension identity type used
// by the middleware/handlers. The `Identity` domain type is inserted into the
// axum request extensions directly, so no wrapper is needed; this alias documents
// intent at call sites.
pub type RequestIdentity = Identity;

#[cfg(test)]
mod tests {
    use super::*;
    use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
    use std::sync::Mutex;

    // Serialize env-mutating tests (process-global env is shared).
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    const HMAC_SECRET: &[u8] = b"unit-test-signing-secret";

    fn hmac_keys() -> (EncodingKey, DecodingKey) {
        (
            EncodingKey::from_secret(HMAC_SECRET),
            DecodingKey::from_secret(HMAC_SECRET),
        )
    }

    fn sign(claims: serde_json::Value) -> String {
        let (enc, _) = hmac_keys();
        encode(&Header::new(Algorithm::HS256), &claims, &enc).unwrap()
    }

    fn now() -> i64 {
        chrono::Utc::now().timestamp()
    }

    // --- identity_from_claims (§3.3) ---

    #[test]
    fn identity_prefers_email_claim() {
        let claims = serde_json::json!({ "email": "alice@example.com", "sub": "abc-123" });
        let id = identity_from_claims(&claims, "email", "sub");
        assert_eq!(id.as_deref(), Some("alice@example.com"));
    }

    #[test]
    fn identity_falls_back_when_email_absent() {
        let claims = serde_json::json!({ "sub": "client-cred-123" });
        let id = identity_from_claims(&claims, "email", "sub");
        assert_eq!(id.as_deref(), Some("client-cred-123"));
    }

    #[test]
    fn identity_none_when_no_usable_claim() {
        let claims = serde_json::json!({ "unrelated": "x" });
        assert!(identity_from_claims(&claims, "email", "sub").is_none());
        // Empty strings are not usable identities.
        let empty = serde_json::json!({ "email": "", "sub": "" });
        assert!(identity_from_claims(&empty, "email", "sub").is_none());
    }

    // --- JWT validation (§3.2, criteria 3/4/5) ---

    fn cfg() -> (String, Vec<String>) {
        (
            "https://issuer.example".to_string(),
            vec!["mp-api".to_string()],
        )
    }

    #[test]
    fn valid_jwt_yields_email_identity() {
        let (issuer, auds) = cfg();
        let (_, key) = hmac_keys();
        let token = sign(serde_json::json!({
            "iss": issuer,
            "aud": "mp-api",
            "email": "alice@example.com",
            "sub": "abc",
            "exp": now() + 3600,
        }));
        let id = validate_token_with_key(
            &token,
            &key,
            Algorithm::HS256,
            &issuer,
            &auds,
            "email",
            "sub",
        )
        .expect("valid token");
        assert_eq!(
            id,
            Identity::User {
                id: "alice@example.com".into()
            }
        );
    }

    #[test]
    fn m2m_client_credentials_token_uses_email_alias() {
        // A client-credentials token that still carries an email alias is treated
        // as that user identity (criterion 5) — no separate code path.
        let (issuer, auds) = cfg();
        let (_, key) = hmac_keys();
        let token = sign(serde_json::json!({
            "iss": issuer,
            "aud": "mp-api",
            "email": "agent-bot@example.com",
            "exp": now() + 3600,
        }));
        let id = validate_token_with_key(
            &token,
            &key,
            Algorithm::HS256,
            &issuer,
            &auds,
            "email",
            "sub",
        )
        .unwrap();
        assert_eq!(
            id,
            Identity::User {
                id: "agent-bot@example.com".into()
            }
        );
    }

    #[test]
    fn expired_jwt_is_rejected() {
        let (issuer, auds) = cfg();
        let (_, key) = hmac_keys();
        let token = sign(serde_json::json!({
            "iss": issuer,
            "aud": "mp-api",
            "email": "alice@example.com",
            "exp": now() - 3600, // expired an hour ago
        }));
        let res = validate_token_with_key(
            &token,
            &key,
            Algorithm::HS256,
            &issuer,
            &auds,
            "email",
            "sub",
        );
        assert!(matches!(res, Err(AuthError::Validation(_))), "got {res:?}");
    }

    #[test]
    fn wrong_audience_jwt_is_rejected() {
        let (issuer, auds) = cfg();
        let (_, key) = hmac_keys();
        let token = sign(serde_json::json!({
            "iss": issuer,
            "aud": "some-other-api",
            "email": "alice@example.com",
            "exp": now() + 3600,
        }));
        let res = validate_token_with_key(
            &token,
            &key,
            Algorithm::HS256,
            &issuer,
            &auds,
            "email",
            "sub",
        );
        assert!(matches!(res, Err(AuthError::Validation(_))), "got {res:?}");
    }

    #[test]
    fn wrong_issuer_jwt_is_rejected() {
        let (_, auds) = cfg();
        let (_, key) = hmac_keys();
        let token = sign(serde_json::json!({
            "iss": "https://evil.example",
            "aud": "mp-api",
            "email": "alice@example.com",
            "exp": now() + 3600,
        }));
        let res = validate_token_with_key(
            &token,
            &key,
            Algorithm::HS256,
            "https://issuer.example",
            &auds,
            "email",
            "sub",
        );
        assert!(matches!(res, Err(AuthError::Validation(_))), "got {res:?}");
    }

    #[test]
    fn valid_jwt_falls_back_to_sub_when_no_email() {
        let (issuer, auds) = cfg();
        let (_, key) = hmac_keys();
        let token = sign(serde_json::json!({
            "iss": issuer,
            "aud": "mp-api",
            "sub": "svc-account-7",
            "exp": now() + 3600,
        }));
        let id = validate_token_with_key(
            &token,
            &key,
            Algorithm::HS256,
            &issuer,
            &auds,
            "email",
            "sub",
        )
        .unwrap();
        assert_eq!(
            id,
            Identity::User {
                id: "svc-account-7".into()
            }
        );
    }

    // --- Authenticator (None / Token modes) ---

    #[tokio::test]
    async fn none_mode_is_anonymous() {
        let auth = Authenticator::new(AuthConfig {
            mode: AuthMode::None,
            token: None,
            token_identity: "shared-token".into(),
            oidc: None,
        });
        assert_eq!(auth.authenticate(None).await.unwrap(), Identity::Anonymous);
        // Even a bearer token is ignored in none mode.
        assert_eq!(
            auth.authenticate(Some("whatever")).await.unwrap(),
            Identity::Anonymous
        );
    }

    #[tokio::test]
    async fn token_mode_matches_shared_token() {
        let auth = Authenticator::new(AuthConfig {
            mode: AuthMode::Token,
            token: Some("s3cret".into()),
            token_identity: "shared-agent".into(),
            oidc: None,
        });
        assert_eq!(
            auth.authenticate(Some("s3cret")).await.unwrap(),
            Identity::User {
                id: "shared-agent".into()
            }
        );
        assert!(matches!(
            auth.authenticate(Some("wrong")).await,
            Err(AuthError::InvalidToken)
        ));
        assert!(matches!(
            auth.authenticate(None).await,
            Err(AuthError::MissingToken)
        ));
    }

    // --- Config parsing (§5) ---

    struct EnvGuard {
        keys: Vec<&'static str>,
    }
    impl EnvGuard {
        fn set(pairs: &[(&'static str, &str)]) -> Self {
            let keys = pairs.iter().map(|(k, _)| *k).collect();
            for (k, v) in pairs {
                unsafe { std::env::set_var(k, v) };
            }
            Self { keys }
        }
    }
    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for k in &self.keys {
                unsafe { std::env::remove_var(k) };
            }
        }
    }

    const ALL_AUTH_ENV: &[&str] = &[
        "MP_AUTH_MODE",
        "MP_AUTH_TOKEN",
        "MP_AUTH_TOKEN_IDENTITY",
        "MP_OIDC_ISSUER",
        "MP_OIDC_JWKS_URL",
        "MP_OIDC_AUDIENCES",
        "MP_OIDC_EMAIL_CLAIM",
        "MP_OIDC_FALLBACK_CLAIM",
        "MP_OIDC_AUTH_SERVER_URL",
    ];

    fn clear_auth_env() {
        for k in ALL_AUTH_ENV {
            unsafe { std::env::remove_var(k) };
        }
    }

    #[test]
    fn config_defaults_to_none_mode() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_auth_env();
        let cfg = AuthConfig::from_env().unwrap();
        assert_eq!(cfg.mode, AuthMode::None);
        assert!(cfg.oidc.is_none());
    }

    #[test]
    fn config_token_mode_requires_token() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_auth_env();
        let _g = EnvGuard::set(&[("MP_AUTH_MODE", "token")]);
        assert!(matches!(
            AuthConfig::from_env(),
            Err(ConfigError::MissingToken)
        ));
    }

    #[test]
    fn config_oidc_requires_issuer_and_derives_jwks() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_auth_env();
        // Missing issuer -> error.
        {
            let _g = EnvGuard::set(&[("MP_AUTH_MODE", "oidc")]);
            assert!(matches!(
                AuthConfig::from_env(),
                Err(ConfigError::MissingOidc("MP_OIDC_ISSUER"))
            ));
        }
        // With issuer, jwks + auth-server default from issuer; audiences parse.
        let _g = EnvGuard::set(&[
            ("MP_AUTH_MODE", "oidc"),
            ("MP_OIDC_ISSUER", "https://issuer.example/pool"),
            ("MP_OIDC_AUDIENCES", "a, b ,c"),
        ]);
        let cfg = AuthConfig::from_env().unwrap();
        let oidc = cfg.oidc.unwrap();
        assert_eq!(oidc.issuer, "https://issuer.example/pool");
        assert_eq!(
            oidc.jwks_url,
            "https://issuer.example/pool/.well-known/jwks.json"
        );
        assert_eq!(oidc.auth_server_url, "https://issuer.example/pool");
        assert_eq!(oidc.audiences, vec!["a", "b", "c"]);
        assert_eq!(oidc.email_claim, "email");
        assert_eq!(oidc.fallback_claim, "sub");
    }

    #[test]
    fn config_rejects_invalid_mode() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_auth_env();
        let _g = EnvGuard::set(&[("MP_AUTH_MODE", "bogus")]);
        assert!(matches!(
            AuthConfig::from_env(),
            Err(ConfigError::InvalidMode(_))
        ));
    }
}

//! Pluggable secrets backend port (Spec 4 §3 — GENERIC).
//!
//! The core knows only about an abstract secrets backend: it can *validate* that
//! a string looks like a well-formed reference for the configured backend, and
//! *resolve* a reference to its value at use time. It knows nothing about AWS
//! Secrets Manager, Vault, or any concrete provider — those live in the infra
//! crate as adapters implementing this trait.
//!
//! **Reference, never value.** The `reference` passed in is an opaque locator
//! (an ARN for the AWS adapter). The resolved [`SecretValue`] is returned ONLY
//! through the dedicated `wiki_get_secret` operation, never through page
//! content / `wiki_read`.

use async_trait::async_trait;

/// A resolved secret value. Deliberately NOT `Serialize`/`Deserialize` and NOT
/// `Debug`-printed as plaintext, so it cannot accidentally leak into logs,
/// page frontmatter, or transcripts. Callers must use it for the immediate
/// operation only and must not echo it into visible output.
#[derive(Clone)]
pub struct SecretValue(String);

impl SecretValue {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Borrow the plaintext for the immediate operation. Do not log or persist.
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// Consume into the plaintext string.
    pub fn into_inner(self) -> String {
        self.0
    }
}

// Redacted Debug so a `SecretValue` in a struct/error never prints the value.
impl std::fmt::Debug for SecretValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SecretValue(<redacted>)")
    }
}

/// Errors from a secrets backend. None of these carry the secret value.
#[derive(Debug, Clone, thiserror::Error)]
pub enum SecretError {
    /// The reference does not match the backend's expected pattern (e.g. it is
    /// not an ARN under the configured prefix, or it looks like a raw secret
    /// value rather than a locator). This is the code-level guardrail against
    /// storing plaintext secrets (Spec 4 §2).
    #[error("invalid secret reference: {0}")]
    InvalidReference(String),

    /// The reference is well-formed but the backend could not resolve it
    /// (not found, no permission, backend unavailable, etc.).
    #[error("secret resolution failed: {0}")]
    ResolutionFailed(String),

    /// No secrets backend is configured, so resolution is denied. This is the
    /// safe default (the deny adapter) — a deployment must opt in to a real
    /// backend.
    #[error("secrets backend not configured: {0}")]
    NotConfigured(String),
}

/// Pluggable secrets backend (Spec 4 §3).
///
/// Implementations live in the infra crate. The core depends only on this
/// trait, so it never imports any provider SDK.
#[async_trait]
pub trait SecretsResolver: Send + Sync {
    /// Validate that `reference` is a well-formed locator for this backend.
    ///
    /// Used both at save time (reject a page whose `secret_refs` contain
    /// something that isn't a reference — i.e. a raw secret) and by lint. MUST
    /// NOT perform any network call — it is a pure, synchronous pattern check.
    fn validate_reference(&self, reference: &str) -> Result<(), SecretError>;

    /// Resolve a reference to its value. Called ONLY by the access-checked,
    /// audited `wiki_get_secret` operation.
    async fn resolve(&self, reference: &str) -> Result<SecretValue, SecretError>;
}

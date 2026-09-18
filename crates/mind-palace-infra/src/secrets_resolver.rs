//! Secrets backend adapters (Spec 4 §3 — infra side).
//!
//! Two adapters implement the core [`SecretsResolver`] port:
//! - [`AwsSecretsManagerResolver`] — resolves ARNs under a configured prefix
//!   (least privilege: the deployment IAM role grants
//!   `secretsmanager:GetSecretValue` on that prefix ONLY).
//! - [`DenySecretsResolver`] — the safe no-op default. Validates nothing and
//!   denies every resolution with [`SecretError::NotConfigured`]. Used when a
//!   deployment has not opted into a real backend.
//!
//! The core never imports any of this — it depends only on the trait.

use async_trait::async_trait;
use aws_sdk_secretsmanager::Client;

use mind_palace_core::ports::secrets::{SecretError, SecretValue, SecretsResolver};

/// Configuration for the AWS Secrets Manager adapter.
///
/// `arn_prefix` is a locator pattern the deployment role is permitted to read.
/// It may end in `*` to allow any secret under a path, e.g.
/// `arn:aws:secretsmanager:us-east-1:123456789012:secret:mind-palace/*`.
/// A reference must (a) look like a Secrets Manager ARN and (b) match this
/// prefix, or it is rejected as invalid.
#[derive(Debug, Clone)]
pub struct AwsSecretsManagerConfig {
    pub arn_prefix: String,
}

pub struct AwsSecretsManagerResolver {
    client: Client,
    config: AwsSecretsManagerConfig,
}

impl AwsSecretsManagerResolver {
    pub fn new(client: Client, config: AwsSecretsManagerConfig) -> Self {
        Self { client, config }
    }

    /// True if `reference` matches the configured allow prefix. Supports a
    /// single trailing `*` wildcard (glob-style prefix match); otherwise an
    /// exact-or-startswith comparison against the literal prefix.
    fn matches_prefix(&self, reference: &str) -> bool {
        match self.config.arn_prefix.strip_suffix('*') {
            Some(stem) => reference.starts_with(stem),
            None => reference == self.config.arn_prefix,
        }
    }
}

/// A Secrets Manager ARN looks like:
/// `arn:aws:secretsmanager:<region>:<account>:secret:<name>`.
/// We do a cheap structural check (no regex dependency): the two leading
/// segments plus the `secret:` marker. This is the guardrail that rejects a raw
/// secret value pasted in place of a reference.
fn looks_like_sm_arn(reference: &str) -> bool {
    reference.starts_with("arn:aws:secretsmanager:") && reference.contains(":secret:")
}

#[async_trait]
impl SecretsResolver for AwsSecretsManagerResolver {
    fn validate_reference(&self, reference: &str) -> Result<(), SecretError> {
        if !looks_like_sm_arn(reference) {
            return Err(SecretError::InvalidReference(format!(
                "'{}' is not a Secrets Manager ARN (expected 'arn:aws:secretsmanager:…:secret:…')",
                truncate_for_error(reference)
            )));
        }
        if !self.matches_prefix(reference) {
            return Err(SecretError::InvalidReference(format!(
                "reference is outside the allowed prefix '{}'",
                self.config.arn_prefix
            )));
        }
        Ok(())
    }

    async fn resolve(&self, reference: &str) -> Result<SecretValue, SecretError> {
        // Re-validate on resolve so a stored-but-out-of-policy reference can
        // never be fetched even if it slipped past an earlier check.
        self.validate_reference(reference)?;

        let out = self
            .client
            .get_secret_value()
            .secret_id(reference)
            .send()
            .await
            .map_err(|e| SecretError::ResolutionFailed(format!("GetSecretValue failed: {e}")))?;

        if let Some(s) = out.secret_string() {
            Ok(SecretValue::new(s.to_string()))
        } else if let Some(blob) = out.secret_binary() {
            // Binary secrets are returned as UTF-8 when possible.
            match std::str::from_utf8(blob.as_ref()) {
                Ok(s) => Ok(SecretValue::new(s.to_string())),
                Err(_) => Err(SecretError::ResolutionFailed(
                    "secret is binary and not valid UTF-8".into(),
                )),
            }
        } else {
            Err(SecretError::ResolutionFailed(
                "secret has neither a string nor a binary value".into(),
            ))
        }
    }
}

/// Truncate a reference for inclusion in an error message so we never echo a
/// long pasted value in full (defense in depth — errors may be logged).
fn truncate_for_error(reference: &str) -> String {
    const MAX: usize = 24;
    if reference.len() <= MAX {
        reference.to_string()
    } else {
        format!("{}…", &reference[..MAX])
    }
}

/// The safe default backend: denies all resolution. Used when no real secrets
/// backend is configured (`MP_SECRETS_BACKEND` unset or `none`). It validates
/// nothing (so pages can still *carry* references for a future backend) but
/// never returns a value.
#[derive(Debug, Default, Clone)]
pub struct DenySecretsResolver;

#[async_trait]
impl SecretsResolver for DenySecretsResolver {
    fn validate_reference(&self, _reference: &str) -> Result<(), SecretError> {
        // No backend to validate against; accept the reference as opaque so
        // create/update aren't blocked, but resolution is denied below.
        Ok(())
    }

    async fn resolve(&self, _reference: &str) -> Result<SecretValue, SecretError> {
        Err(SecretError::NotConfigured(
            "no secrets backend is configured (set MP_SECRETS_BACKEND=aws)".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolver(prefix: &str) -> AwsSecretsManagerResolver {
        // The client is never used by validate_reference (pure), so we build a
        // resolver with a dummy client via a config-only path in tests that only
        // exercise validation. We construct the config and test matches_prefix /
        // validate_reference through a helper that doesn't need the client.
        AwsSecretsManagerResolver {
            client: dummy_client(),
            config: AwsSecretsManagerConfig {
                arn_prefix: prefix.to_string(),
            },
        }
    }

    // A minimal SM client. Not called by the validation-only tests.
    fn dummy_client() -> Client {
        let conf = aws_sdk_secretsmanager::Config::builder()
            .behavior_version(aws_sdk_secretsmanager::config::BehaviorVersion::latest())
            .region(aws_sdk_secretsmanager::config::Region::new("us-east-1"))
            .build();
        Client::from_conf(conf)
    }

    const PREFIX: &str = "arn:aws:secretsmanager:us-east-1:123456789012:secret:mind-palace/*";

    #[test]
    fn valid_arn_under_prefix_ok() {
        let r = resolver(PREFIX);
        assert!(
            r.validate_reference(
                "arn:aws:secretsmanager:us-east-1:123456789012:secret:mind-palace/db-abc123"
            )
            .is_ok()
        );
    }

    #[test]
    fn raw_secret_value_rejected() {
        let r = resolver(PREFIX);
        let err = r.validate_reference("hunter2-super-secret").unwrap_err();
        assert!(matches!(err, SecretError::InvalidReference(_)));
    }

    #[test]
    fn arn_outside_prefix_rejected() {
        let r = resolver(PREFIX);
        let err = r
            .validate_reference(
                "arn:aws:secretsmanager:us-east-1:123456789012:secret:other-app/key-xyz",
            )
            .unwrap_err();
        assert!(matches!(err, SecretError::InvalidReference(_)));
    }

    #[test]
    fn wrong_service_arn_rejected() {
        let r = resolver(PREFIX);
        // An S3 ARN is not a Secrets Manager reference.
        assert!(r.validate_reference("arn:aws:s3:::my-bucket/obj").is_err());
    }

    #[tokio::test]
    async fn deny_resolver_denies_everything() {
        let deny = DenySecretsResolver;
        assert!(deny.validate_reference("anything").is_ok());
        let err = deny.resolve("anything").await.unwrap_err();
        assert!(matches!(err, SecretError::NotConfigured(_)));
    }
}

//! Mind Palace Dreaming Process
//!
//! Scheduled ECS task that analyzes agent session logs, extracts knowledge
//! not yet captured in the wiki, and updates pages accordingly.
//!
//! Environment variables:
//!   MIND_PALACE_S3_BUCKET     - Page content bucket
//!   MIND_PALACE_S3_PREFIX     - Page key prefix (e.g., "v1")
//!   MIND_PALACE_DYNAMO_TABLE  - Graph/changelog DynamoDB table
//!   MIND_PALACE_VECTORS_BUCKET - S3 Vectors bucket
//!   MIND_PALACE_VECTORS_INDEX  - Vector index name
//!   MIND_PALACE_BEDROCK_MODEL  - Embedding model ID
//!   MIND_PALACE_REGION         - AWS region
//!   MP_LOG_BUCKET              - Session logs S3 bucket
//!   MP_LOG_PREFIX              - Session logs prefix (default: "sessions")
//!   MP_LLM_MODEL_ID            - LLM model for analysis (default: "anthropic.claude-sonnet-4-20250514-v1:0")

use tracing::{error, info};

mod dreamer;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("mind_palace_dream=info".parse().unwrap()),
        )
        .json()
        .init();

    info!("Mind Palace dreaming process starting");

    let config = dreamer::DreamConfig::from_env()?;
    let dreamer = dreamer::Dreamer::new(config).await?;

    let result = dreamer.run().await;

    match &result {
        Ok(stats) => info!(
            logs_processed = stats.logs_processed,
            pages_created = stats.pages_created,
            pages_updated = stats.pages_updated,
            logs_deleted = stats.logs_deleted,
            "Dreaming complete"
        ),
        Err(e) => error!(error = %e, "Dreaming failed"),
    }

    result.map(|_| ())
}

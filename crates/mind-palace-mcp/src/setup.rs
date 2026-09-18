//! Shared setup used by both the stdio and remote (HTTP) MCP binaries:
//! building the WikiService from environment config, constructing the tenant
//! context, and the AWS SSO cache timezone fix.

use std::sync::Arc;

use mind_palace_core::domain::graph::KnowledgeGraph;
use mind_palace_core::domain::service::WikiService;
use mind_palace_core::domain::tenant::TenantContext;
use mind_palace_core::ports::embedding::EmbeddingPort;
use mind_palace_core::ports::graph::GraphStore;
use mind_palace_core::ports::group_store::GroupStore;
use mind_palace_core::ports::page_store::PageStore;
use mind_palace_core::ports::vector_search::VectorSearchPort;
use mind_palace_infra::bedrock_embedding::{BedrockEmbedding, BedrockEmbeddingConfig};
use mind_palace_infra::dynamo_graph_store::{DynamoGraphStore, DynamoGraphStoreConfig};
use mind_palace_infra::dynamo_group_store::{DynamoGroupStore, DynamoGroupStoreConfig};
use mind_palace_infra::s3_page_store::{S3PageStore, S3PageStoreConfig};
use mind_palace_infra::s3vectors_search::{S3VectorsSearch, S3VectorsSearchConfig};
use tokio::sync::RwLock;

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

/// Build the WikiService from `MIND_PALACE_*` environment variables.
/// Shared by the stdio and remote binaries.
pub async fn build_service_from_env() -> Result<Arc<WikiService>, Box<dyn std::error::Error>> {
    let region = env_or("MIND_PALACE_REGION", "us-east-1");
    let s3_bucket = env_or("MIND_PALACE_S3_BUCKET", "mind-palace-pages");
    let s3_prefix = env_or("MIND_PALACE_S3_PREFIX", "v1");
    let dynamo_table = env_or("MIND_PALACE_DYNAMO_TABLE", "mind-palace-graph");
    let vectors_bucket = env_or("MIND_PALACE_VECTORS_BUCKET", "mind-palace-vectors");
    let vectors_index = env_or("MIND_PALACE_VECTORS_INDEX", "wiki");
    let bedrock_model = env_or("MIND_PALACE_BEDROCK_MODEL", "amazon.titan-embed-text-v2:0");

    let aws_cfg = aws_config::defaults(aws_config::BehaviorVersion::latest())
        .region(aws_config::Region::new(region))
        .load()
        .await;

    let page_store: Arc<dyn PageStore> = Arc::new(S3PageStore::new(
        aws_sdk_s3::Client::new(&aws_cfg),
        S3PageStoreConfig {
            bucket_name: s3_bucket,
            prefix: s3_prefix,
        },
    ));
    let graph_store: Arc<dyn GraphStore> = Arc::new(DynamoGraphStore::new(
        aws_sdk_dynamodb::Client::new(&aws_cfg),
        DynamoGraphStoreConfig {
            table_name: dynamo_table.clone(),
        },
    ));
    let group_store: Arc<dyn GroupStore> = Arc::new(DynamoGroupStore::new(
        aws_sdk_dynamodb::Client::new(&aws_cfg),
        DynamoGroupStoreConfig {
            table_name: dynamo_table,
        },
    ));
    let vector_search: Arc<dyn VectorSearchPort> = Arc::new(S3VectorsSearch::new(
        aws_sdk_s3vectors::Client::new(&aws_cfg),
        S3VectorsSearchConfig {
            bucket_name: vectors_bucket,
            index_name: vectors_index,
        },
    ));
    let embedding: Arc<dyn EmbeddingPort> = Arc::new(BedrockEmbedding::new(
        aws_sdk_bedrockruntime::Client::new(&aws_cfg),
        BedrockEmbeddingConfig {
            model_id: bedrock_model,
        },
    ));

    let graph = {
        let data = match graph_store.load_graph().await {
            Ok(d) => {
                tracing::info!(nodes = d.nodes.len(), edges = d.edges.len(), "loaded graph");
                d
            }
            Err(e) => {
                tracing::error!(error = ?e, "FAILED to load graph");
                Default::default()
            }
        };
        Arc::new(RwLock::new(KnowledgeGraph::from_data(data)))
    };

    Ok(Arc::new(
        WikiService::new(page_store, vector_search, embedding, graph_store, graph)
            .with_group_store(group_store),
    ))
}

/// Build the tenant context for the local stdio binary from
/// `MIND_PALACE_USER_ID` (optional).
///
/// Backward-compatibility (Spec 1, acceptance criterion 7): the stdio binary's
/// behavior for existing single-user use is unchanged. With no
/// `MIND_PALACE_USER_ID`, the context is `global()` — the trusted, everything-
/// visible local/admin identity. When `MIND_PALACE_USER_ID` is set, it becomes
/// an `Identity::User`, enabling user-scoped pages. The anonymous-sees-only-
/// public rule from Spec 1 applies to the *remote* transport, not stdio.
pub fn build_context_from_env() -> TenantContext {
    let mut ctx = TenantContext::global();
    if let Ok(user_id) = std::env::var("MIND_PALACE_USER_ID")
        && !user_id.trim().is_empty()
    {
        ctx = ctx.with_user(user_id);
    }
    ctx
}

/// Fix AWS SSO cache tokens that have timezone offsets in `expiresAt`.
///
/// The Rust AWS SDK (Smithy) only accepts UTC timestamps ending in "Z".
/// Some AWS CLI versions write local offsets like "2026-08-26T11:54:38-04:00".
/// Normalizes them in-place before the SDK reads them. Relevant when running
/// against SSO credentials (typically the local stdio binary); harmless on a
/// server using an instance/task role.
pub fn fix_sso_cache_timestamps() {
    let home = match std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")) {
        Ok(h) => h,
        Err(_) => return,
    };
    let cache_dir = std::path::Path::new(&home).join(".aws/sso/cache");
    let entries = match std::fs::read_dir(&cache_dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => continue,
        };
        if !content.contains("expiresAt") {
            continue;
        }
        let mut json: serde_json::Value = match serde_json::from_str(&content) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let expires = match json.get("expiresAt").and_then(|v| v.as_str()) {
            Some(s) => s.to_string(),
            None => continue,
        };
        if expires.ends_with('Z') {
            continue;
        }
        let fixed = match chrono::DateTime::parse_from_rfc3339(&expires) {
            Ok(dt) => dt
                .with_timezone(&chrono::Utc)
                .format("%Y-%m-%dT%H:%M:%SZ")
                .to_string(),
            Err(_) => continue,
        };
        json["expiresAt"] = serde_json::Value::String(fixed);
        if let Ok(output) = serde_json::to_string_pretty(&json) {
            let _ = std::fs::write(&path, output);
        }
    }
}

use std::sync::Arc;

use mind_palace_core::domain::graph::KnowledgeGraph;
use mind_palace_core::domain::service::WikiService;
use mind_palace_core::domain::tenant::TenantContext;
use mind_palace_core::ports::embedding::EmbeddingPort;
use mind_palace_core::ports::graph::GraphStore;
use mind_palace_core::ports::page_store::PageStore;
use mind_palace_core::ports::vector_search::VectorSearchPort;
use mind_palace_infra::bedrock_embedding::{BedrockEmbedding, BedrockEmbeddingConfig};
use mind_palace_infra::dynamo_graph_store::{DynamoGraphStore, DynamoGraphStoreConfig};
use mind_palace_infra::s3_page_store::{S3PageStore, S3PageStoreConfig};
use mind_palace_infra::s3vectors_search::{S3VectorsSearch, S3VectorsSearchConfig};
use mind_palace_mcp::MindPalaceMcpServer;
use rmcp::ServiceExt;
use rmcp::transport::stdio;
use tokio::sync::RwLock;

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Fix SSO cache timestamps that have timezone offsets (e.g., "-04:00")
    // which the Rust AWS SDK cannot parse. Convert them to UTC "Z" format.
    fix_sso_cache_timestamps();

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
                eprintln!(
                    "Mind Palace MCP: loaded graph with {} nodes, {} edges",
                    d.nodes.len(),
                    d.edges.len()
                );
                d
            }
            Err(e) => {
                eprintln!("Mind Palace MCP: FAILED to load graph: {:?}", e);
                Default::default()
            }
        };
        Arc::new(RwLock::new(KnowledgeGraph::from_data(data)))
    };

    let service = Arc::new(WikiService::new(
        page_store,
        vector_search,
        embedding,
        graph_store,
        graph,
    ));

    let ctx = {
        let mut ctx = TenantContext::global();
        if let Ok(user_id) = std::env::var("MIND_PALACE_USER_ID") {
            ctx = ctx.with_user(user_id);
        }
        ctx
    };

    // User name is available for display/attribution but doesn't affect visibility
    if let Ok(user_name) = std::env::var("MIND_PALACE_USER_NAME") {
        eprintln!("Mind Palace MCP: user={}", user_name);
    }

    let server = MindPalaceMcpServer::new(service, ctx);

    let service = server.serve(stdio()).await.inspect_err(|e| {
        eprintln!("MCP server error: {:?}", e);
    })?;

    service.waiting().await?;
    Ok(())
}

/// Fix AWS SSO cache tokens that have timezone offsets in `expiresAt`.
///
/// The Rust AWS SDK (Smithy) only accepts UTC timestamps ending in "Z".
/// Some AWS CLI versions write local offsets like "2026-08-26T11:54:38-04:00".
/// This function normalizes them in-place before the SDK tries to read them.
fn fix_sso_cache_timestamps() {
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
        // Quick check: does it have expiresAt with an offset?
        if !content.contains("expiresAt") {
            continue;
        }
        // Look for a pattern like "2026-08-26T11:54:38-04:00" or "+05:30"
        // UTC timestamps end with Z and don't need fixing
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
        // Parse and convert to UTC
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

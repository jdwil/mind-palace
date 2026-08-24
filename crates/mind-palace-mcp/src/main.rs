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

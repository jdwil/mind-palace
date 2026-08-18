use std::sync::Arc;

use mind_palace_core::domain::graph::KnowledgeGraph;
use mind_palace_core::domain::service::WikiService;
use mind_palace_core::domain::tenant::TenantContext;
use mind_palace_core::error::MindPalaceError;
use mind_palace_core::ports::changelog::ChangelogStore;
use mind_palace_core::ports::embedding::EmbeddingPort;
use mind_palace_core::ports::graph::GraphStore;
use mind_palace_core::ports::page_store::PageStore;
use mind_palace_core::ports::vector_search::VectorSearchPort;
use mind_palace_infra::bedrock_embedding::{BedrockEmbedding, BedrockEmbeddingConfig};
use mind_palace_infra::dynamo_graph_store::{DynamoGraphStore, DynamoGraphStoreConfig};
use mind_palace_infra::s3_page_store::{S3PageStore, S3PageStoreConfig};
use mind_palace_infra::s3vectors_search::{S3VectorsSearch, S3VectorsSearchConfig};
use mind_palace_rig::tools::*;
use mind_palace_rig::vector_index::MindPalaceVectorIndex;
use tokio::sync::RwLock;

pub use mind_palace_core as core;

// --- Config structs ---

pub struct S3Config {
    pub bucket_name: String,
    pub region: String,
    pub prefix: String,
}

pub struct DynamoConfig {
    pub table_name: String,
    pub region: String,
}

pub struct S3VectorsConfig {
    pub bucket_name: String,
    pub index_name: String,
    pub region: String,
}

pub struct BedrockConfig {
    pub model_id: String,
    pub region: String,
}

// --- Builder ---

pub struct MindPalaceBuilder {
    s3: Option<S3Config>,
    dynamo: Option<DynamoConfig>,
    s3vectors: Option<S3VectorsConfig>,
    bedrock: Option<BedrockConfig>,
    tenancy_enabled: bool,
    changelog: Option<Arc<dyn ChangelogStore>>,
}

impl MindPalaceBuilder {
    pub fn new() -> Self {
        Self {
            s3: None,
            dynamo: None,
            s3vectors: None,
            bedrock: None,
            tenancy_enabled: false,
            changelog: None,
        }
    }

    pub fn s3(mut self, config: S3Config) -> Self {
        self.s3 = Some(config);
        self
    }

    pub fn dynamo(mut self, config: DynamoConfig) -> Self {
        self.dynamo = Some(config);
        self
    }

    pub fn s3vectors(mut self, config: S3VectorsConfig) -> Self {
        self.s3vectors = Some(config);
        self
    }

    pub fn bedrock(mut self, config: BedrockConfig) -> Self {
        self.bedrock = Some(config);
        self
    }

    pub fn enable_tenancy(mut self, enabled: bool) -> Self {
        self.tenancy_enabled = enabled;
        self
    }

    pub fn changelog(mut self, store: Arc<dyn ChangelogStore>) -> Self {
        self.changelog = Some(store);
        self
    }

    pub async fn build(self) -> Result<MindPalace, MindPalaceError> {
        let s3_cfg = self
            .s3
            .ok_or_else(|| MindPalaceError::Validation("s3 config required".into()))?;
        let dynamo_cfg = self
            .dynamo
            .ok_or_else(|| MindPalaceError::Validation("dynamo config required".into()))?;
        let s3vec_cfg = self
            .s3vectors
            .ok_or_else(|| MindPalaceError::Validation("s3vectors config required".into()))?;
        let bedrock_cfg = self
            .bedrock
            .ok_or_else(|| MindPalaceError::Validation("bedrock config required".into()))?;

        // Load AWS configs per region
        let s3_aws = aws_config::defaults(aws_config::BehaviorVersion::latest())
            .region(aws_config::Region::new(s3_cfg.region.clone()))
            .load()
            .await;
        let dynamo_aws = aws_config::defaults(aws_config::BehaviorVersion::latest())
            .region(aws_config::Region::new(dynamo_cfg.region.clone()))
            .load()
            .await;
        let s3vec_aws = aws_config::defaults(aws_config::BehaviorVersion::latest())
            .region(aws_config::Region::new(s3vec_cfg.region.clone()))
            .load()
            .await;
        let bedrock_aws = aws_config::defaults(aws_config::BehaviorVersion::latest())
            .region(aws_config::Region::new(bedrock_cfg.region.clone()))
            .load()
            .await;

        // Construct SDK clients
        let s3_client = aws_sdk_s3::Client::new(&s3_aws);
        let dynamo_client = aws_sdk_dynamodb::Client::new(&dynamo_aws);
        let s3vec_client = aws_sdk_s3vectors::Client::new(&s3vec_aws);
        let bedrock_client = aws_sdk_bedrockruntime::Client::new(&bedrock_aws);

        // Build real adapters
        let page_store: Arc<dyn PageStore> = Arc::new(S3PageStore::new(
            s3_client,
            S3PageStoreConfig {
                bucket_name: s3_cfg.bucket_name,
                prefix: s3_cfg.prefix,
            },
        ));
        let graph_store: Arc<dyn GraphStore> = Arc::new(DynamoGraphStore::new(
            dynamo_client,
            DynamoGraphStoreConfig {
                table_name: dynamo_cfg.table_name,
            },
        ));
        let vector_search: Arc<dyn VectorSearchPort> = Arc::new(S3VectorsSearch::new(
            s3vec_client,
            S3VectorsSearchConfig {
                bucket_name: s3vec_cfg.bucket_name,
                index_name: s3vec_cfg.index_name,
            },
        ));
        let embedding: Arc<dyn EmbeddingPort> = Arc::new(BedrockEmbedding::new(
            bedrock_client,
            BedrockEmbeddingConfig {
                model_id: bedrock_cfg.model_id,
            },
        ));

        let graph = {
            let data = graph_store.load_graph().await.unwrap_or_default();
            Arc::new(RwLock::new(KnowledgeGraph::from_data(data)))
        };

        let service = Arc::new({
            let svc = WikiService::new(page_store, vector_search, embedding, graph_store, graph);
            if let Some(changelog) = self.changelog {
                svc.with_changelog(changelog)
            } else {
                svc
            }
        });

        let ctx = TenantContext::global();
        let tools = MindPalaceTools {
            search: WikiSearchTool {
                service: service.clone(),
                ctx: ctx.clone(),
            },
            read: WikiReadTool {
                service: service.clone(),
                ctx: ctx.clone(),
            },
            traverse: WikiTraverseTool {
                service: service.clone(),
                ctx: ctx.clone(),
            },
            create: WikiCreateTool {
                service: service.clone(),
                ctx: ctx.clone(),
            },
            update: WikiUpdateTool {
                service: service.clone(),
                ctx: ctx.clone(),
            },
            list: WikiListTool {
                service: service.clone(),
                ctx: ctx.clone(),
            },
        };

        let vector_index = MindPalaceVectorIndex::new(service.clone(), ctx);

        Ok(MindPalace {
            service,
            tools,
            vector_index,
            _tenancy_enabled: self.tenancy_enabled,
        })
    }
}

impl Default for MindPalaceBuilder {
    fn default() -> Self {
        Self::new()
    }
}

// --- MindPalace facade ---

pub struct MindPalaceTools {
    pub search: WikiSearchTool,
    pub read: WikiReadTool,
    pub traverse: WikiTraverseTool,
    pub create: WikiCreateTool,
    pub update: WikiUpdateTool,
    pub list: WikiListTool,
}

pub struct MindPalace {
    service: Arc<WikiService>,
    tools: MindPalaceTools,
    vector_index: MindPalaceVectorIndex,
    _tenancy_enabled: bool,
}

impl MindPalace {
    pub fn builder() -> MindPalaceBuilder {
        MindPalaceBuilder::new()
    }

    pub fn wiki_service(&self) -> &WikiService {
        &self.service
    }

    pub fn tools(&self) -> &MindPalaceTools {
        &self.tools
    }

    pub fn vector_index(&self) -> &MindPalaceVectorIndex {
        &self.vector_index
    }
}

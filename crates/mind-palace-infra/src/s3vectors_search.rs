use async_trait::async_trait;
use aws_sdk_s3vectors::Client;
use aws_sdk_s3vectors::types::{PutInputVector, VectorData};
use aws_smithy_types::Document;

use mind_palace_core::domain::tenant::TenantContext;
use mind_palace_core::domain::value_objects::{PageId, Slug, Visibility};
use mind_palace_core::error::MindPalaceError;
use mind_palace_core::ports::vector_search::{EmbeddingMetadata, SearchResult, VectorSearchPort};

#[derive(Debug, Clone)]
pub struct S3VectorsSearchConfig {
    pub bucket_name: String,
    pub index_name: String,
}

pub struct S3VectorsSearch {
    client: Client,
    config: S3VectorsSearchConfig,
}

impl S3VectorsSearch {
    pub fn new(client: Client, config: S3VectorsSearchConfig) -> Self {
        Self { client, config }
    }
}

fn json_value_to_document(val: &serde_json::Value) -> Document {
    match val {
        serde_json::Value::Null => Document::Null,
        serde_json::Value::Bool(b) => Document::Bool(*b),
        serde_json::Value::Number(n) => {
            Document::Number(aws_smithy_types::Number::Float(n.as_f64().unwrap_or(0.0)))
        }
        serde_json::Value::String(s) => Document::String(s.clone()),
        serde_json::Value::Array(arr) => {
            Document::Array(arr.iter().map(json_value_to_document).collect())
        }
        serde_json::Value::Object(map) => {
            let hm = map
                .iter()
                .map(|(k, v)| (k.clone(), json_value_to_document(v)))
                .collect();
            Document::Object(hm)
        }
    }
}

fn document_to_json_value(doc: &Document) -> serde_json::Value {
    match doc {
        Document::Null => serde_json::Value::Null,
        Document::Bool(b) => serde_json::Value::Bool(*b),
        Document::Number(n) => serde_json::Value::Number(
            serde_json::Number::from_f64(n.to_f64_lossy()).unwrap_or(serde_json::Number::from(0)),
        ),
        Document::String(s) => serde_json::Value::String(s.clone()),
        Document::Array(arr) => {
            serde_json::Value::Array(arr.iter().map(document_to_json_value).collect())
        }
        Document::Object(map) => {
            let m = map
                .iter()
                .map(|(k, v)| (k.clone(), document_to_json_value(v)))
                .collect();
            serde_json::Value::Object(m)
        }
    }
}

#[async_trait]
impl VectorSearchPort for S3VectorsSearch {
    async fn search(
        &self,
        query_embedding: &[f64],
        limit: usize,
        ctx: &TenantContext,
    ) -> Result<Vec<SearchResult>, MindPalaceError> {
        let float32_vec: Vec<f32> = query_embedding.iter().map(|&v| v as f32).collect();

        let mut req = self
            .client
            .query_vectors()
            .vector_bucket_name(&self.config.bucket_name)
            .index_name(&self.config.index_name)
            .top_k(limit as i32)
            .query_vector(VectorData::Float32(float32_vec))
            .return_metadata(true);

        // Build filter for tenant visibility
        if ctx.tenant_id.is_some() {
            let mut visible: Vec<Document> = ctx
                .visible_tenants
                .iter()
                .map(|t| Document::String(t.0.clone()))
                .chain(std::iter::once(Document::String("general".to_string())))
                .collect();
            if let Some(uid) = &ctx.user_id {
                visible.push(Document::String(format!("user-{uid}")));
            }
            let filter = Document::Object(
                [(
                    "visibility".to_string(),
                    Document::Object(
                        [("$in".to_string(), Document::Array(visible))]
                            .into_iter()
                            .collect(),
                    ),
                )]
                .into_iter()
                .collect(),
            );
            req = req.filter(filter);
        }

        let resp = req
            .send()
            .await
            .map_err(|e| MindPalaceError::Store(e.to_string()))?;

        let results = resp
            .vectors()
            .iter()
            .filter_map(|v| {
                let metadata = v.metadata()?;
                let json = document_to_json_value(metadata);
                let page_id_str = json.get("page_id")?.as_str()?;
                let uuid = uuid::Uuid::parse_str(page_id_str).ok()?;
                let slug_str = json.get("slug")?.as_str()?;
                let slug = Slug::new(slug_str).ok()?;
                let title = json.get("title")?.as_str()?.to_string();
                let summary = json
                    .get("summary")
                    .and_then(|s| s.as_str())
                    .unwrap_or("")
                    .to_string();
                let score = v.distance().unwrap_or(0.0) as f64;
                Some(SearchResult {
                    page_id: PageId(uuid),
                    slug,
                    title,
                    summary,
                    score,
                })
            })
            .collect();

        Ok(results)
    }

    async fn upsert_embedding(
        &self,
        metadata: &EmbeddingMetadata,
        embedding: &[f64],
    ) -> Result<(), MindPalaceError> {
        let float32_vec: Vec<f32> = embedding.iter().map(|&v| v as f32).collect();

        let visibility_str = match &metadata.visibility {
            Visibility::General => "general".to_string(),
            Visibility::Tenant(tid) => tid.0.clone(),
            Visibility::User(uid) => format!("user-{uid}"),
        };

        let meta_json = serde_json::json!({
            "page_id": metadata.page_id.0.to_string(),
            "slug": metadata.slug.as_str(),
            "title": metadata.title,
            "visibility": visibility_str,
        });
        let meta_doc = json_value_to_document(&meta_json);

        let vector = PutInputVector::builder()
            .key(metadata.page_id.0.to_string())
            .data(VectorData::Float32(float32_vec))
            .metadata(meta_doc)
            .build()
            .map_err(|e| MindPalaceError::Store(e.to_string()))?;

        self.client
            .put_vectors()
            .vector_bucket_name(&self.config.bucket_name)
            .index_name(&self.config.index_name)
            .vectors(vector)
            .send()
            .await
            .map_err(|e| MindPalaceError::Store(e.to_string()))?;
        Ok(())
    }

    async fn delete_embedding(&self, page_id: &PageId) -> Result<(), MindPalaceError> {
        self.client
            .delete_vectors()
            .vector_bucket_name(&self.config.bucket_name)
            .index_name(&self.config.index_name)
            .keys(page_id.0.to_string())
            .send()
            .await
            .map_err(|e| MindPalaceError::Store(e.to_string()))?;
        Ok(())
    }
}

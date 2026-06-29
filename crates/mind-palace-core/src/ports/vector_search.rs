use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::domain::tenant::TenantContext;
use crate::domain::value_objects::{PageId, Slug, Visibility};
use crate::error::MindPalaceError;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub page_id: PageId,
    pub slug: Slug,
    pub title: String,
    pub summary: String,
    pub score: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingMetadata {
    pub page_id: PageId,
    pub slug: Slug,
    pub title: String,
    pub visibility: Visibility,
}

#[async_trait]
pub trait VectorSearchPort: Send + Sync {
    async fn search(
        &self,
        query_embedding: &[f64],
        limit: usize,
        ctx: &TenantContext,
    ) -> Result<Vec<SearchResult>, MindPalaceError>;

    async fn upsert_embedding(
        &self,
        metadata: &EmbeddingMetadata,
        embedding: &[f64],
    ) -> Result<(), MindPalaceError>;

    async fn delete_embedding(&self, page_id: &PageId) -> Result<(), MindPalaceError>;
}

use std::sync::Arc;

use mind_palace_core::domain::service::WikiService;
use mind_palace_core::domain::tenant::TenantContext;
use rig_core::vector_store::request::{Filter, VectorSearchRequest};
use rig_core::vector_store::{VectorStoreError, VectorStoreIndex};
use serde::Deserialize;
use serde_json::Value;

pub struct MindPalaceVectorIndex {
    service: Arc<WikiService>,
    ctx: TenantContext,
}

impl MindPalaceVectorIndex {
    pub fn new(service: Arc<WikiService>, ctx: TenantContext) -> Self {
        Self { service, ctx }
    }
}

impl VectorStoreIndex for MindPalaceVectorIndex {
    type Filter = Filter<Value>;

    async fn top_n<T: for<'a> Deserialize<'a> + Send>(
        &self,
        req: VectorSearchRequest<Self::Filter>,
    ) -> Result<Vec<(f64, String, T)>, VectorStoreError> {
        let query = req.query();
        let n = req.samples() as usize;
        let results = self
            .service
            .search(query, &self.ctx, n)
            .await
            .map_err(|e| VectorStoreError::DatastoreError(Box::new(e)))?;
        results
            .into_iter()
            .map(|r| {
                let id = r.slug.as_str().to_string();
                let score = r.score;
                let doc = serde_json::json!({
                    "slug": id,
                    "title": r.title,
                    "summary": r.summary,
                });
                let t: T = serde_json::from_value(doc)
                    .map_err(|e| VectorStoreError::DatastoreError(Box::new(e)))?;
                Ok((score, id, t))
            })
            .collect()
    }

    async fn top_n_ids(
        &self,
        req: VectorSearchRequest<Self::Filter>,
    ) -> Result<Vec<(f64, String)>, VectorStoreError> {
        let query = req.query();
        let n = req.samples() as usize;
        let results = self
            .service
            .search(query, &self.ctx, n)
            .await
            .map_err(|e| VectorStoreError::DatastoreError(Box::new(e)))?;
        Ok(results
            .into_iter()
            .map(|r| (r.score, r.slug.as_str().to_string()))
            .collect())
    }
}

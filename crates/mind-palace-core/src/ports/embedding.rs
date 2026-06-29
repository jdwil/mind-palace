use async_trait::async_trait;

use crate::error::MindPalaceError;

#[async_trait]
pub trait EmbeddingPort: Send + Sync {
    async fn embed_text(&self, text: &str) -> Result<Vec<f64>, MindPalaceError>;

    async fn embed_texts(&self, texts: &[&str]) -> Result<Vec<Vec<f64>>, MindPalaceError>;
}

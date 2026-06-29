use async_trait::async_trait;
use aws_sdk_bedrockruntime::Client;
use aws_sdk_bedrockruntime::primitives::Blob;

use mind_palace_core::error::MindPalaceError;
use mind_palace_core::ports::embedding::EmbeddingPort;

#[derive(Debug, Clone)]
pub struct BedrockEmbeddingConfig {
    pub model_id: String,
}

impl Default for BedrockEmbeddingConfig {
    fn default() -> Self {
        Self {
            model_id: "amazon.titan-embed-text-v2:0".to_string(),
        }
    }
}

pub struct BedrockEmbedding {
    client: Client,
    config: BedrockEmbeddingConfig,
}

impl BedrockEmbedding {
    pub fn new(client: Client, config: BedrockEmbeddingConfig) -> Self {
        Self { client, config }
    }
}

#[async_trait]
impl EmbeddingPort for BedrockEmbedding {
    async fn embed_text(&self, text: &str) -> Result<Vec<f64>, MindPalaceError> {
        let body = serde_json::json!({
            "inputText": text
        });
        let body_bytes =
            serde_json::to_vec(&body).map_err(|e| MindPalaceError::Embedding(e.to_string()))?;

        let resp = self
            .client
            .invoke_model()
            .model_id(&self.config.model_id)
            .content_type("application/json")
            .accept("application/json")
            .body(Blob::new(body_bytes))
            .send()
            .await
            .map_err(|e| MindPalaceError::Embedding(e.to_string()))?;

        let resp_bytes = resp.body.into_inner();
        let resp_json: serde_json::Value = serde_json::from_slice(&resp_bytes)
            .map_err(|e| MindPalaceError::Embedding(e.to_string()))?;

        let embedding = resp_json
            .get("embedding")
            .and_then(|v| v.as_array())
            .ok_or_else(|| MindPalaceError::Embedding("missing embedding in response".into()))?
            .iter()
            .filter_map(|v| v.as_f64())
            .collect();

        Ok(embedding)
    }

    async fn embed_texts(&self, texts: &[&str]) -> Result<Vec<Vec<f64>>, MindPalaceError> {
        let mut results = Vec::with_capacity(texts.len());
        for text in texts {
            results.push(self.embed_text(text).await?);
        }
        Ok(results)
    }
}

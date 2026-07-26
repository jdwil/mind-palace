use std::env;
use std::sync::Arc;

use aws_config::Region;
use aws_sdk_s3::Client as S3Client;
use chrono::{Duration, Utc};
use mind_palace::{
    BedrockConfig, DynamoConfig, MindPalace, MindPalaceBuilder, S3Config, S3VectorsConfig,
};
use mind_palace_core::domain::service::WikiService;
use mind_palace_core::domain::tenant::TenantContext;
use mind_palace_core::ports::changelog::{ChangeAction, ChangelogEntry, ChangelogStore};
use mind_palace_infra::dynamo_changelog_store::DynamoChangelogStore;
use rig_core::client::CompletionClient;
use rig_core::completion::Prompt;
use tracing::{debug, info, warn};

/// Configuration loaded from environment.
pub struct DreamConfig {
    // Mind Palace config
    pub s3_bucket: String,
    pub s3_prefix: String,
    pub dynamo_table: String,
    pub vectors_bucket: String,
    pub vectors_index: String,
    pub bedrock_model: String,
    pub region: String,
    // Logs config
    pub log_bucket: String,
    pub log_prefix: String,
    // LLM config
    pub llm_model_id: String,
}

impl DreamConfig {
    pub fn from_env() -> Result<Self, Box<dyn std::error::Error>> {
        Ok(Self {
            s3_bucket: env::var("MIND_PALACE_S3_BUCKET")?,
            s3_prefix: env::var("MIND_PALACE_S3_PREFIX").unwrap_or_else(|_| "v1".into()),
            dynamo_table: env::var("MIND_PALACE_DYNAMO_TABLE")?,
            vectors_bucket: env::var("MIND_PALACE_VECTORS_BUCKET")?,
            vectors_index: env::var("MIND_PALACE_VECTORS_INDEX")
                .unwrap_or_else(|_| "wiki-pages".into()),
            bedrock_model: env::var("MIND_PALACE_BEDROCK_MODEL")
                .unwrap_or_else(|_| "amazon.titan-embed-text-v2:0".into()),
            region: env::var("MIND_PALACE_REGION").unwrap_or_else(|_| "us-west-2".into()),
            log_bucket: env::var("MP_LOG_BUCKET")?,
            log_prefix: env::var("MP_LOG_PREFIX").unwrap_or_else(|_| "sessions".into()),
            llm_model_id: env::var("MP_LLM_MODEL_ID")
                .unwrap_or_else(|_| "anthropic.claude-sonnet-4-20250514-v1:0".into()),
        })
    }
}

/// Statistics from a dream run.
#[derive(Debug, Default)]
pub struct DreamStats {
    pub logs_processed: usize,
    pub pages_created: usize,
    pub pages_updated: usize,
    pub logs_deleted: usize,
}

pub struct Dreamer {
    config: DreamConfig,
    s3_client: S3Client,
    changelog: Arc<DynamoChangelogStore>,
    palace: MindPalace,
}

impl Dreamer {
    pub async fn new(config: DreamConfig) -> Result<Self, Box<dyn std::error::Error>> {
        let aws_config = aws_config::defaults(aws_config::BehaviorVersion::latest())
            .region(Region::new(config.region.clone()))
            .load()
            .await;

        let s3_client = S3Client::new(&aws_config);
        let dynamo_client = aws_sdk_dynamodb::Client::new(&aws_config);

        let changelog = Arc::new(DynamoChangelogStore::new(
            dynamo_client,
            config.dynamo_table.clone(),
        ));

        let palace = MindPalaceBuilder::new()
            .s3(S3Config {
                bucket_name: config.s3_bucket.clone(),
                region: config.region.clone(),
                prefix: config.s3_prefix.clone(),
            })
            .dynamo(DynamoConfig {
                table_name: config.dynamo_table.clone(),
                region: config.region.clone(),
            })
            .s3vectors(S3VectorsConfig {
                bucket_name: config.vectors_bucket.clone(),
                index_name: config.vectors_index.clone(),
                region: config.region.clone(),
            })
            .bedrock(BedrockConfig {
                model_id: config.bedrock_model.clone(),
                region: config.region.clone(),
            })
            .build()
            .await?;

        Ok(Self {
            config,
            s3_client,
            changelog,
            palace,
        })
    }

    pub async fn run(&self) -> Result<DreamStats, Box<dyn std::error::Error>> {
        let mut stats = DreamStats::default();

        // 1. List all session logs
        let logs = self.list_logs().await?;
        info!(count = logs.len(), "Found session logs to process");

        if logs.is_empty() {
            info!("No logs to process, exiting");
            return Ok(stats);
        }

        // 2. Get recent changelog entries for context
        let recent_changes = self
            .changelog
            .since(Utc::now() - Duration::days(7), Some(100))
            .await
            .unwrap_or_default();
        info!(
            count = recent_changes.len(),
            "Recent wiki changes for context"
        );

        // 3. Process each log
        for log_key in &logs {
            match self.process_log(log_key, &recent_changes).await {
                Ok((created, updated)) => {
                    stats.logs_processed += 1;
                    stats.pages_created += created;
                    stats.pages_updated += updated;

                    // Delete processed log
                    if self.delete_log(log_key).await.is_ok() {
                        stats.logs_deleted += 1;
                    }
                }
                Err(e) => {
                    warn!(key = %log_key, error = %e, "Failed to process log, skipping");
                }
            }
        }

        // 4. Prune old changelog entries (older than 30 days)
        let prune_before = Utc::now() - Duration::days(30);
        match self.changelog.prune_before(prune_before).await {
            Ok(pruned) => info!(count = pruned, "Pruned old changelog entries"),
            Err(e) => warn!(error = %e, "Failed to prune changelog"),
        }

        Ok(stats)
    }

    /// List all session log keys in the log bucket.
    async fn list_logs(&self) -> Result<Vec<String>, Box<dyn std::error::Error>> {
        let mut keys = Vec::new();
        let mut continuation_token: Option<String> = None;

        loop {
            let mut req = self
                .s3_client
                .list_objects_v2()
                .bucket(&self.config.log_bucket)
                .prefix(&self.config.log_prefix);

            if let Some(token) = &continuation_token {
                req = req.continuation_token(token);
            }

            let resp = req.send().await?;

            for obj in resp.contents() {
                if let Some(key) = obj.key()
                    && key.ends_with(".jsonl")
                {
                    keys.push(key.to_string());
                }
            }

            if resp.is_truncated() == Some(true) {
                continuation_token = resp.next_continuation_token().map(|s| s.to_string());
            } else {
                break;
            }
        }

        Ok(keys)
    }

    /// Read a session log from S3.
    async fn read_log(&self, key: &str) -> Result<String, Box<dyn std::error::Error>> {
        let resp = self
            .s3_client
            .get_object()
            .bucket(&self.config.log_bucket)
            .key(key)
            .send()
            .await?;

        let body = resp.body.collect().await?;
        Ok(String::from_utf8_lossy(&body.into_bytes()).to_string())
    }

    /// Delete a processed log from S3.
    async fn delete_log(&self, key: &str) -> Result<(), Box<dyn std::error::Error>> {
        self.s3_client
            .delete_object()
            .bucket(&self.config.log_bucket)
            .key(key)
            .send()
            .await?;
        debug!(key = %key, "Deleted processed log");
        Ok(())
    }

    /// Process a single session log: read it, ask the LLM to extract knowledge,
    /// and update the wiki. Returns (pages_created, pages_updated).
    async fn process_log(
        &self,
        key: &str,
        recent_changes: &[ChangelogEntry],
    ) -> Result<(usize, usize), Box<dyn std::error::Error>> {
        info!(key = %key, "Processing session log");

        let log_content = self.read_log(key).await?;

        // Truncate very large logs to stay within context window
        let log_content = if log_content.len() > 200_000 {
            warn!(key = %key, size = log_content.len(), "Log too large, truncating");
            log_content[..200_000].to_string()
        } else {
            log_content
        };

        // Build context about recent wiki changes
        let changes_context = if recent_changes.is_empty() {
            "No recent wiki changes.".to_string()
        } else {
            let entries: Vec<String> = recent_changes
                .iter()
                .take(20)
                .map(|e| {
                    format!(
                        "- {} {} ({})",
                        match &e.action {
                            ChangeAction::Created => "Created",
                            ChangeAction::Updated => "Updated",
                            ChangeAction::Deleted => "Deleted",
                        },
                        e.slug.as_str(),
                        e.summary.as_deref().unwrap_or("no summary")
                    )
                })
                .collect();
            format!("Recent wiki changes:\n{}", entries.join("\n"))
        };

        // Build fresh tool instances for this call (tools are moved into the agent builder)
        let service = self.wiki_service();
        let ctx = TenantContext::global();

        let search_tool = mind_palace_rig::tools::WikiSearchTool {
            service: service.clone(),
            ctx: ctx.clone(),
        };
        let read_tool = mind_palace_rig::tools::WikiReadTool {
            service: service.clone(),
            ctx: ctx.clone(),
        };
        let create_tool = mind_palace_rig::tools::WikiCreateTool {
            service: service.clone(),
            ctx: ctx.clone(),
        };
        let update_tool = mind_palace_rig::tools::WikiUpdateTool {
            service: service.clone(),
            ctx: ctx.clone(),
        };
        let list_tool = mind_palace_rig::tools::WikiListTool {
            service: service.clone(),
            ctx,
        };

        // Build a Bedrock client for the LLM (Claude)
        let bedrock_client = rig_bedrock::client::ClientBuilder::default()
            .region(&self.config.region)
            .build()
            .await;

        let agent = bedrock_client
            .agent(&self.config.llm_model_id)
            .preamble(DREAMER_SYSTEM_PROMPT)
            .tool(search_tool)
            .tool(read_tool)
            .tool(create_tool)
            .tool(update_tool)
            .tool(list_tool)
            .build();

        let user_prompt = format!(
            "Analyze this session log and update the wiki with any knowledge not already captured.\n\n\
            {changes_context}\n\n\
            --- SESSION LOG ---\n\
            {log_content}\n\
            --- END SESSION LOG ---\n\n\
            Instructions:\n\
            1. Search the wiki for topics mentioned in this session\n\
            2. If knowledge from this session is already captured, skip it\n\
            3. If new knowledge exists, either update existing pages or create new ones\n\
            4. Use appropriate page types: Concept for ideas, Decision for choices made, Entity for specific things, Leaf for detailed reference\n\
            5. Always add links to related existing pages\n\
            6. Be concise — store the insight, not the raw conversation\n\
            7. Report what you did at the end"
        );

        let response = agent.prompt(&user_prompt).await?;
        debug!(response = %response, "Agent response");

        // Parse response for stats (rough heuristic)
        let created = response.matches("Created page").count()
            + response.matches("created page").count()
            + response.matches("Created:").count();
        let updated = response.matches("Updated page").count()
            + response.matches("updated page").count()
            + response.matches("Updated:").count();

        Ok((created, updated))
    }

    /// Get a reference to the wiki service Arc for constructing tools.
    fn wiki_service(&self) -> Arc<WikiService> {
        // Access the service through the palace's tools (they hold Arc<WikiService>)
        self.palace.tools().search.service.clone()
    }
}

const DREAMER_SYSTEM_PROMPT: &str = r#"You are the Mind Palace Dreamer — a knowledge consolidation agent.

Your job is to analyze agent session logs and ensure valuable knowledge is captured in the wiki. You have access to wiki tools: search, read, create, update, and list.

Principles:
- Store insights, not conversations. Synthesize before writing.
- Check before writing. Always search first to avoid duplicates.
- Update over create. If a page exists on the topic, update it rather than creating a new one.
- Link everything. Always include links to related pages.
- Be concise. Summaries should be 1-2 sentences. Sections should be focused.
- Use appropriate page types:
  - Concept: synthesized understanding of a topic
  - Decision: a choice that was made with rationale
  - Entity: a specific thing (project, service, person)
  - Leaf: deep reference material
  - Index: lightweight hub linking related pages

What to extract from session logs:
- Decisions made (and their rationale)
- New understanding of systems or domains
- Patterns discovered
- Configuration or setup procedures
- Architecture choices

What to skip:
- Trivial back-and-forth
- Information already well-captured in existing pages
- Raw code without conceptual insight
- Temporary debugging that won't matter next week

After analyzing, report what you did: "Created page: X", "Updated page: Y", or "No new knowledge found."
"#;

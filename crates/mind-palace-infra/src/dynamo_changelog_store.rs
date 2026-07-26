use async_trait::async_trait;
use aws_sdk_dynamodb::Client;
use aws_sdk_dynamodb::types::AttributeValue;
use chrono::{DateTime, Utc};

use mind_palace_core::domain::value_objects::{PageId, Slug};
use mind_palace_core::error::MindPalaceError;
use mind_palace_core::ports::changelog::{ChangeAction, ChangelogEntry, ChangelogStore};

const PK_VALUE: &str = "CHANGELOG";

/// DynamoDB adapter for the ChangelogStore port.
///
/// Reuses the existing graph table with:
/// - PK = "CHANGELOG" (constant)
/// - SK = "{iso_timestamp}#{uuid}" (ensures uniqueness and sort order)
pub struct DynamoChangelogStore {
    client: Client,
    table_name: String,
}

impl DynamoChangelogStore {
    pub fn new(client: Client, table_name: String) -> Self {
        Self { client, table_name }
    }

    fn pk() -> AttributeValue {
        AttributeValue::S(PK_VALUE.into())
    }

    fn sk(entry: &ChangelogEntry) -> AttributeValue {
        let ts = entry.timestamp.to_rfc3339();
        let id = uuid::Uuid::new_v4();
        AttributeValue::S(format!("{}#{}", ts, id))
    }

    fn parse_entry(
        item: &std::collections::HashMap<String, AttributeValue>,
    ) -> Result<ChangelogEntry, MindPalaceError> {
        let sk = get_s(item, "SK")?;
        let timestamp = parse_timestamp_from_sk(&sk)?;
        let slug_str = get_s(item, "slug")?;
        let slug = Slug::new(&slug_str).map_err(|e| MindPalaceError::Changelog(e.to_string()))?;
        let page_id_str = get_s(item, "page_id")?;
        let page_id_uuid = uuid::Uuid::parse_str(&page_id_str)
            .map_err(|e| MindPalaceError::Changelog(e.to_string()))?;
        let page_id = PageId(page_id_uuid);
        let action_str = get_s(item, "action")?;
        let action: ChangeAction = serde_json::from_str(&format!("\"{}\"", action_str))
            .map_err(|e| MindPalaceError::Changelog(e.to_string()))?;
        let agent_id = get_opt_s(item, "agent_id");
        let summary = get_opt_s(item, "summary");

        Ok(ChangelogEntry {
            timestamp,
            slug,
            page_id,
            action,
            agent_id,
            summary,
        })
    }
}

fn get_s(
    item: &std::collections::HashMap<String, AttributeValue>,
    key: &str,
) -> Result<String, MindPalaceError> {
    item.get(key)
        .and_then(|v| v.as_s().ok())
        .map(|s| s.to_string())
        .ok_or_else(|| MindPalaceError::Changelog(format!("missing attribute: {}", key)))
}

fn get_opt_s(
    item: &std::collections::HashMap<String, AttributeValue>,
    key: &str,
) -> Option<String> {
    item.get(key)
        .and_then(|v| v.as_s().ok())
        .map(|s| s.to_string())
}

fn parse_timestamp_from_sk(sk: &str) -> Result<DateTime<Utc>, MindPalaceError> {
    // SK format: "{iso_timestamp}#{uuid}"
    let ts_str = sk
        .split('#')
        .next()
        .ok_or_else(|| MindPalaceError::Changelog("invalid SK format".into()))?;
    DateTime::parse_from_rfc3339(ts_str)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|e| MindPalaceError::Changelog(format!("invalid timestamp in SK: {}", e)))
}

fn action_to_string(action: &ChangeAction) -> String {
    match action {
        ChangeAction::Created => "Created".into(),
        ChangeAction::Updated => "Updated".into(),
        ChangeAction::Deleted => "Deleted".into(),
    }
}

#[async_trait]
impl ChangelogStore for DynamoChangelogStore {
    async fn append(&self, entry: &ChangelogEntry) -> Result<(), MindPalaceError> {
        let mut req = self
            .client
            .put_item()
            .table_name(&self.table_name)
            .item("PK", Self::pk())
            .item("SK", Self::sk(entry))
            .item("slug", AttributeValue::S(entry.slug.as_str().to_string()))
            .item("page_id", AttributeValue::S(entry.page_id.0.to_string()))
            .item("action", AttributeValue::S(action_to_string(&entry.action)));

        if let Some(ref agent_id) = entry.agent_id {
            req = req.item("agent_id", AttributeValue::S(agent_id.clone()));
        }
        if let Some(ref summary) = entry.summary {
            req = req.item("summary", AttributeValue::S(summary.clone()));
        }

        req.send()
            .await
            .map_err(|e| MindPalaceError::Changelog(e.to_string()))?;

        Ok(())
    }

    async fn since(
        &self,
        since: DateTime<Utc>,
        limit: Option<usize>,
    ) -> Result<Vec<ChangelogEntry>, MindPalaceError> {
        let since_sk = since.to_rfc3339();

        let mut req = self
            .client
            .query()
            .table_name(&self.table_name)
            .key_condition_expression("PK = :pk AND SK > :since")
            .expression_attribute_values(":pk", Self::pk())
            .expression_attribute_values(":since", AttributeValue::S(since_sk))
            .scan_index_forward(true);

        if let Some(l) = limit {
            req = req.limit(l as i32);
        }

        let resp = req
            .send()
            .await
            .map_err(|e| MindPalaceError::Changelog(e.to_string()))?;

        let mut entries = Vec::new();
        for item in resp.items() {
            entries.push(Self::parse_entry(item)?);
        }

        Ok(entries)
    }

    async fn prune_before(&self, before: DateTime<Utc>) -> Result<u64, MindPalaceError> {
        let before_sk = before.to_rfc3339();
        let mut deleted: u64 = 0;
        let mut last_key = None;

        loop {
            let mut req = self
                .client
                .query()
                .table_name(&self.table_name)
                .key_condition_expression("PK = :pk AND SK < :before")
                .expression_attribute_values(":pk", Self::pk())
                .expression_attribute_values(":before", AttributeValue::S(before_sk.clone()))
                .scan_index_forward(true);

            if let Some(key) = last_key.take() {
                req = req.set_exclusive_start_key(Some(key));
            }

            let resp = req
                .send()
                .await
                .map_err(|e| MindPalaceError::Changelog(e.to_string()))?;

            let items = resp.items();
            if items.is_empty() {
                break;
            }

            // BatchWriteItem in chunks of 25
            for chunk in items.chunks(25) {
                let delete_requests: Vec<_> = chunk
                    .iter()
                    .map(|item| {
                        let pk = item.get("PK").cloned().unwrap();
                        let sk = item.get("SK").cloned().unwrap();
                        aws_sdk_dynamodb::types::WriteRequest::builder()
                            .delete_request(
                                aws_sdk_dynamodb::types::DeleteRequest::builder()
                                    .key("PK", pk)
                                    .key("SK", sk)
                                    .build()
                                    .unwrap(),
                            )
                            .build()
                    })
                    .collect();

                let batch_size = delete_requests.len() as u64;
                self.client
                    .batch_write_item()
                    .request_items(&self.table_name, delete_requests)
                    .send()
                    .await
                    .map_err(|e| MindPalaceError::Changelog(e.to_string()))?;

                deleted += batch_size;
            }

            last_key = resp.last_evaluated_key().map(|k| k.to_owned());
            if last_key.is_none() {
                break;
            }
        }

        Ok(deleted)
    }
}

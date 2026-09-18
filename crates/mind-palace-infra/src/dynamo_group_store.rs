use async_trait::async_trait;
use aws_sdk_dynamodb::Client;
use aws_sdk_dynamodb::types::AttributeValue;

use mind_palace_core::domain::group::Group;
use mind_palace_core::domain::value_objects::GroupId;
use mind_palace_core::error::MindPalaceError;
use mind_palace_core::ports::group_store::GroupStore;

/// DynamoDB-backed [`GroupStore`] (Spec 2 §2.2, §7).
///
/// Groups are a new item type in the existing graph table:
/// `PK = GROUP#<id>`, `SK = META`, with the group JSON stored in a `data`
/// attribute. This keeps groups in the same on-demand table as page nodes and
/// edges without a schema change.
#[derive(Debug, Clone)]
pub struct DynamoGroupStoreConfig {
    pub table_name: String,
}

pub struct DynamoGroupStore {
    client: Client,
    config: DynamoGroupStoreConfig,
}

impl DynamoGroupStore {
    pub fn new(client: Client, config: DynamoGroupStoreConfig) -> Self {
        Self { client, config }
    }

    fn pk(id: &GroupId) -> AttributeValue {
        AttributeValue::S(format!("GROUP#{}", id.as_str()))
    }
}

#[async_trait]
impl GroupStore for DynamoGroupStore {
    async fn get_group(&self, id: &GroupId) -> Result<Option<Group>, MindPalaceError> {
        let resp = self
            .client
            .get_item()
            .table_name(&self.config.table_name)
            .key("PK", Self::pk(id))
            .key("SK", AttributeValue::S("META".into()))
            .send()
            .await
            .map_err(|e| MindPalaceError::Graph(e.to_string()))?;

        let Some(item) = resp.item() else {
            return Ok(None);
        };
        let data = item
            .get("data")
            .and_then(|v| v.as_s().ok())
            .ok_or_else(|| MindPalaceError::Graph("group item missing data attribute".into()))?;
        let group: Group =
            serde_json::from_str(data).map_err(|e| MindPalaceError::Graph(e.to_string()))?;
        Ok(Some(group))
    }

    async fn list_groups(&self) -> Result<Vec<Group>, MindPalaceError> {
        // Scan for GROUP# items. Group counts are small; a filtered scan is fine.
        let mut groups = Vec::new();
        let mut last_key = None;
        loop {
            let mut req = self
                .client
                .scan()
                .table_name(&self.config.table_name)
                .filter_expression("begins_with(PK, :g) AND SK = :m")
                .expression_attribute_values(":g", AttributeValue::S("GROUP#".into()))
                .expression_attribute_values(":m", AttributeValue::S("META".into()));
            if let Some(key) = last_key.take() {
                req = req.set_exclusive_start_key(Some(key));
            }
            let resp = req
                .send()
                .await
                .map_err(|e| MindPalaceError::Graph(e.to_string()))?;
            for item in resp.items() {
                if let Some(data) = item.get("data").and_then(|v| v.as_s().ok())
                    && let Ok(group) = serde_json::from_str::<Group>(data)
                {
                    groups.push(group);
                }
            }
            last_key = resp.last_evaluated_key().map(|k| k.to_owned());
            if last_key.is_none() {
                break;
            }
        }
        Ok(groups)
    }

    async fn save_group(&self, group: &Group) -> Result<(), MindPalaceError> {
        let data =
            serde_json::to_string(group).map_err(|e| MindPalaceError::Graph(e.to_string()))?;
        self.client
            .put_item()
            .table_name(&self.config.table_name)
            .item("PK", Self::pk(&group.id))
            .item("SK", AttributeValue::S("META".into()))
            .item("name", AttributeValue::S(group.name.clone()))
            .item("data", AttributeValue::S(data))
            .send()
            .await
            .map_err(|e| MindPalaceError::Graph(e.to_string()))?;
        Ok(())
    }

    async fn delete_group(&self, id: &GroupId) -> Result<(), MindPalaceError> {
        self.client
            .delete_item()
            .table_name(&self.config.table_name)
            .key("PK", Self::pk(id))
            .key("SK", AttributeValue::S("META".into()))
            .send()
            .await
            .map_err(|e| MindPalaceError::Graph(e.to_string()))?;
        Ok(())
    }
}

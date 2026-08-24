use async_trait::async_trait;
use aws_sdk_dynamodb::Client;
use aws_sdk_dynamodb::types::AttributeValue;

use mind_palace_core::domain::value_objects::{EdgeKind, PageId, Slug};
use mind_palace_core::error::MindPalaceError;
use mind_palace_core::ports::graph::{GraphData, GraphEdgeData, GraphNodeData, GraphStore};

#[derive(Debug, Clone)]
pub struct DynamoGraphStoreConfig {
    pub table_name: String,
}

pub struct DynamoGraphStore {
    client: Client,
    config: DynamoGraphStoreConfig,
}

impl DynamoGraphStore {
    pub fn new(client: Client, config: DynamoGraphStoreConfig) -> Self {
        Self { client, config }
    }

    fn pk(page_id: &PageId) -> AttributeValue {
        AttributeValue::S(format!("PAGE#{}", page_id.0))
    }

    fn parse_page_id(pk: &str) -> Result<PageId, MindPalaceError> {
        let id_str = pk
            .strip_prefix("PAGE#")
            .ok_or_else(|| MindPalaceError::Graph("invalid PK format".into()))?;
        let uuid =
            uuid::Uuid::parse_str(id_str).map_err(|e| MindPalaceError::Graph(e.to_string()))?;
        Ok(PageId(uuid))
    }
}

fn get_s(
    item: &std::collections::HashMap<String, AttributeValue>,
    key: &str,
) -> Result<String, MindPalaceError> {
    item.get(key)
        .and_then(|v| v.as_s().ok())
        .map(|s| s.to_string())
        .ok_or_else(|| MindPalaceError::Graph(format!("missing attribute: {}", key)))
}

#[async_trait]
impl GraphStore for DynamoGraphStore {
    async fn load_graph(&self) -> Result<GraphData, MindPalaceError> {
        let mut nodes = Vec::new();
        let mut edges = Vec::new();
        let mut last_key = None;

        loop {
            let mut req = self.client.scan().table_name(&self.config.table_name);
            if let Some(key) = last_key.take() {
                req = req.set_exclusive_start_key(Some(key));
            }
            let resp = req
                .send()
                .await
                .map_err(|e| MindPalaceError::Graph(format!("{e}")))?;

            for item in resp.items() {
                let sk = get_s(item, "SK")?;
                let pk = get_s(item, "PK")?;

                if sk == "META" {
                    let page_id = Self::parse_page_id(&pk)?;
                    let slug = Slug::new(&get_s(item, "slug")?)
                        .map_err(|e| MindPalaceError::Graph(e.to_string()))?;
                    let title = get_s(item, "title")?;
                    let summary = get_s(item, "summary")?;
                    let visibility_str = get_s(item, "visibility")?;
                    let visibility = serde_json::from_str(&visibility_str)
                        .map_err(|e| MindPalaceError::Graph(e.to_string()))?;
                    let page_type_str = get_s(item, "page_type")?;
                    let page_type = serde_json::from_str(&page_type_str)
                        .map_err(|e| MindPalaceError::Graph(e.to_string()))?;

                    nodes.push(GraphNodeData {
                        page_id,
                        slug,
                        title,
                        summary,
                        visibility,
                        page_type,
                    });
                } else if let Some(target_str) = sk.strip_prefix("EDGE#") {
                    let source = Self::parse_page_id(&pk)?;
                    let target_uuid = uuid::Uuid::parse_str(target_str)
                        .map_err(|e| MindPalaceError::Graph(e.to_string()))?;
                    let target = PageId(target_uuid);
                    let kind_str = get_s(item, "edge_kind")?;
                    let kind: EdgeKind = serde_json::from_str(&kind_str)
                        .map_err(|e| MindPalaceError::Graph(e.to_string()))?;
                    edges.push(GraphEdgeData {
                        source,
                        target,
                        kind,
                    });
                }
                // Skip BACKLINK# items — they're just reverse pointers
            }

            last_key = resp.last_evaluated_key().map(|k| k.to_owned());
            if last_key.is_none() {
                break;
            }
        }

        Ok(GraphData { nodes, edges })
    }

    async fn save_node(&self, node: &GraphNodeData) -> Result<(), MindPalaceError> {
        let visibility_json = serde_json::to_string(&node.visibility)
            .map_err(|e| MindPalaceError::Graph(e.to_string()))?;
        let page_type_json = serde_json::to_string(&node.page_type)
            .map_err(|e| MindPalaceError::Graph(e.to_string()))?;

        self.client
            .put_item()
            .table_name(&self.config.table_name)
            .item("PK", Self::pk(&node.page_id))
            .item("SK", AttributeValue::S("META".into()))
            .item("slug", AttributeValue::S(node.slug.as_str().to_string()))
            .item("title", AttributeValue::S(node.title.clone()))
            .item("summary", AttributeValue::S(node.summary.clone()))
            .item("visibility", AttributeValue::S(visibility_json))
            .item("page_type", AttributeValue::S(page_type_json))
            .send()
            .await
            .map_err(|e| MindPalaceError::Graph(e.to_string()))?;
        Ok(())
    }

    async fn save_edge(&self, edge: &GraphEdgeData) -> Result<(), MindPalaceError> {
        let kind_json =
            serde_json::to_string(&edge.kind).map_err(|e| MindPalaceError::Graph(e.to_string()))?;

        // Forward edge
        self.client
            .put_item()
            .table_name(&self.config.table_name)
            .item("PK", Self::pk(&edge.source))
            .item("SK", AttributeValue::S(format!("EDGE#{}", edge.target.0)))
            .item("edge_kind", AttributeValue::S(kind_json.clone()))
            .send()
            .await
            .map_err(|e| MindPalaceError::Graph(e.to_string()))?;

        // Backlink
        self.client
            .put_item()
            .table_name(&self.config.table_name)
            .item("PK", Self::pk(&edge.target))
            .item(
                "SK",
                AttributeValue::S(format!("BACKLINK#{}", edge.source.0)),
            )
            .item("edge_kind", AttributeValue::S(kind_json))
            .send()
            .await
            .map_err(|e| MindPalaceError::Graph(e.to_string()))?;
        Ok(())
    }

    async fn delete_node(&self, id: &PageId) -> Result<(), MindPalaceError> {
        // Query all items for this PK
        let resp = self
            .client
            .query()
            .table_name(&self.config.table_name)
            .key_condition_expression("PK = :pk")
            .expression_attribute_values(":pk", Self::pk(id))
            .send()
            .await
            .map_err(|e| MindPalaceError::Graph(e.to_string()))?;

        let items = resp.items();
        if items.is_empty() {
            return Ok(());
        }

        // BatchWriteItem to delete all (max 25 per batch)
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

            self.client
                .batch_write_item()
                .request_items(&self.config.table_name, delete_requests)
                .send()
                .await
                .map_err(|e| MindPalaceError::Graph(e.to_string()))?;
        }
        Ok(())
    }

    async fn delete_edge(&self, source: &PageId, target: &PageId) -> Result<(), MindPalaceError> {
        // Delete forward edge
        self.client
            .delete_item()
            .table_name(&self.config.table_name)
            .key("PK", Self::pk(source))
            .key("SK", AttributeValue::S(format!("EDGE#{}", target.0)))
            .send()
            .await
            .map_err(|e| MindPalaceError::Graph(e.to_string()))?;

        // Delete backlink
        self.client
            .delete_item()
            .table_name(&self.config.table_name)
            .key("PK", Self::pk(target))
            .key("SK", AttributeValue::S(format!("BACKLINK#{}", source.0)))
            .send()
            .await
            .map_err(|e| MindPalaceError::Graph(e.to_string()))?;
        Ok(())
    }
}

use async_trait::async_trait;

use crate::domain::value_objects::{EdgeKind, PageId, PageType, Slug, Visibility};
use crate::error::MindPalaceError;

/// A node as stored/loaded from the graph backend.
#[derive(Debug, Clone)]
pub struct GraphNodeData {
    pub page_id: PageId,
    pub slug: Slug,
    pub title: String,
    pub summary: String,
    pub visibility: Visibility,
    pub page_type: PageType,
}

/// An edge as stored/loaded from the graph backend.
#[derive(Debug, Clone)]
pub struct GraphEdgeData {
    pub source: PageId,
    pub target: PageId,
    pub kind: EdgeKind,
}

/// Full graph data loaded from persistence.
#[derive(Debug, Clone, Default)]
pub struct GraphData {
    pub nodes: Vec<GraphNodeData>,
    pub edges: Vec<GraphEdgeData>,
}

#[async_trait]
pub trait GraphStore: Send + Sync {
    async fn load_graph(&self) -> Result<GraphData, MindPalaceError>;

    async fn save_node(&self, node: &GraphNodeData) -> Result<(), MindPalaceError>;

    async fn save_edge(&self, edge: &GraphEdgeData) -> Result<(), MindPalaceError>;

    async fn delete_node(&self, id: &PageId) -> Result<(), MindPalaceError>;

    async fn delete_edge(&self, source: &PageId, target: &PageId) -> Result<(), MindPalaceError>;
}

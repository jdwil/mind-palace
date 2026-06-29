use std::collections::HashMap;

use petgraph::Direction;
use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::visit::EdgeRef;

use super::tenant::TenantContext;
use super::value_objects::{EdgeKind, PageId, PageType, Slug, Visibility};
use crate::ports::graph::GraphData;

#[cfg(test)]
use crate::ports::graph::{GraphEdgeData, GraphNodeData};

#[derive(Debug, Clone)]
pub struct GraphNode {
    pub page_id: PageId,
    pub slug: Slug,
    pub title: String,
    pub summary: String,
    pub visibility: Visibility,
    pub page_type: PageType,
}

#[derive(Debug, Clone)]
pub struct GraphEdge {
    pub kind: EdgeKind,
}

/// Lightweight neighbor info returned by traversal (minimal tokens).
#[derive(Debug, Clone)]
pub struct NeighborInfo {
    pub page_id: PageId,
    pub slug: Slug,
    pub title: String,
    pub summary: String,
    pub page_type: PageType,
    pub edge_kind: EdgeKind,
}

pub struct KnowledgeGraph {
    graph: DiGraph<GraphNode, GraphEdge>,
    index_map: HashMap<PageId, NodeIndex>,
}

impl KnowledgeGraph {
    pub fn new() -> Self {
        Self {
            graph: DiGraph::new(),
            index_map: HashMap::new(),
        }
    }

    pub fn from_data(data: GraphData) -> Self {
        let mut kg = Self::new();
        for node in data.nodes {
            kg.add_node(GraphNode {
                page_id: node.page_id,
                slug: node.slug,
                title: node.title,
                summary: node.summary,
                visibility: node.visibility,
                page_type: node.page_type,
            });
        }
        for edge in data.edges {
            kg.add_edge(&edge.source, &edge.target, edge.kind);
        }
        kg
    }

    pub fn add_node(&mut self, node: GraphNode) -> NodeIndex {
        let page_id = node.page_id.clone();
        let idx = self.graph.add_node(node);
        self.index_map.insert(page_id, idx);
        idx
    }

    pub fn remove_node(&mut self, page_id: &PageId) {
        if let Some(idx) = self.index_map.remove(page_id) {
            self.graph.remove_node(idx);
            // Rebuild index_map since NodeIndex values may shift after removal
            self.index_map.clear();
            for idx in self.graph.node_indices() {
                let node = &self.graph[idx];
                self.index_map.insert(node.page_id.clone(), idx);
            }
        }
    }

    pub fn add_edge(&mut self, source: &PageId, target: &PageId, kind: EdgeKind) {
        if let (Some(&src_idx), Some(&tgt_idx)) =
            (self.index_map.get(source), self.index_map.get(target))
        {
            self.graph.add_edge(src_idx, tgt_idx, GraphEdge { kind });
        }
    }

    pub fn remove_edge(&mut self, source: &PageId, target: &PageId) {
        if let (Some(&src_idx), Some(&tgt_idx)) =
            (self.index_map.get(source), self.index_map.get(target))
            && let Some(edge) = self.graph.find_edge(src_idx, tgt_idx)
        {
            self.graph.remove_edge(edge);
        }
    }

    pub fn get_node(&self, page_id: &PageId) -> Option<&GraphNode> {
        self.index_map.get(page_id).map(|&idx| &self.graph[idx])
    }

    pub fn get_neighbors(
        &self,
        page_id: &PageId,
        direction: Direction,
        ctx: &TenantContext,
    ) -> Vec<NeighborInfo> {
        let Some(&idx) = self.index_map.get(page_id) else {
            return Vec::new();
        };

        self.graph
            .edges_directed(idx, direction)
            .filter_map(|edge_ref| {
                let neighbor_idx = match direction {
                    Direction::Outgoing => edge_ref.target(),
                    Direction::Incoming => edge_ref.source(),
                };
                let neighbor = &self.graph[neighbor_idx];
                if ctx.can_see(&neighbor.visibility) {
                    Some(NeighborInfo {
                        page_id: neighbor.page_id.clone(),
                        slug: neighbor.slug.clone(),
                        title: neighbor.title.clone(),
                        summary: neighbor.summary.clone(),
                        page_type: neighbor.page_type.clone(),
                        edge_kind: edge_ref.weight().kind.clone(),
                    })
                } else {
                    None
                }
            })
            .collect()
    }

    pub fn get_subtree(
        &self,
        root: &PageId,
        max_depth: usize,
        ctx: &TenantContext,
    ) -> Vec<NeighborInfo> {
        let mut results = Vec::new();
        let mut visited = HashMap::new();
        let mut queue = std::collections::VecDeque::new();

        if let Some(&idx) = self.index_map.get(root) {
            visited.insert(idx, 0usize);
            queue.push_back((idx, 0usize));
        }

        while let Some((current_idx, depth)) = queue.pop_front() {
            if depth >= max_depth {
                continue;
            }
            for edge_ref in self.graph.edges_directed(current_idx, Direction::Outgoing) {
                let neighbor_idx = edge_ref.target();
                if visited.contains_key(&neighbor_idx) {
                    continue;
                }
                let neighbor = &self.graph[neighbor_idx];
                if !ctx.can_see(&neighbor.visibility) {
                    continue;
                }
                visited.insert(neighbor_idx, depth + 1);
                queue.push_back((neighbor_idx, depth + 1));
                results.push(NeighborInfo {
                    page_id: neighbor.page_id.clone(),
                    slug: neighbor.slug.clone(),
                    title: neighbor.title.clone(),
                    summary: neighbor.summary.clone(),
                    page_type: neighbor.page_type.clone(),
                    edge_kind: edge_ref.weight().kind.clone(),
                });
            }
        }
        results
    }

    pub fn get_index_pages(&self, ctx: &TenantContext) -> Vec<&GraphNode> {
        self.graph
            .node_indices()
            .filter_map(|idx| {
                let node = &self.graph[idx];
                if node.page_type == PageType::Index && ctx.can_see(&node.visibility) {
                    Some(node)
                } else {
                    None
                }
            })
            .collect()
    }

    pub fn node_count(&self) -> usize {
        self.graph.node_count()
    }

    pub fn edge_count(&self) -> usize {
        self.graph.edge_count()
    }

    pub fn find_by_slug(&self, slug: &Slug, ctx: &TenantContext) -> Option<&GraphNode> {
        self.graph.node_indices().find_map(|idx| {
            let node = &self.graph[idx];
            if &node.slug == slug && ctx.can_see(&node.visibility) {
                Some(node)
            } else {
                None
            }
        })
    }

    pub fn all_nodes(&self, ctx: &TenantContext) -> Vec<&GraphNode> {
        self.graph
            .node_indices()
            .filter_map(|idx| {
                let node = &self.graph[idx];
                if ctx.can_see(&node.visibility) {
                    Some(node)
                } else {
                    None
                }
            })
            .collect()
    }

    pub fn has_node(&self, page_id: &PageId) -> bool {
        self.index_map.contains_key(page_id)
    }
}

impl Default for KnowledgeGraph {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_node(id: &str, slug: &str, ptype: PageType, vis: Visibility) -> GraphNode {
        GraphNode {
            page_id: PageId(uuid::Uuid::new_v4()),
            slug: Slug::new(slug).unwrap(),
            title: id.to_string(),
            summary: format!("Summary of {id}"),
            visibility: vis,
            page_type: ptype,
        }
    }

    #[test]
    fn add_and_retrieve_node() {
        let mut kg = KnowledgeGraph::new();
        let node = make_node("Index", "index", PageType::Index, Visibility::General);
        let pid = node.page_id.clone();
        kg.add_node(node);
        assert_eq!(kg.node_count(), 1);
        assert!(kg.get_node(&pid).is_some());
    }

    #[test]
    fn remove_node_cleans_up() {
        let mut kg = KnowledgeGraph::new();
        let n1 = make_node("A", "a", PageType::Concept, Visibility::General);
        let n2 = make_node("B", "b", PageType::Leaf, Visibility::General);
        let pid1 = n1.page_id.clone();
        let pid2 = n2.page_id.clone();
        kg.add_node(n1);
        kg.add_node(n2);
        kg.add_edge(&pid1, &pid2, EdgeKind::Child);
        assert_eq!(kg.node_count(), 2);

        kg.remove_node(&pid1);
        assert_eq!(kg.node_count(), 1);
        assert!(!kg.has_node(&pid1));
        assert!(kg.has_node(&pid2));
    }

    #[test]
    fn get_neighbors_respects_visibility() {
        let mut kg = KnowledgeGraph::new();
        let general = make_node("General", "general", PageType::Concept, Visibility::General);
        let tenant_a = make_node(
            "TenantA",
            "tenant-a",
            PageType::Leaf,
            Visibility::Tenant(super::super::value_objects::TenantId::new("a")),
        );
        let tenant_b = make_node(
            "TenantB",
            "tenant-b",
            PageType::Leaf,
            Visibility::Tenant(super::super::value_objects::TenantId::new("b")),
        );

        let pid_g = general.page_id.clone();
        let pid_a = tenant_a.page_id.clone();
        let pid_b = tenant_b.page_id.clone();

        kg.add_node(general);
        kg.add_node(tenant_a);
        kg.add_node(tenant_b);
        kg.add_edge(&pid_g, &pid_a, EdgeKind::Child);
        kg.add_edge(&pid_g, &pid_b, EdgeKind::Child);

        // Tenant A can only see general + own pages
        let ctx_a = TenantContext::leaf(super::super::value_objects::TenantId::new("a"));
        let neighbors = kg.get_neighbors(&pid_g, Direction::Outgoing, &ctx_a);
        assert_eq!(neighbors.len(), 1);
        assert_eq!(neighbors[0].slug.as_str(), "tenant-a");

        // Global sees both
        let ctx_global = TenantContext::global();
        let neighbors = kg.get_neighbors(&pid_g, Direction::Outgoing, &ctx_global);
        assert_eq!(neighbors.len(), 2);
    }

    #[test]
    fn subtree_traversal() {
        let mut kg = KnowledgeGraph::new();
        let root = make_node("Root", "root", PageType::Index, Visibility::General);
        let mid = make_node("Mid", "mid", PageType::Concept, Visibility::General);
        let leaf = make_node("Leaf", "leaf", PageType::Leaf, Visibility::General);

        let pid_root = root.page_id.clone();
        let pid_mid = mid.page_id.clone();
        let pid_leaf = leaf.page_id.clone();

        kg.add_node(root);
        kg.add_node(mid);
        kg.add_node(leaf);
        kg.add_edge(&pid_root, &pid_mid, EdgeKind::Child);
        kg.add_edge(&pid_mid, &pid_leaf, EdgeKind::Child);

        let ctx = TenantContext::global();

        // Depth 1: only mid
        let results = kg.get_subtree(&pid_root, 1, &ctx);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].slug.as_str(), "mid");

        // Depth 2: mid + leaf
        let results = kg.get_subtree(&pid_root, 2, &ctx);
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn get_index_pages_filters() {
        let mut kg = KnowledgeGraph::new();
        kg.add_node(make_node(
            "Idx",
            "idx",
            PageType::Index,
            Visibility::General,
        ));
        kg.add_node(make_node(
            "Concept",
            "concept",
            PageType::Concept,
            Visibility::General,
        ));

        let ctx = TenantContext::global();
        let indexes = kg.get_index_pages(&ctx);
        assert_eq!(indexes.len(), 1);
        assert_eq!(indexes[0].title, "Idx");
    }

    #[test]
    fn from_graph_data() {
        let pid1 = PageId::new();
        let pid2 = PageId::new();
        let data = GraphData {
            nodes: vec![
                GraphNodeData {
                    page_id: pid1.clone(),
                    slug: Slug::new("one").unwrap(),
                    title: "One".into(),
                    summary: "S1".into(),
                    visibility: Visibility::General,
                    page_type: PageType::Index,
                },
                GraphNodeData {
                    page_id: pid2.clone(),
                    slug: Slug::new("two").unwrap(),
                    title: "Two".into(),
                    summary: "S2".into(),
                    visibility: Visibility::General,
                    page_type: PageType::Leaf,
                },
            ],
            edges: vec![GraphEdgeData {
                source: pid1.clone(),
                target: pid2.clone(),
                kind: EdgeKind::Child,
            }],
        };

        let kg = KnowledgeGraph::from_data(data);
        assert_eq!(kg.node_count(), 2);
        assert_eq!(kg.edge_count(), 1);
        let neighbors = kg.get_neighbors(&pid1, Direction::Outgoing, &TenantContext::global());
        assert_eq!(neighbors.len(), 1);
        assert_eq!(neighbors[0].slug.as_str(), "two");
    }
}

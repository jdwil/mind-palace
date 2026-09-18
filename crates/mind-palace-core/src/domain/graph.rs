use std::collections::HashMap;

use petgraph::Direction;
use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::visit::EdgeRef;

use super::tenant::TenantContext;
use super::value_objects::{EdgeKind, PageAccess, PageId, PageType, Slug, Visibility};
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
    /// Access model (Spec 2) for owner/grant filtering during traversal/list.
    pub access: PageAccess,
    pub page_type: PageType,
    /// Containment parent (Spec 3) — used to resolve inherited access by walking
    /// upward along Parent edges. `None` = a root page (inherits nothing).
    pub parent: Option<Slug>,
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
                access: node.access,
                page_type: node.page_type,
                parent: node.parent,
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

    pub fn get_node_mut(&mut self, page_id: &PageId) -> Option<&mut GraphNode> {
        self.index_map.get(page_id).map(|&idx| &mut self.graph[idx])
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
                if self.can_see_node(neighbor, ctx) {
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
                if !self.can_see_node(neighbor, ctx) {
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
                if node.page_type == PageType::Index && self.can_see_node(node, ctx) {
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
            if &node.slug == slug && self.can_see_node(node, ctx) {
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
                if self.can_see_node(node, ctx) {
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

    /// Find a node by slug WITHOUT access filtering. Used internally by the
    /// containment walk, which must resolve ancestors regardless of whether the
    /// current identity can see them — an ancestor a user cannot directly see
    /// can still grant that user access to a descendant.
    fn node_by_slug_unfiltered(&self, slug: &Slug) -> Option<&GraphNode> {
        self.graph.node_indices().find_map(|idx| {
            let node = &self.graph[idx];
            if &node.slug == slug { Some(node) } else { None }
        })
    }

    /// Walk the containment chain upward from `page_id` via `parent` pointers,
    /// returning the ancestor nodes' slugs from nearest parent to root.
    ///
    /// Bounded by a visited-set cycle guard: even though writes reject cycles
    /// (see [`would_create_cycle`]), a malformed store must never cause an
    /// infinite loop (Spec 3 §3.4). Stops at the first repeated slug.
    pub fn ancestor_slugs(&self, page_id: &PageId) -> Vec<Slug> {
        let mut chain = Vec::new();
        let mut visited: std::collections::HashSet<String> = std::collections::HashSet::new();

        let Some(start) = self.get_node(page_id) else {
            return chain;
        };
        visited.insert(start.slug.as_str().to_string());
        let mut current_parent = start.parent.clone();

        while let Some(parent_slug) = current_parent {
            // Cycle guard: stop if we've already seen this slug.
            if !visited.insert(parent_slug.as_str().to_string()) {
                break;
            }
            let Some(parent_node) = self.node_by_slug_unfiltered(&parent_slug) else {
                // Parent not (yet) in graph — stop the walk (defensive).
                break;
            };
            chain.push(parent_slug.clone());
            current_parent = parent_node.parent.clone();
        }
        chain
    }

    /// Resolve the EFFECTIVE access for a page (Spec 3 §3): the page's own access
    /// with the grants of all containment ancestors unioned in. `owner` and
    /// `base_visibility` are taken from the page ITSELF and do NOT inherit — a
    /// `Private` child stays private even under a `Public` ancestor; it only
    /// gains the ancestors' *grants* (Spec 3 §3.3).
    ///
    /// Returns the page's own access unchanged when it has no containment
    /// ancestors (a root page → identical to Spec 2, acceptance criterion 4).
    pub fn effective_access(&self, page_id: &PageId) -> Option<PageAccess> {
        let node = self.get_node(page_id)?;
        let mut effective = node.access.clone();

        for ancestor_slug in self.ancestor_slugs(page_id) {
            if let Some(ancestor) = self.node_by_slug_unfiltered(&ancestor_slug) {
                for grant in &ancestor.access.grants {
                    Self::union_grant(&mut effective.grants, grant.clone());
                }
            }
        }
        Some(effective)
    }

    /// Effective access for a page identified by slug (convenience for callers
    /// that only have a slug). Uses the unfiltered slug lookup.
    pub fn effective_access_by_slug(&self, slug: &Slug) -> Option<PageAccess> {
        let page_id = self.node_by_slug_unfiltered(slug)?.page_id.clone();
        self.effective_access(&page_id)
    }

    /// Merge a grant into `grants`, raising the level if the principal is already
    /// present (so an ancestor Edit grant upgrades a descendant View grant, and
    /// vice-versa the higher level wins). Mirrors the share-page upsert.
    fn union_grant(
        grants: &mut Vec<super::value_objects::Grant>,
        incoming: super::value_objects::Grant,
    ) {
        if let Some(existing) = grants
            .iter_mut()
            .find(|g| g.principal == incoming.principal)
        {
            existing.level = existing.level.max(incoming.level);
        } else {
            grants.push(incoming);
        }
    }

    /// Visibility check for a node that respects containment inheritance
    /// (Spec 3): resolves the node's effective access (own grants ∪ ancestor
    /// grants) and applies the Spec 2 view rule against it.
    fn can_see_node(&self, node: &GraphNode, ctx: &TenantContext) -> bool {
        match self.effective_access(&node.page_id) {
            Some(access) => ctx.can_see_page(&access),
            None => ctx.can_see_page(&node.access),
        }
    }

    /// Would setting `parent` as the container of `child` create a cycle?
    ///
    /// Single-parent + acyclic is the Spec 3 §2 invariant. A cycle forms if
    /// `parent` is `child` itself, or if `child` is already an ancestor of
    /// `parent` (i.e. reachable by walking `parent`'s containment chain upward).
    /// Slugs are used because that is how `parent` is stored on the page.
    pub fn would_create_cycle(&self, child: &Slug, parent: &Slug) -> bool {
        if child == parent {
            return true;
        }
        // Walk parent's ancestor chain; if we encounter `child`, it's a cycle.
        let Some(parent_node) = self.node_by_slug_unfiltered(parent) else {
            // Proposed parent not in graph: can't form a cycle through it yet.
            return false;
        };
        if self
            .ancestor_slugs(&parent_node.page_id)
            .iter()
            .any(|s| s == child)
        {
            return true;
        }
        false
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
            access: PageAccess::from_visibility(&vis),
            visibility: vis,
            page_type: ptype,
            parent: None,
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
                    access: PageAccess::from_visibility(&Visibility::General),
                    page_type: PageType::Index,
                    parent: None,
                },
                GraphNodeData {
                    page_id: pid2.clone(),
                    slug: Slug::new("two").unwrap(),
                    title: "Two".into(),
                    summary: "S2".into(),
                    visibility: Visibility::General,
                    access: PageAccess::from_visibility(&Visibility::General),
                    page_type: PageType::Leaf,
                    parent: None,
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

    // --- Spec 3 containment helpers ---

    fn node_with_parent(slug: &str, parent: Option<&str>) -> GraphNode {
        let mut n = make_node(slug, slug, PageType::Concept, Visibility::General);
        n.parent = parent.map(|p| Slug::new(p).unwrap());
        n
    }

    #[test]
    fn ancestor_walk_returns_chain_root_last() {
        let mut kg = KnowledgeGraph::new();
        kg.add_node(node_with_parent("root", None));
        kg.add_node(node_with_parent("child", Some("root")));
        let gc = node_with_parent("grandchild", Some("child"));
        let gc_id = gc.page_id.clone();
        kg.add_node(gc);

        let chain: Vec<String> = kg
            .ancestor_slugs(&gc_id)
            .iter()
            .map(|s| s.as_str().to_string())
            .collect();
        assert_eq!(chain, vec!["child".to_string(), "root".to_string()]);
    }

    #[test]
    fn ancestor_walk_is_cycle_safe() {
        // Build a malformed cycle a->b->a directly (writes normally prevent this)
        // and ensure the walk terminates rather than looping forever.
        let mut kg = KnowledgeGraph::new();
        kg.add_node(node_with_parent("a", Some("b")));
        let b = node_with_parent("b", Some("a"));
        let b_id = b.page_id.clone();
        kg.add_node(b);
        let chain = kg.ancestor_slugs(&b_id);
        // Terminates; length is bounded by the number of distinct nodes.
        assert!(chain.len() <= 2);
    }

    #[test]
    fn effective_access_unions_ancestor_grants_but_not_base_visibility() {
        use super::super::value_objects::{BaseVisibility, Grant, Level, PageAccess, Principal};
        let mut kg = KnowledgeGraph::new();

        // Parent: Private with a View grant to alice.
        let mut parent = node_with_parent("parent", None);
        parent.access = PageAccess {
            owner: Some("owner@x.com".into()),
            base_visibility: BaseVisibility::Public, // parent is Public…
            grants: vec![Grant {
                principal: Principal::User("alice@x.com".into()),
                level: Level::View,
            }],
        };
        kg.add_node(parent);

        // Child: Private (must STAY private even though parent is Public).
        let mut child = node_with_parent("child", Some("parent"));
        let child_id = child.page_id.clone();
        child.access = PageAccess {
            owner: Some("owner@x.com".into()),
            base_visibility: BaseVisibility::Private,
            grants: vec![],
        };
        kg.add_node(child);

        let eff = kg.effective_access(&child_id).unwrap();
        // base_visibility does NOT inherit: child stays Private.
        assert_eq!(eff.base_visibility, BaseVisibility::Private);
        // The ancestor's grant IS inherited.
        assert!(eff.grants.iter().any(|g| matches!(
            &g.principal,
            Principal::User(u) if u == "alice@x.com"
        )));
    }

    #[test]
    fn would_create_cycle_detects_self_and_ancestor() {
        let mut kg = KnowledgeGraph::new();
        kg.add_node(node_with_parent("a", None));
        kg.add_node(node_with_parent("b", Some("a")));
        kg.add_node(node_with_parent("c", Some("b")));

        // self-parent
        assert!(kg.would_create_cycle(&Slug::new("a").unwrap(), &Slug::new("a").unwrap()));
        // making `a` a child of `c` closes the loop a->c->b->a
        assert!(kg.would_create_cycle(&Slug::new("a").unwrap(), &Slug::new("c").unwrap()));
        // a valid new leaf under c is fine
        assert!(!kg.would_create_cycle(&Slug::new("d").unwrap(), &Slug::new("c").unwrap()));
    }

    #[test]
    fn root_effective_access_equals_own_access() {
        let mut kg = KnowledgeGraph::new();
        let n = node_with_parent("solo", None);
        let id = n.page_id.clone();
        let own = n.access.clone();
        kg.add_node(n);
        assert_eq!(kg.effective_access(&id).unwrap(), own);
    }
}

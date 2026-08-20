use std::sync::Arc;
use tokio::sync::RwLock;

use super::graph::{GraphNode, KnowledgeGraph, NeighborInfo};
use super::lint::{LintIssue, lint_page};
use super::page::{Page, ReadLevel};
use super::tenant::TenantContext;
use super::value_objects::{EdgeKind, PageType, Section, Slug, Visibility};
use crate::error::MindPalaceError;
use crate::ports::changelog::{ChangeAction, ChangelogEntry, ChangelogStore};
use crate::ports::embedding::EmbeddingPort;
use crate::ports::graph::{GraphEdgeData, GraphNodeData, GraphStore};
use crate::ports::page_store::PageStore;
use crate::ports::vector_search::{EmbeddingMetadata, SearchResult, VectorSearchPort};

pub struct CreatePageInput {
    pub title: String,
    pub slug: Slug,
    pub summary: String,
    pub sections: Vec<Section>,
    pub page_type: PageType,
    pub visibility: Visibility,
    pub links: Vec<Slug>,
}

pub struct UpdatePageInput {
    pub title: Option<String>,
    pub summary: Option<String>,
    pub sections: Option<Vec<Section>>,
    pub links: Option<Vec<Slug>>,
}

#[derive(Debug, Clone)]
pub enum PageResponse {
    Summary {
        title: String,
        slug: Slug,
        summary: String,
        page_type: PageType,
    },
    Section {
        heading: String,
        content: String,
    },
    Full(Page),
}

pub struct WikiService {
    page_store: Arc<dyn PageStore>,
    vector_search: Arc<dyn VectorSearchPort>,
    embedding: Arc<dyn EmbeddingPort>,
    graph_store: Arc<dyn GraphStore>,
    graph: Arc<RwLock<KnowledgeGraph>>,
    changelog: Option<Arc<dyn ChangelogStore>>,
}

impl WikiService {
    pub fn new(
        page_store: Arc<dyn PageStore>,
        vector_search: Arc<dyn VectorSearchPort>,
        embedding: Arc<dyn EmbeddingPort>,
        graph_store: Arc<dyn GraphStore>,
        graph: Arc<RwLock<KnowledgeGraph>>,
    ) -> Self {
        Self {
            page_store,
            vector_search,
            embedding,
            graph_store,
            graph,
            changelog: None,
        }
    }

    /// Attach a changelog store for recording mutations.
    pub fn with_changelog(mut self, store: Arc<dyn ChangelogStore>) -> Self {
        self.changelog = Some(store);
        self
    }

    pub async fn create_page(
        &self,
        input: CreatePageInput,
        ctx: &TenantContext,
    ) -> Result<(Page, Vec<LintIssue>), MindPalaceError> {
        let mut page = Page::new(
            input.title,
            input.slug,
            input.summary,
            input.sections,
            input.page_type,
            input.visibility,
        )
        .map_err(|e| MindPalaceError::Validation(e.to_string()))?;
        page.links = input.links;

        let issues = {
            let g = self.graph.read().await;
            lint_page(&page, Some(&g))
        };

        self.page_store.save_page(&page).await?;

        let text = page.full_content();
        let embedding = self.embedding.embed_text(&text).await?;
        let meta = EmbeddingMetadata {
            page_id: page.id.clone(),
            slug: page.slug.clone(),
            title: page.title.clone(),
            visibility: page.visibility.clone(),
        };
        self.vector_search
            .upsert_embedding(&meta, &embedding)
            .await?;

        let node_data = GraphNodeData {
            page_id: page.id.clone(),
            slug: page.slug.clone(),
            title: page.title.clone(),
            summary: page.summary.clone(),
            visibility: page.visibility.clone(),
            page_type: page.page_type.clone(),
        };
        self.graph_store.save_node(&node_data).await?;

        {
            let mut g = self.graph.write().await;
            g.add_node(GraphNode {
                page_id: page.id.clone(),
                slug: page.slug.clone(),
                title: page.title.clone(),
                summary: page.summary.clone(),
                visibility: page.visibility.clone(),
                page_type: page.page_type.clone(),
            });
            for link_slug in &page.links {
                // Resolve target by looking through graph nodes
                let target_id = self.find_page_id_by_slug(&g, link_slug);
                if let Some(tid) = target_id {
                    let edge = GraphEdgeData {
                        source: page.id.clone(),
                        target: tid.clone(),
                        kind: EdgeKind::Related,
                    };
                    self.graph_store.save_edge(&edge).await?;
                    g.add_edge(&page.id, &tid, EdgeKind::Related);
                }
            }
        }

        let _ = ctx; // used for future access control

        if let Some(ref changelog) = self.changelog {
            let entry = ChangelogEntry {
                timestamp: chrono::Utc::now(),
                slug: page.slug.clone(),
                page_id: page.id.clone(),
                action: ChangeAction::Created,
                agent_id: None,
                summary: Some(page.summary.clone()),
            };
            changelog.append(&entry).await?;
        }

        Ok((page, issues))
    }

    pub async fn update_page(
        &self,
        slug: &Slug,
        input: UpdatePageInput,
        ctx: &TenantContext,
    ) -> Result<(Page, Vec<LintIssue>), MindPalaceError> {
        let mut page = self.page_store.get_page_by_slug(slug, ctx).await?;

        if let Some(title) = input.title {
            page.title = title;
        }
        if let Some(summary) = input.summary {
            page.summary = summary;
        }
        if let Some(sections) = input.sections {
            page.sections = sections;
            page.toc = super::value_objects::TableOfContents::from_sections(&page.sections);
        }
        if let Some(links) = input.links {
            page.links = links;
        }
        page.version += 1;
        page.updated_at = chrono::Utc::now();

        let issues = {
            let g = self.graph.read().await;
            lint_page(&page, Some(&g))
        };

        self.page_store.save_page(&page).await?;

        let text = page.full_content();
        let embedding = self.embedding.embed_text(&text).await?;
        let meta = EmbeddingMetadata {
            page_id: page.id.clone(),
            slug: page.slug.clone(),
            title: page.title.clone(),
            visibility: page.visibility.clone(),
        };
        self.vector_search
            .upsert_embedding(&meta, &embedding)
            .await?;

        // Update graph node
        let node_data = GraphNodeData {
            page_id: page.id.clone(),
            slug: page.slug.clone(),
            title: page.title.clone(),
            summary: page.summary.clone(),
            visibility: page.visibility.clone(),
            page_type: page.page_type.clone(),
        };
        self.graph_store.save_node(&node_data).await?;

        if let Some(ref changelog) = self.changelog {
            let entry = ChangelogEntry {
                timestamp: chrono::Utc::now(),
                slug: page.slug.clone(),
                page_id: page.id.clone(),
                action: ChangeAction::Updated,
                agent_id: None,
                summary: Some(page.summary.clone()),
            };
            changelog.append(&entry).await?;
        }

        Ok((page, issues))
    }

    pub async fn read_page(
        &self,
        slug: &Slug,
        level: ReadLevel,
        ctx: &TenantContext,
    ) -> Result<PageResponse, MindPalaceError> {
        let page = self.page_store.get_page_by_slug(slug, ctx).await?;
        match level {
            ReadLevel::Summary => Ok(PageResponse::Summary {
                title: page.title,
                slug: page.slug,
                summary: page.summary,
                page_type: page.page_type,
            }),
            ReadLevel::Section(heading) => {
                let section = page
                    .section_by_heading(&heading)
                    .ok_or_else(|| MindPalaceError::PageNotFound(format!("section: {heading}")))?;
                Ok(PageResponse::Section {
                    heading: section.heading.clone(),
                    content: section.content.clone(),
                })
            }
            ReadLevel::Full => Ok(PageResponse::Full(page)),
        }
    }

    pub async fn search(
        &self,
        query: &str,
        ctx: &TenantContext,
        limit: usize,
    ) -> Result<Vec<SearchResult>, MindPalaceError> {
        let embedding = self.embedding.embed_text(query).await?;
        self.vector_search.search(&embedding, limit, ctx).await
    }

    pub async fn traverse(
        &self,
        slug: &Slug,
        depth: usize,
        ctx: &TenantContext,
    ) -> Result<Vec<NeighborInfo>, MindPalaceError> {
        let page = self.page_store.get_page_by_slug(slug, ctx).await?;
        let g = self.graph.read().await;
        Ok(g.get_subtree(&page.id, depth, ctx))
    }

    pub async fn list_pages(
        &self,
        filter: &crate::ports::page_store::PageFilter,
        ctx: &TenantContext,
    ) -> Result<Vec<Page>, MindPalaceError> {
        // Use in-memory graph for listing (avoids S3 GetObject per page).
        // Returns lightweight Page stubs with metadata only.
        let g = self.graph.read().await;
        let limit = filter.limit.unwrap_or(50);
        let pages: Vec<Page> = g
            .all_nodes(ctx)
            .into_iter()
            .filter(|node| {
                filter
                    .page_type
                    .as_ref()
                    .is_none_or(|pt| pt == &node.page_type)
            })
            .take(limit)
            .map(|node| Page {
                id: node.page_id.clone(),
                slug: node.slug.clone(),
                title: node.title.clone(),
                summary: node.summary.clone(),
                toc: super::value_objects::TableOfContents { entries: vec![] },
                sections: vec![],
                page_type: node.page_type.clone(),
                visibility: node.visibility.clone(),
                confidence: super::value_objects::Confidence::default(),
                version: 0,
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
                links: vec![],
            })
            .collect();
        Ok(pages)
    }

    pub async fn delete_page(
        &self,
        slug: &Slug,
        ctx: &TenantContext,
    ) -> Result<(), MindPalaceError> {
        let page = self.page_store.get_page_by_slug(slug, ctx).await?;
        self.page_store.delete_page(&page.id).await?;
        self.vector_search.delete_embedding(&page.id).await?;
        self.graph_store.delete_node(&page.id).await?;
        let mut g = self.graph.write().await;
        g.remove_node(&page.id);

        if let Some(ref changelog) = self.changelog {
            let entry = ChangelogEntry {
                timestamp: chrono::Utc::now(),
                slug: page.slug.clone(),
                page_id: page.id.clone(),
                action: ChangeAction::Deleted,
                agent_id: None,
                summary: None,
            };
            changelog.append(&entry).await?;
        }

        Ok(())
    }

    pub async fn archive_page(
        &self,
        slug: &Slug,
        ctx: &TenantContext,
    ) -> Result<(), MindPalaceError> {
        let mut page = self.page_store.get_page_by_slug(slug, ctx).await?;
        page.visibility = Visibility::Archived;
        page.version += 1;
        page.updated_at = chrono::Utc::now();
        self.page_store.save_page(&page).await?;

        // Remove from vector search (won't appear in semantic search)
        self.vector_search.delete_embedding(&page.id).await?;

        // Update graph node visibility so it's filtered from traversal/list
        let node_data = crate::ports::graph::GraphNodeData {
            page_id: page.id.clone(),
            slug: page.slug.clone(),
            title: page.title.clone(),
            summary: page.summary.clone(),
            visibility: Visibility::Archived,
            page_type: page.page_type.clone(),
        };
        self.graph_store.save_node(&node_data).await?;

        // Update in-memory graph
        {
            let mut g = self.graph.write().await;
            if let Some(node) = g.get_node_mut(&page.id) {
                node.visibility = Visibility::Archived;
            }
        }

        if let Some(ref changelog) = self.changelog {
            let entry = ChangelogEntry {
                timestamp: chrono::Utc::now(),
                slug: page.slug.clone(),
                page_id: page.id.clone(),
                action: ChangeAction::Updated,
                agent_id: None,
                summary: Some("Archived".to_string()),
            };
            changelog.append(&entry).await?;
        }

        Ok(())
    }

    pub async fn unarchive_page(&self, slug: &Slug) -> Result<(), MindPalaceError> {
        // Must bypass normal visibility check since archived pages are hidden
        // by can_see. Use get_page_by_slug_unfiltered which reads without
        // visibility filtering.
        let mut page = self.page_store.get_page_by_slug_unfiltered(slug).await?;
        page.visibility = Visibility::General;
        page.version += 1;
        page.updated_at = chrono::Utc::now();
        self.page_store.save_page(&page).await?;

        // Re-index in vector search
        let text = page.full_content();
        let embedding = self.embedding.embed_text(&text).await?;
        let meta = EmbeddingMetadata {
            page_id: page.id.clone(),
            slug: page.slug.clone(),
            title: page.title.clone(),
            visibility: page.visibility.clone(),
        };
        self.vector_search
            .upsert_embedding(&meta, &embedding)
            .await?;

        // Update graph node
        let node_data = crate::ports::graph::GraphNodeData {
            page_id: page.id.clone(),
            slug: page.slug.clone(),
            title: page.title.clone(),
            summary: page.summary.clone(),
            visibility: Visibility::General,
            page_type: page.page_type.clone(),
        };
        self.graph_store.save_node(&node_data).await?;

        // Update in-memory graph
        {
            let mut g = self.graph.write().await;
            if let Some(node) = g.get_node_mut(&page.id) {
                node.visibility = Visibility::General;
            }
        }

        if let Some(ref changelog) = self.changelog {
            let entry = ChangelogEntry {
                timestamp: chrono::Utc::now(),
                slug: page.slug.clone(),
                page_id: page.id.clone(),
                action: ChangeAction::Updated,
                agent_id: None,
                summary: Some("Unarchived".to_string()),
            };
            changelog.append(&entry).await?;
        }

        Ok(())
    }

    fn find_page_id_by_slug(
        &self,
        graph: &KnowledgeGraph,
        slug: &Slug,
    ) -> Option<super::value_objects::PageId> {
        let ctx = TenantContext::global();
        graph.find_by_slug(slug, &ctx).map(|n| n.page_id.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::value_objects::PageId;
    use crate::ports::graph::GraphData;
    use crate::ports::page_store::PageFilter;
    use async_trait::async_trait;
    use std::sync::Mutex;

    // --- Mock PageStore ---
    struct MockPageStore {
        pages: Mutex<Vec<Page>>,
    }

    impl MockPageStore {
        fn new() -> Self {
            Self {
                pages: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl PageStore for MockPageStore {
        async fn get_page(
            &self,
            id: &PageId,
            _ctx: &TenantContext,
        ) -> Result<Page, MindPalaceError> {
            let pages = self.pages.lock().unwrap();
            pages
                .iter()
                .find(|p| &p.id == id)
                .cloned()
                .ok_or_else(|| MindPalaceError::PageNotFound(format!("{:?}", id)))
        }

        async fn get_page_by_slug(
            &self,
            slug: &Slug,
            _ctx: &TenantContext,
        ) -> Result<Page, MindPalaceError> {
            let pages = self.pages.lock().unwrap();
            pages
                .iter()
                .find(|p| &p.slug == slug)
                .cloned()
                .ok_or_else(|| MindPalaceError::PageNotFound(slug.as_str().into()))
        }

        async fn get_page_by_slug_unfiltered(&self, slug: &Slug) -> Result<Page, MindPalaceError> {
            let pages = self.pages.lock().unwrap();
            pages
                .iter()
                .find(|p| &p.slug == slug)
                .cloned()
                .ok_or_else(|| MindPalaceError::PageNotFound(slug.as_str().into()))
        }

        async fn save_page(&self, page: &Page) -> Result<(), MindPalaceError> {
            let mut pages = self.pages.lock().unwrap();
            pages.retain(|p| p.id != page.id);
            pages.push(page.clone());
            Ok(())
        }

        async fn delete_page(&self, id: &PageId) -> Result<(), MindPalaceError> {
            let mut pages = self.pages.lock().unwrap();
            pages.retain(|p| &p.id != id);
            Ok(())
        }

        async fn list_pages(
            &self,
            _filter: &PageFilter,
            _ctx: &TenantContext,
        ) -> Result<Vec<Page>, MindPalaceError> {
            Ok(self.pages.lock().unwrap().clone())
        }
    }

    // --- Mock EmbeddingPort ---
    struct MockEmbedding;

    #[async_trait]
    impl EmbeddingPort for MockEmbedding {
        async fn embed_text(&self, _text: &str) -> Result<Vec<f64>, MindPalaceError> {
            Ok(vec![0.1, 0.2, 0.3])
        }
        async fn embed_texts(&self, texts: &[&str]) -> Result<Vec<Vec<f64>>, MindPalaceError> {
            Ok(texts.iter().map(|_| vec![0.1, 0.2, 0.3]).collect())
        }
    }

    // --- Mock VectorSearchPort ---
    struct MockVectorSearch;

    #[async_trait]
    impl VectorSearchPort for MockVectorSearch {
        async fn search(
            &self,
            _embedding: &[f64],
            _limit: usize,
            _ctx: &TenantContext,
        ) -> Result<Vec<SearchResult>, MindPalaceError> {
            Ok(vec![])
        }
        async fn upsert_embedding(
            &self,
            _meta: &EmbeddingMetadata,
            _embedding: &[f64],
        ) -> Result<(), MindPalaceError> {
            Ok(())
        }
        async fn delete_embedding(&self, _page_id: &PageId) -> Result<(), MindPalaceError> {
            Ok(())
        }
    }

    // --- Mock GraphStore ---
    struct MockGraphStore;

    #[async_trait]
    impl GraphStore for MockGraphStore {
        async fn load_graph(&self) -> Result<GraphData, MindPalaceError> {
            Ok(GraphData {
                nodes: vec![],
                edges: vec![],
            })
        }
        async fn save_node(&self, _node: &GraphNodeData) -> Result<(), MindPalaceError> {
            Ok(())
        }
        async fn save_edge(&self, _edge: &GraphEdgeData) -> Result<(), MindPalaceError> {
            Ok(())
        }
        async fn delete_node(&self, _id: &PageId) -> Result<(), MindPalaceError> {
            Ok(())
        }
        async fn delete_edge(
            &self,
            _source: &PageId,
            _target: &PageId,
        ) -> Result<(), MindPalaceError> {
            Ok(())
        }
    }

    fn make_service() -> WikiService {
        WikiService::new(
            Arc::new(MockPageStore::new()),
            Arc::new(MockVectorSearch),
            Arc::new(MockEmbedding),
            Arc::new(MockGraphStore),
            Arc::new(RwLock::new(KnowledgeGraph::new())),
        )
    }

    fn sample_input() -> CreatePageInput {
        CreatePageInput {
            title: "Rust Basics".into(),
            slug: Slug::new("rust-basics").unwrap(),
            summary: "An intro to Rust".into(),
            sections: vec![Section {
                heading: "Overview".into(),
                content: "Rust is a systems language.".into(),
            }],
            page_type: PageType::Concept,
            visibility: Visibility::General,
            links: vec![],
        }
    }

    #[tokio::test]
    async fn create_and_read_page() {
        let svc = make_service();
        let ctx = TenantContext::global();

        let (page, issues) = svc.create_page(sample_input(), &ctx).await.unwrap();
        assert_eq!(page.title, "Rust Basics");
        assert!(
            !issues
                .iter()
                .any(|i| i.severity == crate::domain::lint::Severity::Error)
        );

        let resp = svc
            .read_page(&Slug::new("rust-basics").unwrap(), ReadLevel::Summary, &ctx)
            .await
            .unwrap();
        match resp {
            PageResponse::Summary { title, .. } => assert_eq!(title, "Rust Basics"),
            _ => panic!("expected Summary"),
        }
    }

    #[tokio::test]
    async fn update_page_changes_title() {
        let svc = make_service();
        let ctx = TenantContext::global();

        svc.create_page(sample_input(), &ctx).await.unwrap();

        let (updated, _) = svc
            .update_page(
                &Slug::new("rust-basics").unwrap(),
                UpdatePageInput {
                    title: Some("Advanced Rust".into()),
                    summary: None,
                    sections: None,
                    links: None,
                },
                &ctx,
            )
            .await
            .unwrap();
        assert_eq!(updated.title, "Advanced Rust");
        assert_eq!(updated.version, 2);
    }

    #[tokio::test]
    async fn delete_page_removes_from_store() {
        let svc = make_service();
        let ctx = TenantContext::global();

        svc.create_page(sample_input(), &ctx).await.unwrap();
        svc.delete_page(&Slug::new("rust-basics").unwrap(), &ctx)
            .await
            .unwrap();

        let result = svc
            .read_page(&Slug::new("rust-basics").unwrap(), ReadLevel::Full, &ctx)
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn search_returns_results() {
        let svc = make_service();
        let ctx = TenantContext::global();
        let results = svc.search("rust", &ctx, 10).await.unwrap();
        assert!(results.is_empty()); // mock returns empty
    }
}

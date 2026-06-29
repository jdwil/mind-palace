use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use mind_palace_core::domain::graph::KnowledgeGraph;
use mind_palace_core::domain::page::{Page, ReadLevel};
use mind_palace_core::domain::service::{CreatePageInput, PageResponse, WikiService};
use mind_palace_core::domain::tenant::TenantContext;
use mind_palace_core::domain::value_objects::*;
use mind_palace_core::error::MindPalaceError;
use mind_palace_core::ports::embedding::EmbeddingPort;
use mind_palace_core::ports::graph::{GraphData, GraphEdgeData, GraphNodeData, GraphStore};
use mind_palace_core::ports::page_store::{PageFilter, PageStore};
use mind_palace_core::ports::vector_search::{EmbeddingMetadata, SearchResult, VectorSearchPort};
use tokio::sync::RwLock;

// --- Mock implementations ---

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
    async fn get_page(&self, id: &PageId, ctx: &TenantContext) -> Result<Page, MindPalaceError> {
        let pages = self.pages.lock().unwrap();
        pages
            .iter()
            .find(|p| &p.id == id && ctx.can_see(&p.visibility))
            .cloned()
            .ok_or_else(|| MindPalaceError::PageNotFound(format!("{:?}", id)))
    }

    async fn get_page_by_slug(
        &self,
        slug: &Slug,
        ctx: &TenantContext,
    ) -> Result<Page, MindPalaceError> {
        let pages = self.pages.lock().unwrap();
        pages
            .iter()
            .find(|p| &p.slug == slug && ctx.can_see(&p.visibility))
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
        filter: &PageFilter,
        ctx: &TenantContext,
    ) -> Result<Vec<Page>, MindPalaceError> {
        let pages = self.pages.lock().unwrap();
        let result: Vec<Page> = pages
            .iter()
            .filter(|p| ctx.can_see(&p.visibility))
            .filter(|p| {
                filter
                    .page_type
                    .as_ref()
                    .is_none_or(|pt| pt == &p.page_type)
            })
            .cloned()
            .collect();
        Ok(result)
    }
}

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
    async fn delete_edge(&self, _source: &PageId, _target: &PageId) -> Result<(), MindPalaceError> {
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

// --- Integration Tests ---

#[tokio::test]
async fn create_index_concept_leaf_and_verify_graph_connectivity() {
    let svc = make_service();
    let ctx = TenantContext::global();

    // Create Index page first
    svc.create_page(
        CreatePageInput {
            title: "Knowledge Index".into(),
            slug: Slug::new("knowledge-index").unwrap(),
            summary: "Root index for all knowledge".into(),
            sections: vec![Section {
                heading: "Overview".into(),
                content: "Top-level index.".into(),
            }],
            page_type: PageType::Index,
            visibility: Visibility::General,
            links: vec![],
        },
        &ctx,
    )
    .await
    .unwrap();

    // Create Concept page linked to Index (link resolves because target is Index type)
    svc.create_page(
        CreatePageInput {
            title: "Rust Language".into(),
            slug: Slug::new("rust-language").unwrap(),
            summary: "Concept page about Rust".into(),
            sections: vec![Section {
                heading: "Basics".into(),
                content: "Rust is a systems programming language.".into(),
            }],
            page_type: PageType::Index, // Make it Index so leaf can link to it
            visibility: Visibility::General,
            links: vec![Slug::new("knowledge-index").unwrap()],
        },
        &ctx,
    )
    .await
    .unwrap();

    // Create Leaf page linked to Concept (works because rust-language is Index type)
    svc.create_page(
        CreatePageInput {
            title: "Ownership Rules".into(),
            slug: Slug::new("ownership-rules").unwrap(),
            summary: "Leaf page about ownership".into(),
            sections: vec![Section {
                heading: "Rules".into(),
                content: "Each value has one owner. Detailed explanation of Rust ownership model."
                    .into(),
            }],
            page_type: PageType::Leaf,
            visibility: Visibility::General,
            links: vec![Slug::new("rust-language").unwrap()],
        },
        &ctx,
    )
    .await
    .unwrap();

    // Verify graph connectivity via traverse (uses outgoing edges)
    // Concept -> knowledge-index (Related edge)
    let concept_neighbors = svc
        .traverse(&Slug::new("rust-language").unwrap(), 1, &ctx)
        .await
        .unwrap();
    assert!(
        concept_neighbors
            .iter()
            .any(|n| n.slug.as_str() == "knowledge-index"),
        "Concept should link to Index"
    );

    // Leaf -> rust-language (Related edge)
    let leaf_neighbors = svc
        .traverse(&Slug::new("ownership-rules").unwrap(), 1, &ctx)
        .await
        .unwrap();
    assert!(
        leaf_neighbors
            .iter()
            .any(|n| n.slug.as_str() == "rust-language"),
        "Leaf should link to Concept"
    );

    // Verify 3 nodes exist by listing all pages
    let all_pages = svc.list_pages(&PageFilter::default(), &ctx).await.unwrap();
    assert_eq!(all_pages.len(), 3);
}

#[tokio::test]
async fn read_at_different_levels_summary_shorter_than_full() {
    let svc = make_service();
    let ctx = TenantContext::global();

    svc.create_page(
        CreatePageInput {
            title: "Detailed Page".into(),
            slug: Slug::new("detailed-page").unwrap(),
            summary: "Short summary".into(),
            sections: vec![
                Section { heading: "Introduction".into(), content: "This is a lengthy introduction section with lots of content to ensure the full version is much longer than the summary.".into() },
                Section { heading: "Details".into(), content: "Even more detailed content here that adds to the total length of the full page read.".into() },
            ],
            page_type: PageType::Concept,
            visibility: Visibility::General,
            links: vec![],
        },
        &ctx,
    )
    .await
    .unwrap();

    let slug = Slug::new("detailed-page").unwrap();

    // Read summary
    let summary_resp = svc
        .read_page(&slug, ReadLevel::Summary, &ctx)
        .await
        .unwrap();
    let summary_len = match &summary_resp {
        PageResponse::Summary { summary, .. } => summary.len(),
        _ => panic!("expected Summary response"),
    };

    // Read full
    let full_resp = svc.read_page(&slug, ReadLevel::Full, &ctx).await.unwrap();
    let full_len = match &full_resp {
        PageResponse::Full(page) => page.full_content().len(),
        _ => panic!("expected Full response"),
    };

    assert!(
        full_len > summary_len,
        "Full content ({full_len}) should be longer than summary ({summary_len})"
    );
}

#[tokio::test]
async fn tenant_isolation_prevents_cross_tenant_access() {
    let svc = make_service();
    let tenant_a = TenantContext::leaf(TenantId::new("tenant-a"));
    let tenant_b = TenantContext::leaf(TenantId::new("tenant-b"));

    // Create a page scoped to tenant-a
    svc.create_page(
        CreatePageInput {
            title: "Secret A Page".into(),
            slug: Slug::new("secret-a").unwrap(),
            summary: "Only for tenant A".into(),
            sections: vec![Section {
                heading: "Content".into(),
                content: "Private data.".into(),
            }],
            page_type: PageType::Leaf,
            visibility: Visibility::Tenant(TenantId::new("tenant-a")),
            links: vec![],
        },
        &tenant_a,
    )
    .await
    .unwrap();

    // Tenant A can read it
    let result_a = svc
        .read_page(
            &Slug::new("secret-a").unwrap(),
            ReadLevel::Summary,
            &tenant_a,
        )
        .await;
    assert!(result_a.is_ok(), "Tenant A should see its own page");

    // Tenant B cannot read it
    let result_b = svc
        .read_page(
            &Slug::new("secret-a").unwrap(),
            ReadLevel::Summary,
            &tenant_b,
        )
        .await;
    assert!(result_b.is_err(), "Tenant B should NOT see tenant A's page");

    // Global context can see it
    let global = TenantContext::global();
    let result_global = svc
        .read_page(&Slug::new("secret-a").unwrap(), ReadLevel::Summary, &global)
        .await;
    assert!(result_global.is_ok(), "Global context should see all pages");
}

#[tokio::test]
async fn tenant_isolation_list_pages_filters_correctly() {
    let svc = make_service();
    let tenant_a = TenantContext::leaf(TenantId::new("tenant-a"));
    let tenant_b = TenantContext::leaf(TenantId::new("tenant-b"));

    // Create pages for different tenants
    svc.create_page(
        CreatePageInput {
            title: "A's Page".into(),
            slug: Slug::new("a-page").unwrap(),
            summary: "Belongs to A".into(),
            sections: vec![Section {
                heading: "Info".into(),
                content: "Data.".into(),
            }],
            page_type: PageType::Leaf,
            visibility: Visibility::Tenant(TenantId::new("tenant-a")),
            links: vec![],
        },
        &tenant_a,
    )
    .await
    .unwrap();

    svc.create_page(
        CreatePageInput {
            title: "General Page".into(),
            slug: Slug::new("general-page").unwrap(),
            summary: "Visible to all".into(),
            sections: vec![Section {
                heading: "Info".into(),
                content: "Public data.".into(),
            }],
            page_type: PageType::Concept,
            visibility: Visibility::General,
            links: vec![],
        },
        &tenant_a,
    )
    .await
    .unwrap();

    let filter = PageFilter::default();

    // Tenant B sees only General pages
    let b_pages = svc.list_pages(&filter, &tenant_b).await.unwrap();
    assert_eq!(b_pages.len(), 1);
    assert_eq!(b_pages[0].slug.as_str(), "general-page");

    // Tenant A sees both
    let a_pages = svc.list_pages(&filter, &tenant_a).await.unwrap();
    assert_eq!(a_pages.len(), 2);
}

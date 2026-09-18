//! Spec 4 acceptance tests: secrets proxy. Reference-based secret access —
//! service-layer validation rejects raw values, wiki_read never returns a
//! value, get_secret is access-gated + audited, and a Public page carrying a
//! secret ref fails lint.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use mind_palace_core::domain::graph::KnowledgeGraph;
use mind_palace_core::domain::lint::{LintCode, Severity, lint_page};
use mind_palace_core::domain::page::{Page, ReadLevel};
use mind_palace_core::domain::service::{CreatePageInput, PageResponse, WikiService};
use mind_palace_core::domain::tenant::TenantContext;
use mind_palace_core::domain::value_objects::*;
use mind_palace_core::error::MindPalaceError;
use mind_palace_core::ports::embedding::EmbeddingPort;
use mind_palace_core::ports::graph::{GraphData, GraphEdgeData, GraphNodeData, GraphStore};
use mind_palace_core::ports::page_store::{PageFilter, PageStore};
use mind_palace_core::ports::secrets::{SecretError, SecretValue, SecretsResolver};
use mind_palace_core::ports::vector_search::{EmbeddingMetadata, SearchResult, VectorSearchPort};
use tokio::sync::RwLock;

// --- Mocks (self-contained) ---

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
    async fn get_page(&self, id: &PageId, _c: &TenantContext) -> Result<Page, MindPalaceError> {
        self.pages
            .lock()
            .unwrap()
            .iter()
            .find(|p| &p.id == id)
            .cloned()
            .ok_or_else(|| MindPalaceError::PageNotFound(format!("{id:?}")))
    }
    async fn get_page_by_slug(
        &self,
        slug: &Slug,
        _c: &TenantContext,
    ) -> Result<Page, MindPalaceError> {
        self.pages
            .lock()
            .unwrap()
            .iter()
            .find(|p| &p.slug == slug)
            .cloned()
            .ok_or_else(|| MindPalaceError::PageNotFound(slug.as_str().into()))
    }
    async fn get_page_by_slug_unfiltered(&self, slug: &Slug) -> Result<Page, MindPalaceError> {
        self.pages
            .lock()
            .unwrap()
            .iter()
            .find(|p| &p.slug == slug)
            .cloned()
            .ok_or_else(|| MindPalaceError::PageNotFound(slug.as_str().into()))
    }
    async fn save_page(&self, page: &Page) -> Result<(), MindPalaceError> {
        let mut p = self.pages.lock().unwrap();
        p.retain(|x| x.id != page.id);
        p.push(page.clone());
        Ok(())
    }
    async fn delete_page(&self, id: &PageId) -> Result<(), MindPalaceError> {
        self.pages.lock().unwrap().retain(|p| &p.id != id);
        Ok(())
    }
    async fn list_pages(
        &self,
        _f: &PageFilter,
        _c: &TenantContext,
    ) -> Result<Vec<Page>, MindPalaceError> {
        Ok(self.pages.lock().unwrap().clone())
    }
}

struct MockEmbedding;
#[async_trait]
impl EmbeddingPort for MockEmbedding {
    async fn embed_text(&self, _t: &str) -> Result<Vec<f64>, MindPalaceError> {
        Ok(vec![0.1, 0.2, 0.3])
    }
    async fn embed_texts(&self, t: &[&str]) -> Result<Vec<Vec<f64>>, MindPalaceError> {
        Ok(t.iter().map(|_| vec![0.1, 0.2, 0.3]).collect())
    }
}

struct MockVectorSearch;
#[async_trait]
impl VectorSearchPort for MockVectorSearch {
    async fn search(
        &self,
        _e: &[f64],
        _l: usize,
        _c: &TenantContext,
    ) -> Result<Vec<SearchResult>, MindPalaceError> {
        Ok(vec![])
    }
    async fn upsert_embedding(
        &self,
        _m: &EmbeddingMetadata,
        _e: &[f64],
    ) -> Result<(), MindPalaceError> {
        Ok(())
    }
    async fn delete_embedding(&self, _id: &PageId) -> Result<(), MindPalaceError> {
        Ok(())
    }
}

#[derive(Default)]
struct MockGraphStore {
    nodes: Mutex<Vec<GraphNodeData>>,
}
#[async_trait]
impl GraphStore for MockGraphStore {
    async fn load_graph(&self) -> Result<GraphData, MindPalaceError> {
        Ok(GraphData {
            nodes: self.nodes.lock().unwrap().clone(),
            edges: vec![],
        })
    }
    async fn save_node(&self, node: &GraphNodeData) -> Result<(), MindPalaceError> {
        let mut n = self.nodes.lock().unwrap();
        n.retain(|x| x.page_id != node.page_id);
        n.push(node.clone());
        Ok(())
    }
    async fn save_edge(&self, _e: &GraphEdgeData) -> Result<(), MindPalaceError> {
        Ok(())
    }
    async fn delete_node(&self, id: &PageId) -> Result<(), MindPalaceError> {
        self.nodes.lock().unwrap().retain(|x| &x.page_id != id);
        Ok(())
    }
    async fn delete_edge(&self, _s: &PageId, _t: &PageId) -> Result<(), MindPalaceError> {
        Ok(())
    }
}

/// A fake secrets backend: accepts references starting with "ref://" and
/// resolves them to a fixed value. Anything else is an invalid reference (i.e.
/// looks like a raw secret). Records resolve() calls so audit/gating can be
/// asserted.
#[derive(Default)]
struct FakeResolver {
    resolved: Mutex<Vec<String>>,
}
#[async_trait]
impl SecretsResolver for FakeResolver {
    fn validate_reference(&self, reference: &str) -> Result<(), SecretError> {
        if reference.starts_with("ref://") {
            Ok(())
        } else {
            Err(SecretError::InvalidReference(format!(
                "not a ref:// locator: {reference}"
            )))
        }
    }
    async fn resolve(&self, reference: &str) -> Result<SecretValue, SecretError> {
        self.resolved.lock().unwrap().push(reference.to_string());
        Ok(SecretValue::new(format!("VALUE_FOR::{reference}")))
    }
}

fn service_with_resolver(resolver: Arc<FakeResolver>) -> WikiService {
    WikiService::new(
        Arc::new(MockPageStore::new()),
        Arc::new(MockVectorSearch),
        Arc::new(MockEmbedding),
        Arc::new(MockGraphStore::default()),
        Arc::new(RwLock::new(KnowledgeGraph::new())),
    )
    .with_secrets_resolver(resolver)
}

fn service_no_resolver() -> WikiService {
    WikiService::new(
        Arc::new(MockPageStore::new()),
        Arc::new(MockVectorSearch),
        Arc::new(MockEmbedding),
        Arc::new(MockGraphStore::default()),
        Arc::new(RwLock::new(KnowledgeGraph::new())),
    )
}

fn private_page_with_secret(slug: &str, refs: Vec<SecretRef>) -> CreatePageInput {
    CreatePageInput {
        title: slug.into(),
        slug: Slug::new(slug).unwrap(),
        summary: "s".into(),
        sections: vec![Section {
            heading: "H".into(),
            content: "C".into(),
        }],
        page_type: PageType::Leaf,
        visibility: Visibility::General,
        links: vec![],
        base_visibility: Some(BaseVisibility::Private),
        parent: None,
        secret_refs: refs,
    }
}

// --- Criterion 1: valid ref accepted; raw secret rejected at service layer ---

#[tokio::test]
async fn valid_reference_is_accepted_and_raw_secret_is_rejected() {
    let resolver = Arc::new(FakeResolver::default());
    let svc = service_with_resolver(resolver);
    let owner = TenantContext::user("owner@x.com");

    // Valid reference: create succeeds.
    let ok = svc
        .create_page(
            private_page_with_secret("db", vec![SecretRef::new("db", "ref://db/creds")]),
            &owner,
        )
        .await;
    assert!(ok.is_ok(), "valid reference must be accepted: {ok:?}");

    // Raw secret value: create is REJECTED at the service layer (not just lint).
    let bad = svc
        .create_page(
            private_page_with_secret("api", vec![SecretRef::new("api", "hunter2-raw-secret")]),
            &owner,
        )
        .await;
    assert!(
        matches!(bad, Err(MindPalaceError::Validation(_))),
        "raw secret value must be rejected at create, got {bad:?}"
    );
}

// --- Criterion 2: wiki_read (Full) never returns a resolved value ---

#[tokio::test]
async fn read_never_returns_resolved_value_only_the_reference() {
    let resolver = Arc::new(FakeResolver::default());
    let svc = service_with_resolver(resolver.clone());
    let owner = TenantContext::user("owner@x.com");
    svc.create_page(
        private_page_with_secret("db", vec![SecretRef::new("db", "ref://db/creds")]),
        &owner,
    )
    .await
    .unwrap();

    let resp = svc
        .read_page(&Slug::new("db").unwrap(), ReadLevel::Full, &owner)
        .await
        .unwrap();
    match resp {
        PageResponse::Full(page) => {
            // The page carries the reference, not a value.
            assert_eq!(page.secret_refs.len(), 1);
            assert_eq!(page.secret_refs[0].reference, "ref://db/creds");
            // read must NOT have resolved anything.
            assert!(
                resolver.resolved.lock().unwrap().is_empty(),
                "read_page must never call the resolver"
            );
            // No section content contains the resolved value marker.
            assert!(
                !page
                    .sections
                    .iter()
                    .any(|s| s.content.contains("VALUE_FOR::")),
                "resolved value must never appear in page content"
            );
        }
        _ => panic!("expected Full"),
    }
}

// --- Criterion 3: get_secret gated by can_see; denied otherwise; audited ---

#[tokio::test]
async fn get_secret_allows_owner_denies_stranger() {
    let resolver = Arc::new(FakeResolver::default());
    let svc = service_with_resolver(resolver.clone());
    let owner = TenantContext::user("owner@x.com");
    svc.create_page(
        private_page_with_secret("db", vec![SecretRef::new("db", "ref://db/creds")]),
        &owner,
    )
    .await
    .unwrap();

    // Owner can resolve.
    let val = svc
        .get_secret(&Slug::new("db").unwrap(), "db", &owner)
        .await
        .expect("owner may resolve");
    assert_eq!(val.expose(), "VALUE_FOR::ref://db/creds");
    assert_eq!(resolver.resolved.lock().unwrap().len(), 1, "one resolve");

    // Stranger cannot even see the private page → clean PageNotFound, and the
    // resolver is NOT called (no leak of existence, no resolution).
    let stranger = TenantContext::user("stranger@x.com");
    let denied = svc
        .get_secret(&Slug::new("db").unwrap(), "db", &stranger)
        .await;
    assert!(
        matches!(denied, Err(MindPalaceError::PageNotFound(_))),
        "stranger must be denied as PageNotFound, got {denied:?}"
    );
    assert_eq!(
        resolver.resolved.lock().unwrap().len(),
        1,
        "denied resolve must not reach the backend"
    );

    // Anonymous also denied.
    let anon = TenantContext::anonymous();
    let anon_res = svc.get_secret(&Slug::new("db").unwrap(), "db", &anon).await;
    assert!(matches!(anon_res, Err(MindPalaceError::PageNotFound(_))));
}

#[tokio::test]
async fn get_secret_missing_name_is_clean_not_found() {
    let resolver = Arc::new(FakeResolver::default());
    let svc = service_with_resolver(resolver);
    let owner = TenantContext::user("owner@x.com");
    svc.create_page(
        private_page_with_secret("db", vec![SecretRef::new("db", "ref://db/creds")]),
        &owner,
    )
    .await
    .unwrap();
    let res = svc
        .get_secret(&Slug::new("db").unwrap(), "does-not-exist", &owner)
        .await;
    assert!(matches!(res, Err(MindPalaceError::PageNotFound(_))));
}

// --- Criterion 6 (partial): no backend configured → resolution denied ---

#[tokio::test]
async fn get_secret_without_backend_is_not_configured() {
    // No resolver: refs stored as-is (validation skipped), resolution denied.
    let svc = service_no_resolver();
    let owner = TenantContext::user("owner@x.com");
    svc.create_page(
        private_page_with_secret("db", vec![SecretRef::new("db", "ref://db/creds")]),
        &owner,
    )
    .await
    .unwrap();
    let res = svc
        .get_secret(&Slug::new("db").unwrap(), "db", &owner)
        .await;
    assert!(
        matches!(
            res,
            Err(MindPalaceError::Secret(SecretError::NotConfigured(_)))
        ),
        "no backend must yield NotConfigured, got {res:?}"
    );
}

// --- Criterion 4: Public page carrying a secret ref fails lint (Error) ---

#[tokio::test]
async fn public_page_with_secret_ref_fails_lint() {
    let resolver = Arc::new(FakeResolver::default());
    let svc = service_with_resolver(resolver);
    let owner = TenantContext::user("owner@x.com");

    let mut input = private_page_with_secret("pub", vec![SecretRef::new("k", "ref://pub/key")]);
    input.base_visibility = Some(BaseVisibility::Public);

    let (_page, issues) = svc.create_page(input, &owner).await.unwrap();
    assert!(
        issues
            .iter()
            .any(|i| i.code == LintCode::PublicPageWithSecrets && i.severity == Severity::Error),
        "a Public page with a secret ref must produce a PublicPageWithSecrets Error; got {issues:?}"
    );
}

// --- Lint: invalid reference is an Error when a resolver is available ---

#[test]
fn lint_flags_invalid_reference_with_resolver() {
    let resolver = FakeResolver::default();
    // Build a Private page directly with a bad ref (bypassing service validation
    // to exercise the lint rule in isolation).
    let mut page = Page::new_with_access(
        "p".into(),
        Slug::new("p").unwrap(),
        "s".into(),
        vec![Section {
            heading: "H".into(),
            content: "C".into(),
        }],
        PageType::Leaf,
        PageAccess {
            owner: Some("o@x.com".into()),
            base_visibility: BaseVisibility::Private,
            grants: vec![],
        },
    )
    .unwrap();
    page.secret_refs = vec![SecretRef::new("bad", "raw-secret-not-a-ref")];

    let issues = lint_page(&page, None, Some(&resolver));
    assert!(
        issues
            .iter()
            .any(|i| i.code == LintCode::InvalidSecretReference && i.severity == Severity::Error),
        "invalid reference must be an Error; got {issues:?}"
    );
}

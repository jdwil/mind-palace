//! Regression + acceptance tests for two changes:
//!
//! 1. Archived pages must NOT appear in search (security: an archived page's
//!    version history can hold scrubbed-but-sensitive content). Covers the
//!    fail-CLOSED search filter and the graph-node archived guard.
//! 2. First-class global Admin role via the reserved `admins` group: an admin
//!    can see/edit any page and manage any group; the `admins` id is reserved;
//!    admin overrides the owner/manager gates. Non-admins are unaffected.
//!
//! Uses in-memory mock adapters. A key mock here (`LeakyVectorSearch`) keeps a
//! page's embedding even after `delete_embedding` is called, to prove the
//! SEARCH FILTER — not the best-effort vector delete — is what keeps archived
//! pages out of results.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use mind_palace_core::domain::graph::KnowledgeGraph;
use mind_palace_core::domain::group::Group;
use mind_palace_core::domain::page::{Page, ReadLevel};
use mind_palace_core::domain::service::{CreatePageInput, UpdatePageInput, WikiService};
use mind_palace_core::domain::tenant::TenantContext;
use mind_palace_core::domain::value_objects::*;
use mind_palace_core::error::MindPalaceError;
use mind_palace_core::ports::embedding::EmbeddingPort;
use mind_palace_core::ports::graph::{GraphData, GraphEdgeData, GraphNodeData, GraphStore};
use mind_palace_core::ports::group_store::GroupStore;
use mind_palace_core::ports::page_store::{PageFilter, PageStore};
use mind_palace_core::ports::vector_search::{EmbeddingMetadata, SearchResult, VectorSearchPort};
use tokio::sync::RwLock;

// --- Mocks ---

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
    async fn get_page(&self, id: &PageId, _ctx: &TenantContext) -> Result<Page, MindPalaceError> {
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
        _ctx: &TenantContext,
    ) -> Result<Page, MindPalaceError> {
        self.get_page_by_slug_unfiltered(slug).await
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
        let mut pages = self.pages.lock().unwrap();
        pages.retain(|p| p.id != page.id);
        pages.push(page.clone());
        Ok(())
    }
    async fn delete_page(&self, id: &PageId) -> Result<(), MindPalaceError> {
        self.pages.lock().unwrap().retain(|p| &p.id != id);
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

struct MockEmbedding;
#[async_trait]
impl EmbeddingPort for MockEmbedding {
    async fn embed_text(&self, _t: &str) -> Result<Vec<f64>, MindPalaceError> {
        Ok(vec![0.1, 0.2, 0.3])
    }
    async fn embed_texts(&self, texts: &[&str]) -> Result<Vec<Vec<f64>>, MindPalaceError> {
        Ok(texts.iter().map(|_| vec![0.1, 0.2, 0.3]).collect())
    }
}

/// Vector search that honors `delete_embedding` (the normal case): returns every
/// embedded page so the service's own access re-filter is what's under test.
struct MockVectorSearch {
    upserted: Mutex<Vec<EmbeddingMetadata>>,
}
impl MockVectorSearch {
    fn new() -> Self {
        Self {
            upserted: Mutex::new(Vec::new()),
        }
    }
}
#[async_trait]
impl VectorSearchPort for MockVectorSearch {
    async fn search(
        &self,
        _e: &[f64],
        _limit: usize,
        _ctx: &TenantContext,
    ) -> Result<Vec<SearchResult>, MindPalaceError> {
        Ok(self
            .upserted
            .lock()
            .unwrap()
            .iter()
            .map(to_result)
            .collect())
    }
    async fn upsert_embedding(
        &self,
        meta: &EmbeddingMetadata,
        _e: &[f64],
    ) -> Result<(), MindPalaceError> {
        let mut up = self.upserted.lock().unwrap();
        up.retain(|m| m.page_id != meta.page_id);
        up.push(meta.clone());
        Ok(())
    }
    async fn delete_embedding(&self, id: &PageId) -> Result<(), MindPalaceError> {
        self.upserted.lock().unwrap().retain(|m| &m.page_id != id);
        Ok(())
    }
}

/// Vector search whose `delete_embedding` is a NO-OP — models a stale/orphaned
/// vector that survives archive (delete failed, eventual consistency, or the
/// page was archived before the delete path existed). This is the exact
/// condition that made archived pages leak into search; the fail-closed filter
/// must keep them out regardless.
struct LeakyVectorSearch {
    upserted: Mutex<Vec<EmbeddingMetadata>>,
}
impl LeakyVectorSearch {
    fn new() -> Self {
        Self {
            upserted: Mutex::new(Vec::new()),
        }
    }
}
#[async_trait]
impl VectorSearchPort for LeakyVectorSearch {
    async fn search(
        &self,
        _e: &[f64],
        _limit: usize,
        _ctx: &TenantContext,
    ) -> Result<Vec<SearchResult>, MindPalaceError> {
        Ok(self
            .upserted
            .lock()
            .unwrap()
            .iter()
            .map(to_result)
            .collect())
    }
    async fn upsert_embedding(
        &self,
        meta: &EmbeddingMetadata,
        _e: &[f64],
    ) -> Result<(), MindPalaceError> {
        let mut up = self.upserted.lock().unwrap();
        up.retain(|m| m.page_id != meta.page_id);
        up.push(meta.clone());
        Ok(())
    }
    async fn delete_embedding(&self, _id: &PageId) -> Result<(), MindPalaceError> {
        // Intentionally does NOT remove the vector — simulates a surviving vector.
        Ok(())
    }
}

fn to_result(m: &EmbeddingMetadata) -> SearchResult {
    SearchResult {
        page_id: m.page_id.clone(),
        slug: m.slug.clone(),
        title: m.title.clone(),
        summary: String::new(),
        score: 1.0,
    }
}

/// In-memory graph store that persists nodes/edges so list/search reload works.
#[derive(Default)]
struct MockGraphStore {
    nodes: Mutex<Vec<GraphNodeData>>,
    edges: Mutex<Vec<GraphEdgeData>>,
}
#[async_trait]
impl GraphStore for MockGraphStore {
    async fn load_graph(&self) -> Result<GraphData, MindPalaceError> {
        Ok(GraphData {
            nodes: self.nodes.lock().unwrap().clone(),
            edges: self.edges.lock().unwrap().clone(),
        })
    }
    async fn save_node(&self, node: &GraphNodeData) -> Result<(), MindPalaceError> {
        let mut n = self.nodes.lock().unwrap();
        n.retain(|x| x.page_id != node.page_id);
        n.push(node.clone());
        Ok(())
    }
    async fn save_edge(&self, edge: &GraphEdgeData) -> Result<(), MindPalaceError> {
        self.edges.lock().unwrap().push(edge.clone());
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

#[derive(Default)]
struct MockGroupStore {
    groups: Mutex<Vec<Group>>,
}
#[async_trait]
impl GroupStore for MockGroupStore {
    async fn get_group(&self, id: &GroupId) -> Result<Option<Group>, MindPalaceError> {
        Ok(self
            .groups
            .lock()
            .unwrap()
            .iter()
            .find(|g| &g.id == id)
            .cloned())
    }
    async fn list_groups(&self) -> Result<Vec<Group>, MindPalaceError> {
        Ok(self.groups.lock().unwrap().clone())
    }
    async fn save_group(&self, group: &Group) -> Result<(), MindPalaceError> {
        let mut g = self.groups.lock().unwrap();
        g.retain(|x| x.id != group.id);
        g.push(group.clone());
        Ok(())
    }
    async fn delete_group(&self, id: &GroupId) -> Result<(), MindPalaceError> {
        self.groups.lock().unwrap().retain(|g| &g.id != id);
        Ok(())
    }
}

fn service_with_vectors(vs: Arc<dyn VectorSearchPort>) -> Arc<WikiService> {
    Arc::new(
        WikiService::new(
            Arc::new(MockPageStore::new()),
            vs,
            Arc::new(MockEmbedding),
            Arc::new(MockGraphStore::default()),
            Arc::new(RwLock::new(KnowledgeGraph::new())),
        )
        .with_group_store(Arc::new(MockGroupStore::default())),
    )
}

fn public_input(slug: &str) -> CreatePageInput {
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
        base_visibility: Some(BaseVisibility::Public),
        parent: None,
        secret_refs: vec![],
    }
}

fn private_input(slug: &str) -> CreatePageInput {
    CreatePageInput {
        base_visibility: Some(BaseVisibility::Private),
        ..public_input(slug)
    }
}

fn search_slugs(results: &[SearchResult]) -> Vec<String> {
    results
        .iter()
        .map(|r| r.slug.as_str().to_string())
        .collect()
}

// =====================================================================
// #2 — Archived pages must not appear in search
// =====================================================================

/// The core regression: archive removes a page from search; unarchive restores
/// it. Runs against the NORMAL vector store (delete honored).
#[tokio::test]
async fn archive_removes_from_search_unarchive_restores() {
    let svc = service_with_vectors(Arc::new(MockVectorSearch::new()));
    let admin = TenantContext::global();
    let slug = Slug::new("gong-tool").unwrap();

    svc.create_page(public_input("gong-tool"), &admin)
        .await
        .unwrap();

    // Present before archive.
    let before = svc.search("gong access api", &admin, 10).await.unwrap();
    assert!(
        search_slugs(&before).contains(&"gong-tool".to_string()),
        "page should be searchable before archive"
    );

    // Archive -> absent from search.
    svc.archive_page(&slug, &admin).await.unwrap();
    let after = svc.search("gong access api", &admin, 10).await.unwrap();
    assert!(
        !search_slugs(&after).contains(&"gong-tool".to_string()),
        "archived page must NOT appear in search"
    );

    // Unarchive -> returns.
    svc.unarchive_page(&slug).await.unwrap();
    let restored = svc.search("gong access api", &admin, 10).await.unwrap();
    assert!(
        search_slugs(&restored).contains(&"gong-tool".to_string()),
        "unarchived page should be searchable again"
    );
}

/// The security-critical case: even if the embedding SURVIVES archive (delete
/// failed / eventual consistency / archived before the delete path existed),
/// the page must not surface. This is the fail-CLOSED filter under test — the
/// previous `None => true` branch leaked exactly this scenario.
#[tokio::test]
async fn archived_page_with_surviving_vector_is_not_returned() {
    let svc = service_with_vectors(Arc::new(LeakyVectorSearch::new()));
    let admin = TenantContext::global();
    let slug = Slug::new("gong-tool").unwrap();

    svc.create_page(public_input("gong-tool"), &admin)
        .await
        .unwrap();
    // Sanity: the leaky store still has the vector and returns it pre-archive.
    let before = svc.search("gong", &admin, 10).await.unwrap();
    assert!(search_slugs(&before).contains(&"gong-tool".to_string()));

    // Archive. The leaky vector store keeps the embedding, so the vector query
    // will still return this page_id — the service filter must drop it.
    svc.archive_page(&slug, &admin).await.unwrap();
    let after = svc.search("gong", &admin, 10).await.unwrap();
    assert!(
        !search_slugs(&after).contains(&"gong-tool".to_string()),
        "archived page must be filtered out even when its vector survives (fail-closed)"
    );
}

/// After a graph reload (which `list_pages` triggers and a restart would do),
/// the archived node is back in the in-memory graph with Archived visibility.
/// It must still be excluded from search and list.
#[tokio::test]
async fn archived_page_excluded_after_graph_reload() {
    let svc = service_with_vectors(Arc::new(LeakyVectorSearch::new()));
    let admin = TenantContext::global();
    let slug = Slug::new("gong-tool").unwrap();

    svc.create_page(public_input("gong-tool"), &admin)
        .await
        .unwrap();
    svc.archive_page(&slug, &admin).await.unwrap();

    // list_pages reloads the graph from the store — the archived node (preserved
    // in the store for unarchive) comes back into memory as Archived.
    let listed = svc
        .list_pages(&PageFilter::default(), &admin)
        .await
        .unwrap();
    assert!(
        !listed.iter().any(|p| p.slug.as_str() == "gong-tool"),
        "archived page must not appear in list after reload"
    );

    // And search still excludes it after the reload repopulated the node.
    let after = svc.search("gong", &admin, 10).await.unwrap();
    assert!(
        !search_slugs(&after).contains(&"gong-tool".to_string()),
        "archived page must not appear in search after graph reload"
    );
}

// =====================================================================
// #1 — Global Admin role via the reserved `admins` group
// =====================================================================

/// Seed the reserved admins group with `admin_email` as a member (and a
/// separate manager `root@x.com` so the group is manageable), returning a
/// service whose group store already knows the admin.
async fn service_with_admin(admin_email: &str) -> Arc<WikiService> {
    let group_store = Arc::new(MockGroupStore::default());
    let mut admins = Group::new(GroupId::admins(), "Mind Palace Admins", "root@x.com");
    admins.add_member(admin_email);
    group_store.save_group(&admins).await.unwrap();

    Arc::new(
        WikiService::new(
            Arc::new(MockPageStore::new()),
            Arc::new(MockVectorSearch::new()),
            Arc::new(MockEmbedding),
            Arc::new(MockGraphStore::default()),
            Arc::new(RwLock::new(KnowledgeGraph::new())),
        )
        .with_group_store(group_store),
    )
}

#[tokio::test]
async fn admin_can_see_and_edit_any_private_page() {
    let svc = service_with_admin("admin@x.com").await;
    let owner = TenantContext::user("owner@x.com");
    let admin = TenantContext::user("admin@x.com");
    let stranger = TenantContext::user("stranger@x.com");

    svc.create_page(private_input("secret"), &owner)
        .await
        .unwrap();
    let slug = Slug::new("secret").unwrap();

    // Stranger (non-admin) cannot see it — baseline unchanged.
    assert!(
        svc.read_page(&slug, ReadLevel::Summary, &stranger)
            .await
            .is_err()
    );

    // Admin can read a private page they do not own.
    assert!(
        svc.read_page(&slug, ReadLevel::Summary, &admin)
            .await
            .is_ok(),
        "admin should read any private page"
    );

    // Admin can edit it.
    let upd = UpdatePageInput {
        title: Some("edited-by-admin".into()),
        ..Default::default()
    };
    assert!(
        svc.update_page(&slug, upd, &admin).await.is_ok(),
        "admin should edit any page"
    );

    // Admin can manage grants on a page they do not own.
    assert!(
        svc.share_page(
            &slug,
            Principal::User("friend@x.com".into()),
            Level::View,
            &admin,
        )
        .await
        .is_ok(),
        "admin should manage grants on any page"
    );
}

#[tokio::test]
async fn admin_can_manage_any_group() {
    let svc = service_with_admin("admin@x.com").await;
    let owner = TenantContext::user("owner@x.com");
    let admin = TenantContext::user("admin@x.com");
    let intruder = TenantContext::user("intruder@x.com");

    // A group managed solely by `owner`.
    svc.group_create(GroupId::new("team"), "Team", &owner)
        .await
        .unwrap();

    // A non-admin, non-manager cannot add members — baseline unchanged.
    assert!(matches!(
        svc.group_add_member(&GroupId::new("team"), "x@x.com", &intruder)
            .await,
        Err(MindPalaceError::AccessDenied(_))
    ));

    // The admin can add members and promote managers on a group they don't manage.
    assert!(
        svc.group_add_member(&GroupId::new("team"), "newbie@x.com", &admin)
            .await
            .is_ok(),
        "admin should manage any group's membership"
    );
    let g = svc
        .group_add_manager(&GroupId::new("team"), "newbie@x.com", &admin)
        .await
        .unwrap();
    assert!(g.is_manager("newbie@x.com"));
}

#[tokio::test]
async fn admin_grant_and_revoke_via_reserved_group() {
    let svc = service_with_admin("admin@x.com").await;
    let admin = TenantContext::user("admin@x.com");
    let candidate = TenantContext::user("candidate@x.com");

    // Initially the candidate is not an admin: cannot list admins.
    assert!(svc.admin_list(&candidate).await.is_err());

    // Admin lists current admins (includes root manager + admin member).
    let admins = svc.admin_list(&admin).await.unwrap();
    assert!(admins.contains(&"admin@x.com".to_string()));

    // Admin promotes the candidate.
    svc.admin_grant("candidate@x.com", &admin).await.unwrap();
    // Now the candidate is a global admin: can list admins.
    let admins2 = svc.admin_list(&candidate).await.unwrap();
    assert!(admins2.contains(&"candidate@x.com".to_string()));

    // Admin revokes the candidate -> no longer an admin.
    svc.admin_revoke("candidate@x.com", &admin).await.unwrap();
    assert!(svc.admin_list(&candidate).await.is_err());
}

#[tokio::test]
async fn non_admin_cannot_grant_admin() {
    let svc = service_with_admin("admin@x.com").await;
    let attacker = TenantContext::user("attacker@x.com");

    // A non-admin cannot promote anyone (including themselves) to admin — the
    // reserved group is manager-gated, and the attacker is neither manager nor
    // admin.
    let res = svc.admin_grant("attacker@x.com", &attacker).await;
    assert!(matches!(res, Err(MindPalaceError::AccessDenied(_))));
}

#[tokio::test]
async fn reserved_admins_group_cannot_be_created() {
    let svc = service_with_vectors(Arc::new(MockVectorSearch::new()));
    let user = TenantContext::user("nobody@x.com");

    // Attempting to (re)create the reserved `admins` group is refused, so no one
    // can bootstrap themselves into admin by creating the group and becoming its
    // sole manager+member.
    let res = svc
        .group_create(GroupId::admins(), "Pwned Admins", &user)
        .await;
    assert!(matches!(res, Err(MindPalaceError::AccessDenied(_))));
}

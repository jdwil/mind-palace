//! Spec 2 acceptance tests: owner + grants + groups authorization at the
//! service layer. Uses in-memory mock adapters (including a group store) so the
//! full create → share → read/edit/list flow is exercised end to end.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use mind_palace_core::domain::graph::KnowledgeGraph;
use mind_palace_core::domain::group::Group;
use mind_palace_core::domain::page::{Page, ReadLevel};
use mind_palace_core::domain::service::{CreatePageInput, WikiService};
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

/// Vector search that returns every embedded page (partition-agnostic) so the
/// service's own access re-filter is what's under test.
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
            .map(|m| SearchResult {
                page_id: m.page_id.clone(),
                slug: m.slug.clone(),
                title: m.title.clone(),
                summary: String::new(),
                score: 1.0,
            })
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

fn make_service() -> WikiService {
    WikiService::new(
        Arc::new(MockPageStore::new()),
        Arc::new(MockVectorSearch::new()),
        Arc::new(MockEmbedding),
        Arc::new(MockGraphStore::default()),
        Arc::new(RwLock::new(KnowledgeGraph::new())),
    )
    .with_group_store(Arc::new(MockGroupStore::default()))
}

fn private_page_input(slug: &str) -> CreatePageInput {
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
    }
}

// --- Criterion 1 & 3: private page invisible to non-owner; owner can edit ---

#[tokio::test]
async fn private_page_owner_only_until_shared() {
    let svc = make_service();
    let owner = TenantContext::user("owner@x.com");
    let stranger = TenantContext::user("stranger@x.com");

    let (page, _) = svc
        .create_page(private_page_input("secret"), &owner)
        .await
        .unwrap();
    assert_eq!(page.access.owner.as_deref(), Some("owner@x.com"));
    assert_eq!(page.access.base_visibility, BaseVisibility::Private);

    let slug = Slug::new("secret").unwrap();

    // Owner can read.
    assert!(
        svc.read_page(&slug, ReadLevel::Summary, &owner)
            .await
            .is_ok()
    );
    // Stranger cannot read (criterion 1).
    assert!(
        svc.read_page(&slug, ReadLevel::Summary, &stranger)
            .await
            .is_err()
    );
    // Stranger cannot edit (criterion 3).
    let upd = mind_palace_core::domain::service::UpdatePageInput {
        title: Some("hacked".into()),
        ..Default::default()
    };
    assert!(svc.update_page(&slug, upd, &stranger).await.is_err());

    // Owner shares View with stranger → stranger can now read but not edit.
    svc.share_page(
        &slug,
        Principal::User("stranger@x.com".into()),
        Level::View,
        &owner,
    )
    .await
    .unwrap();
    assert!(
        svc.read_page(&slug, ReadLevel::Summary, &stranger)
            .await
            .is_ok()
    );
    let upd2 = mind_palace_core::domain::service::UpdatePageInput {
        title: Some("still-cant".into()),
        ..Default::default()
    };
    assert!(svc.update_page(&slug, upd2, &stranger).await.is_err());
}

// --- Criterion 4: privileged-op denial (share by non-owner) ---

#[tokio::test]
async fn non_owner_cannot_share() {
    let svc = make_service();
    let owner = TenantContext::user("owner@x.com");
    let attacker = TenantContext::user("attacker@x.com");
    svc.create_page(private_page_input("doc"), &owner)
        .await
        .unwrap();
    let slug = Slug::new("doc").unwrap();

    // Attacker attempts to grant themselves access — denied (Outline-class).
    let res = svc
        .share_page(
            &slug,
            Principal::User("attacker@x.com".into()),
            Level::Edit,
            &attacker,
        )
        .await;
    assert!(matches!(res, Err(MindPalaceError::AccessDenied(_))));
    // Still invisible to attacker.
    assert!(
        svc.read_page(&slug, ReadLevel::Summary, &attacker)
            .await
            .is_err()
    );
}

// --- Criterion 1 & group grants: group member sees a group-shared page ---

#[tokio::test]
async fn group_member_sees_group_shared_page() {
    let svc = make_service();
    let owner = TenantContext::user("owner@x.com");
    let member = TenantContext::user("member@x.com");
    let outsider = TenantContext::user("outsider@x.com");

    // Owner creates a group and adds a member.
    svc.group_create(GroupId::new("team"), "Team", &owner)
        .await
        .unwrap();
    svc.group_add_member(&GroupId::new("team"), "member@x.com", &owner)
        .await
        .unwrap();

    svc.create_page(private_page_input("team-doc"), &owner)
        .await
        .unwrap();
    let slug = Slug::new("team-doc").unwrap();

    // Share with the group at View.
    svc.share_page(
        &slug,
        Principal::Group(GroupId::new("team")),
        Level::View,
        &owner,
    )
    .await
    .unwrap();

    // Member sees it; outsider does not.
    assert!(
        svc.read_page(&slug, ReadLevel::Summary, &member)
            .await
            .is_ok()
    );
    assert!(
        svc.read_page(&slug, ReadLevel::Summary, &outsider)
            .await
            .is_err()
    );
}

// --- Criterion 4: non-manager cannot add group members ---

#[tokio::test]
async fn non_manager_cannot_add_group_member() {
    let svc = make_service();
    let owner = TenantContext::user("owner@x.com");
    let member = TenantContext::user("member@x.com");

    svc.group_create(GroupId::new("g"), "G", &owner)
        .await
        .unwrap();
    svc.group_add_member(&GroupId::new("g"), "member@x.com", &owner)
        .await
        .unwrap();

    // A regular member cannot add anyone (no self-service escalation).
    let res = svc
        .group_add_member(&GroupId::new("g"), "intruder@x.com", &member)
        .await;
    assert!(matches!(res, Err(MindPalaceError::AccessDenied(_))));
}

// --- Criterion 5: search/list never return a page the identity can't see ---

#[tokio::test]
async fn search_and_list_filter_by_access() {
    let svc = make_service();
    let owner = TenantContext::user("owner@x.com");
    let stranger = TenantContext::user("stranger@x.com");

    // A public page and a private page.
    svc.create_page(
        CreatePageInput {
            base_visibility: Some(BaseVisibility::Public),
            ..private_page_input("pub")
        },
        &owner,
    )
    .await
    .unwrap();
    svc.create_page(private_page_input("priv"), &owner)
        .await
        .unwrap();

    // Stranger's list shows only the public page.
    let listed = svc
        .list_pages(&PageFilter::default(), &stranger)
        .await
        .unwrap();
    let slugs: Vec<_> = listed.iter().map(|p| p.slug.as_str().to_string()).collect();
    assert!(slugs.contains(&"pub".to_string()));
    assert!(!slugs.contains(&"priv".to_string()));

    // Stranger's search never returns the private page.
    let results = svc.search("anything", &stranger, 10).await.unwrap();
    let rslugs: Vec<_> = results.iter().map(|r| r.slug.as_str().to_string()).collect();
    assert!(!rslugs.contains(&"priv".to_string()));

    // Owner sees both in list.
    let owner_list = svc
        .list_pages(&PageFilter::default(), &owner)
        .await
        .unwrap();
    assert_eq!(owner_list.len(), 2);
}

// --- Criterion 2: public page visible to anonymous ---

#[tokio::test]
async fn anonymous_sees_public_page() {
    let svc = make_service();
    let owner = TenantContext::user("owner@x.com");
    svc.create_page(
        CreatePageInput {
            base_visibility: Some(BaseVisibility::Public),
            ..private_page_input("open")
        },
        &owner,
    )
    .await
    .unwrap();

    let anon = TenantContext::anonymous();
    assert!(
        svc.read_page(&Slug::new("open").unwrap(), ReadLevel::Summary, &anon)
            .await
            .is_ok()
    );
}

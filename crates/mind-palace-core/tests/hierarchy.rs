//! Spec 3 acceptance tests: hierarchical access inheritance via a distinct
//! parent/child CONTAINMENT relationship. Grants inherit DOWNWARD along
//! containment edges only — never across `Related` links. Uses in-memory mock
//! adapters (with a graph store that persists nodes AND edges) so the full
//! create → parent → read/edit/traverse flow is exercised end to end.
//!
//! Criteria (from the spec):
//! 1. A grant on a parent is inherited by children and grandchildren (view+edit).
//! 2. A grant does NOT propagate across a `Related` link (only containment).
//! 3. Assigning a `parent` that would create a cycle is rejected.
//! 4. A page with no parent behaves exactly as in Spec 2.
//! 5. Setting/using a parent requires `can_edit` on both child and parent.
//! 6. `wiki_traverse` (Related) behavior is unchanged; containment is separate.
//! 7. Generic — no consumer-specific concepts introduced (structural: the API
//!    only speaks slugs/grants/groups).

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

// --- Mocks (mirror access_control.rs; graph store persists nodes + edges) ---

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

/// Persists nodes AND edges so effective-access resolution over containment
/// works after a graph reload (list_pages reloads from here).
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
    async fn delete_edge(&self, s: &PageId, t: &PageId) -> Result<(), MindPalaceError> {
        self.edges
            .lock()
            .unwrap()
            .retain(|e| !(&e.source == s && &e.target == t));
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

/// A private page owned by the caller, optionally under a containment parent.
fn private_input(slug_str: &str, parent: Option<&str>) -> CreatePageInput {
    CreatePageInput {
        title: slug_str.into(),
        slug: Slug::new(slug_str).unwrap(),
        summary: "s".into(),
        sections: vec![Section {
            heading: "H".into(),
            content: "C".into(),
        }],
        page_type: PageType::Leaf,
        visibility: Visibility::General,
        links: vec![],
        base_visibility: Some(BaseVisibility::Private),
        parent: parent.map(|p| Slug::new(p).unwrap()),
        secret_refs: vec![],
    }
}

fn slug(s: &str) -> Slug {
    Slug::new(s).unwrap()
}

// --- Criterion 1: a grant on a parent is inherited by children & grandchildren ---

#[tokio::test]
async fn grant_on_parent_inherited_by_descendants() {
    let svc = make_service();
    let owner = TenantContext::user("owner@x.com");
    let grantee = TenantContext::user("grantee@x.com");
    let stranger = TenantContext::user("stranger@x.com");

    // root <- child <- grandchild (containment chain).
    svc.create_page(private_input("root", None), &owner)
        .await
        .unwrap();
    svc.create_page(private_input("child", Some("root")), &owner)
        .await
        .unwrap();
    svc.create_page(private_input("grandchild", Some("child")), &owner)
        .await
        .unwrap();

    // Before sharing: grantee sees nothing.
    assert!(
        svc.read_page(&slug("grandchild"), ReadLevel::Summary, &grantee)
            .await
            .is_err()
    );

    // Owner grants Edit on the ROOT to grantee.
    svc.share_page(
        &slug("root"),
        Principal::User("grantee@x.com".into()),
        Level::Edit,
        &owner,
    )
    .await
    .unwrap();

    // Grantee can now VIEW the child and grandchild via inheritance.
    assert!(
        svc.read_page(&slug("child"), ReadLevel::Summary, &grantee)
            .await
            .is_ok(),
        "grant on parent should be inherited by child"
    );
    assert!(
        svc.read_page(&slug("grandchild"), ReadLevel::Summary, &grantee)
            .await
            .is_ok(),
        "grant on grandparent should be inherited by grandchild"
    );

    // And EDIT inherits too (Edit grant on ancestor confers edit on descendant).
    let upd = UpdatePageInput {
        title: Some("edited-via-inheritance".into()),
        ..Default::default()
    };
    assert!(
        svc.update_page(&slug("grandchild"), upd, &grantee)
            .await
            .is_ok(),
        "Edit grant on ancestor should confer edit on descendant"
    );

    // A stranger with no grant anywhere still sees nothing.
    assert!(
        svc.read_page(&slug("grandchild"), ReadLevel::Summary, &stranger)
            .await
            .is_err()
    );
}

// --- Criterion 2: a grant does NOT propagate across a Related link ---

#[tokio::test]
async fn grant_does_not_cross_related_link() {
    let svc = make_service();
    let owner = TenantContext::user("owner@x.com");
    let grantee = TenantContext::user("grantee@x.com");

    // `shared` is granted to grantee; `associated` merely RELATES to `shared`
    // (a see-also link) and must NOT inherit the grant.
    svc.create_page(private_input("shared", None), &owner)
        .await
        .unwrap();
    let mut related_input = private_input("associated", None);
    related_input.links = vec![slug("shared")];
    svc.create_page(related_input, &owner).await.unwrap();

    svc.share_page(
        &slug("shared"),
        Principal::User("grantee@x.com".into()),
        Level::View,
        &owner,
    )
    .await
    .unwrap();

    // Grantee sees the directly-shared page…
    assert!(
        svc.read_page(&slug("shared"), ReadLevel::Summary, &grantee)
            .await
            .is_ok()
    );
    // …but NOT the merely-associated page (Related carries no access).
    assert!(
        svc.read_page(&slug("associated"), ReadLevel::Summary, &grantee)
            .await
            .is_err(),
        "access must not leak across a Related link"
    );
}

// --- Criterion 3: a parent that would create a cycle is rejected ---

#[tokio::test]
async fn cyclic_parent_is_rejected() {
    let svc = make_service();
    let owner = TenantContext::user("owner@x.com");

    // a <- b <- c chain.
    svc.create_page(private_input("a", None), &owner)
        .await
        .unwrap();
    svc.create_page(private_input("b", Some("a")), &owner)
        .await
        .unwrap();
    svc.create_page(private_input("c", Some("b")), &owner)
        .await
        .unwrap();

    // Reparent `a` under `c` → would form a cycle (a→c→b→a). Reject.
    let upd = UpdatePageInput {
        parent: Some(Some(slug("c"))),
        ..Default::default()
    };
    let res = svc.update_page(&slug("a"), upd, &owner).await;
    assert!(
        matches!(res, Err(MindPalaceError::Validation(_))),
        "cyclic reparent must be rejected, got {res:?}"
    );

    // Self-parent is also a cycle.
    let self_parent = UpdatePageInput {
        parent: Some(Some(slug("b"))),
        ..Default::default()
    };
    assert!(
        matches!(
            svc.update_page(&slug("b"), self_parent, &owner).await,
            Err(MindPalaceError::Validation(_))
        ),
        "self-parent must be rejected"
    );
}

// --- Criterion 4: a page with no parent behaves exactly as in Spec 2 ---

#[tokio::test]
async fn root_page_behaves_like_spec2() {
    let svc = make_service();
    let owner = TenantContext::user("owner@x.com");
    let stranger = TenantContext::user("stranger@x.com");

    // No parent → private page visible only to owner until explicitly shared.
    svc.create_page(private_input("solo", None), &owner)
        .await
        .unwrap();
    assert!(
        svc.read_page(&slug("solo"), ReadLevel::Summary, &owner)
            .await
            .is_ok()
    );
    assert!(
        svc.read_page(&slug("solo"), ReadLevel::Summary, &stranger)
            .await
            .is_err()
    );

    // A direct grant works exactly as Spec 2.
    svc.share_page(
        &slug("solo"),
        Principal::User("stranger@x.com".into()),
        Level::View,
        &owner,
    )
    .await
    .unwrap();
    assert!(
        svc.read_page(&slug("solo"), ReadLevel::Summary, &stranger)
            .await
            .is_ok()
    );
}

// --- Criterion 5: setting/using a parent requires can_edit on BOTH ---

#[tokio::test]
async fn parent_requires_edit_on_both() {
    let svc = make_service();
    let owner = TenantContext::user("owner@x.com");
    let other = TenantContext::user("other@x.com");

    // Owner creates a private container.
    svc.create_page(private_input("container", None), &owner)
        .await
        .unwrap();

    // `other` cannot even see `container` → cannot file a page under it.
    let res = svc
        .create_page(private_input("otherdoc", Some("container")), &other)
        .await;
    assert!(
        matches!(res, Err(MindPalaceError::AccessDenied(_))),
        "attaching under a container you cannot edit must be denied, got {res:?}"
    );

    // Owner grants `other` only VIEW — still insufficient (attach needs EDIT).
    svc.share_page(
        &slug("container"),
        Principal::User("other@x.com".into()),
        Level::View,
        &owner,
    )
    .await
    .unwrap();
    let res_view = svc
        .create_page(private_input("otherdoc2", Some("container")), &other)
        .await;
    assert!(
        matches!(res_view, Err(MindPalaceError::AccessDenied(_))),
        "View on parent is insufficient to attach a child; Edit required"
    );

    // Grant EDIT → now `other` can attach a child under the container.
    svc.share_page(
        &slug("container"),
        Principal::User("other@x.com".into()),
        Level::Edit,
        &owner,
    )
    .await
    .unwrap();
    assert!(
        svc.create_page(private_input("otherdoc3", Some("container")), &other)
            .await
            .is_ok(),
        "Edit on parent should permit attaching a child"
    );

    // Nonexistent parent → validation error.
    let res_missing = svc
        .create_page(private_input("orphan", Some("does-not-exist")), &owner)
        .await;
    assert!(
        matches!(res_missing, Err(MindPalaceError::Validation(_))),
        "nonexistent parent must be rejected"
    );
}

// --- Criterion 6: traverse (Related) is unchanged; containment is separate ---

#[tokio::test]
async fn traverse_related_unchanged_alongside_containment() {
    let svc = make_service();
    let owner = TenantContext::user("owner@x.com");

    // hub RELATES to spoke (Related edge), and separately contains buried.
    svc.create_page(
        CreatePageInput {
            base_visibility: Some(BaseVisibility::Public),
            ..private_input("hub", None)
        },
        &owner,
    )
    .await
    .unwrap();
    svc.create_page(
        CreatePageInput {
            base_visibility: Some(BaseVisibility::Public),
            ..private_input("spoke", None)
        },
        &owner,
    )
    .await
    .unwrap();
    // Add the Related link hub -> spoke.
    svc.update_page(
        &slug("hub"),
        UpdatePageInput {
            links: Some(vec![slug("spoke")]),
            ..Default::default()
        },
        &owner,
    )
    .await
    .unwrap();
    // A contained child under hub (containment edge hub -> buried).
    svc.create_page(
        CreatePageInput {
            base_visibility: Some(BaseVisibility::Public),
            ..private_input("buried", Some("hub"))
        },
        &owner,
    )
    .await
    .unwrap();

    // Traverse from hub returns outgoing neighbors. The Related neighbor has
    // edge_kind Related; the contained child has edge_kind Child — distinct.
    let neighbors = svc.traverse(&slug("hub"), 2, &owner).await.unwrap();
    let related: Vec<_> = neighbors
        .iter()
        .filter(|n| n.edge_kind == EdgeKind::Related)
        .map(|n| n.slug.as_str().to_string())
        .collect();
    let child: Vec<_> = neighbors
        .iter()
        .filter(|n| n.edge_kind == EdgeKind::Child)
        .map(|n| n.slug.as_str().to_string())
        .collect();
    assert!(
        related.contains(&"spoke".to_string()),
        "Related traversal still surfaces the associated page"
    );
    assert!(
        child.contains(&"buried".to_string()),
        "containment child is a distinct Child edge, not a Related one"
    );
    assert!(
        !related.contains(&"buried".to_string()),
        "containment must not be reported as a Related association"
    );
}

// --- Criterion 1 (group form): a group grant on a parent reaches members ---

#[tokio::test]
async fn group_grant_on_parent_reaches_members_of_descendants() {
    let svc = make_service();
    let owner = TenantContext::user("owner@x.com");
    let member = TenantContext::user("member@x.com");
    let outsider = TenantContext::user("outsider@x.com");

    svc.group_create(GroupId::new("team"), "Team", &owner)
        .await
        .unwrap();
    svc.group_add_member(&GroupId::new("team"), "member@x.com", &owner)
        .await
        .unwrap();

    svc.create_page(private_input("area", None), &owner)
        .await
        .unwrap();
    svc.create_page(private_input("doc", Some("area")), &owner)
        .await
        .unwrap();

    // Group grant on the PARENT `area`.
    svc.share_page(
        &slug("area"),
        Principal::Group(GroupId::new("team")),
        Level::View,
        &owner,
    )
    .await
    .unwrap();

    // Member of the group can read the descendant; outsider cannot.
    assert!(
        svc.read_page(&slug("doc"), ReadLevel::Summary, &member)
            .await
            .is_ok()
    );
    assert!(
        svc.read_page(&slug("doc"), ReadLevel::Summary, &outsider)
            .await
            .is_err()
    );
}

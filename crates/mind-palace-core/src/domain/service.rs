use std::sync::Arc;
use tokio::sync::RwLock;

use super::graph::{GraphNode, KnowledgeGraph, NeighborInfo};
use super::group::Group;
use super::lint::{LintIssue, lint_page};
use super::page::{Page, ReadLevel};
use super::tenant::{GroupMembership, TenantContext};
use super::value_objects::{
    BaseVisibility, EdgeKind, Grant, GroupId, Level, PageAccess, PageType, Principal, Section,
    Slug, Visibility,
};
use crate::error::MindPalaceError;
use crate::ports::changelog::{ChangeAction, ChangelogEntry, ChangelogStore};
use crate::ports::embedding::EmbeddingPort;
use crate::ports::graph::{GraphEdgeData, GraphNodeData, GraphStore};
use crate::ports::group_store::GroupStore;
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
    /// Optional explicit access model (Spec 2). When `None`, access is derived
    /// from `visibility` and the caller's identity (owner = caller).
    pub base_visibility: Option<BaseVisibility>,
}

/// Fields set to `None` are left unchanged. Consumers are encouraged to build
/// this with struct-update syntax (`UpdatePageInput { title: ..., ..Default::default() }`)
/// so that future field additions remain backward compatible.
#[derive(Debug, Default, Clone)]
pub struct UpdatePageInput {
    pub title: Option<String>,
    pub summary: Option<String>,
    pub sections: Option<Vec<Section>>,
    pub links: Option<Vec<Slug>>,
    /// When false (default), provided sections are merged into the existing page
    /// by heading: matching headings are updated in place, new headings appended,
    /// untouched headings preserved. When true, the section list is fully replaced.
    pub replace_sections: bool,
    /// Headings to delete from the page (applied after merge/replace).
    pub delete_sections: Vec<String>,
}

#[derive(Debug, Clone)]
// `Full(Page)` is intentionally the large variant — read_page returns it by
// value on the hot path and boxing would add an allocation + ripple through all
// consumers' match arms for no real benefit (PageResponse is short-lived).
#[allow(clippy::large_enum_variant)]
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
    group_store: Option<Arc<dyn GroupStore>>,
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
            group_store: None,
        }
    }

    /// Attach a changelog store for recording mutations.
    pub fn with_changelog(mut self, store: Arc<dyn ChangelogStore>) -> Self {
        self.changelog = Some(store);
        self
    }

    /// Attach a group store enabling group-based grants and group management
    /// (Spec 2). Without it, group grants resolve to empty membership (only
    /// owner + direct user grants apply).
    pub fn with_group_store(mut self, store: Arc<dyn GroupStore>) -> Self {
        self.group_store = Some(store);
        self
    }

    pub async fn create_page(
        &self,
        input: CreatePageInput,
        ctx: &TenantContext,
    ) -> Result<(Page, Vec<LintIssue>), MindPalaceError> {
        // Resolve the effective access model (Spec 2). The owner is the caller's
        // authenticated identity (email). base_visibility comes from the explicit
        // input when provided, else is inferred from the legacy `visibility`.
        let access = self.access_for_new_page(&input, ctx)?;

        let mut page = Page::new_with_access(
            input.title,
            input.slug,
            input.summary,
            input.sections,
            input.page_type,
            access,
        )
        .map_err(|e| MindPalaceError::Validation(e.to_string()))?;
        page.links = input.links;

        let issues = {
            let g = self.graph.read().await;
            lint_page(&page, Some(&g))
        };

        self.page_store.save_page(&page).await?;

        self.reindex_embedding(&page).await;

        let node_data = self.node_data_for(&page);
        self.graph_store.save_node(&node_data).await?;

        {
            let mut g = self.graph.write().await;
            g.add_node(GraphNode {
                page_id: page.id.clone(),
                slug: page.slug.clone(),
                title: page.title.clone(),
                summary: page.summary.clone(),
                visibility: page.visibility.clone(),
                access: page.access.clone(),
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

        if let Some(ref changelog) = self.changelog {
            let entry = ChangelogEntry {
                timestamp: chrono::Utc::now(),
                slug: page.slug.clone(),
                page_id: page.id.clone(),
                action: ChangeAction::Created,
                agent_id: ctx.user_id().map(|s| s.to_string()),
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
        let ctx = self.resolve_ctx(ctx).await;
        let mut page = self.page_store.get_page_by_slug(slug, &ctx).await?;

        // Spec 2 §3: content mutation requires edit access. The untenanted Global
        // admin and the page owner always pass; grantees need Level::Edit.
        if !ctx.can_edit_page(&page.access) {
            return Err(MindPalaceError::AccessDenied(format!(
                "no edit permission for page '{}'",
                slug.as_str()
            )));
        }

        if let Some(title) = input.title {
            page.title = title;
        }
        if let Some(summary) = input.summary {
            page.summary = summary;
        }
        if let Some(sections) = input.sections {
            if input.replace_sections {
                // Full replace (explicit opt-in).
                page.sections = sections;
            } else {
                // Merge by heading: update matching headings in place, append new
                // ones, preserve untouched headings. This is the safe default so an
                // agent updating one section doesn't wipe the rest of the page.
                for incoming in sections {
                    if let Some(existing) = page
                        .sections
                        .iter_mut()
                        .find(|s| s.heading == incoming.heading)
                    {
                        existing.content = incoming.content;
                    } else {
                        page.sections.push(incoming);
                    }
                }
            }
            page.toc = super::value_objects::TableOfContents::from_sections(&page.sections);
        }
        // Delete requested sections (by heading), applied after merge/replace.
        if !input.delete_sections.is_empty() {
            page.sections
                .retain(|s| !input.delete_sections.contains(&s.heading));
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

        self.reindex_embedding(&page).await;

        // Update graph node
        let node_data = self.node_data_for(&page);
        self.graph_store.save_node(&node_data).await?;

        // Rebuild graph edges from the (possibly updated) links.
        // update_page previously set page.links but never persisted edges,
        // leaving the graph disconnected after link changes.
        {
            let mut g = self.graph.write().await;
            // Refresh in-memory node metadata (title/summary/access) so filtering
            // and traversal reflect the update without a full graph reload.
            if let Some(node) = g.get_node_mut(&page.id) {
                node.title = page.title.clone();
                node.summary = page.summary.clone();
                node.visibility = page.visibility.clone();
                node.access = page.access.clone();
            }
            for link_slug in &page.links {
                if let Some(tid) = self.find_page_id_by_slug(&g, link_slug) {
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

        if let Some(ref changelog) = self.changelog {
            let entry = ChangelogEntry {
                timestamp: chrono::Utc::now(),
                slug: page.slug.clone(),
                page_id: page.id.clone(),
                action: ChangeAction::Updated,
                agent_id: ctx.user_id().map(|s| s.to_string()),
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
        let ctx = self.resolve_ctx(ctx).await;
        // Load without relying on the storage partition as the security boundary,
        // then enforce the authoritative access model (Spec 2 §3). This lets a
        // grantee read an owner's Private page even though it physically lives in
        // the owner's partition. Anonymous/non-grantee callers get PageNotFound.
        let page = self.page_store.get_page_by_slug_unfiltered(slug).await?;
        if page.visibility == Visibility::Archived || !ctx.can_see_page(&page.access) {
            return Err(MindPalaceError::PageNotFound(slug.as_str().to_string()));
        }
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
        let ctx = self.resolve_ctx(ctx).await;
        let embedding = self.embedding.embed_text(query).await?;
        // Over-fetch, then re-filter by the authoritative access model against
        // the in-memory graph (which carries owner/grants). The vector store's
        // partition filter is a coarse pre-filter only.
        let raw = self
            .vector_search
            .search(&embedding, limit.max(limit * 2), &ctx)
            .await?;
        let g = self.graph.read().await;
        let filtered: Vec<SearchResult> = raw
            .into_iter()
            .filter(|r| match g.get_node(&r.page_id) {
                Some(node) => ctx.can_see_page(&node.access),
                // Node not in graph yet (eventual consistency): fall back to the
                // vector-store partition decision already applied.
                None => true,
            })
            .take(limit)
            .collect();
        Ok(filtered)
    }

    pub async fn traverse(
        &self,
        slug: &Slug,
        depth: usize,
        ctx: &TenantContext,
    ) -> Result<Vec<NeighborInfo>, MindPalaceError> {
        let ctx = self.resolve_ctx(ctx).await;
        let page = self.page_store.get_page_by_slug_unfiltered(slug).await?;
        if page.visibility == Visibility::Archived || !ctx.can_see_page(&page.access) {
            return Err(MindPalaceError::PageNotFound(slug.as_str().to_string()));
        }
        let g = self.graph.read().await;
        Ok(g.get_subtree(&page.id, depth, &ctx))
    }

    pub async fn list_pages(
        &self,
        filter: &crate::ports::page_store::PageFilter,
        ctx: &TenantContext,
    ) -> Result<Vec<Page>, MindPalaceError> {
        let ctx = self.resolve_ctx(ctx).await;
        // Reload the graph from the store so long-lived servers (remote MCP) see
        // pages created after startup. Without this, list silently omits pages
        // created by other clients since the process started. Only replace the
        // in-memory graph if the reload returned data — never clobber good state
        // with an empty/failed read.
        if let Ok(data) = self.graph_store.load_graph().await
            && !data.nodes.is_empty()
        {
            let mut g = self.graph.write().await;
            *g = KnowledgeGraph::from_data(data);
        }

        // Use in-memory graph for listing (avoids S3 GetObject per page).
        // Returns lightweight Page stubs with metadata only.
        let g = self.graph.read().await;
        // Default to effectively unbounded; only apply a limit if the caller
        // explicitly set one. Silently capping at a low number made agents
        // conclude pages didn't exist.
        let limit = filter.limit.unwrap_or(usize::MAX);
        let pages: Vec<Page> = g
            .all_nodes(&ctx)
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
                access: node.access.clone(),
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
        let ctx = self.resolve_ctx(ctx).await;
        let page = self.page_store.get_page_by_slug_unfiltered(slug).await?;
        if !ctx.can_edit_page(&page.access) {
            return Err(MindPalaceError::AccessDenied(format!(
                "no edit permission to delete page '{}'",
                slug.as_str()
            )));
        }
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
                agent_id: ctx.user_id().map(|s| s.to_string()),
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
        let ctx = self.resolve_ctx(ctx).await;
        let mut page = self.page_store.get_page_by_slug_unfiltered(slug).await?;
        // Spec 2 §3: archive is an edit-class mutation gated by can_edit.
        if !ctx.can_edit_page(&page.access) {
            return Err(MindPalaceError::AccessDenied(format!(
                "no edit permission to archive page '{}'",
                slug.as_str()
            )));
        }
        page.visibility = Visibility::Archived;
        page.version += 1;
        page.updated_at = chrono::Utc::now();
        self.page_store.save_page(&page).await?;

        // Remove from vector search (won't appear in semantic search)
        self.vector_search.delete_embedding(&page.id).await?;

        // Update graph node visibility so it's filtered from traversal/list.
        // Access is preserved so unarchive can restore correct grants.
        let node_data = crate::ports::graph::GraphNodeData {
            page_id: page.id.clone(),
            slug: page.slug.clone(),
            title: page.title.clone(),
            summary: page.summary.clone(),
            visibility: Visibility::Archived,
            access: page.access.clone(),
            page_type: page.page_type.clone(),
        };
        self.graph_store.save_node(&node_data).await?;

        // Update in-memory graph — remove the node so archived pages are hidden
        // from access-based traversal/list (they no longer match any grant).
        {
            let mut g = self.graph.write().await;
            g.remove_node(&page.id);
        }

        if let Some(ref changelog) = self.changelog {
            let entry = ChangelogEntry {
                timestamp: chrono::Utc::now(),
                slug: page.slug.clone(),
                page_id: page.id.clone(),
                action: ChangeAction::Updated,
                agent_id: ctx.user_id().map(|s| s.to_string()),
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
        // Restore the storage partition from the preserved access model rather
        // than forcing General — a Private page must stay private after restore.
        page.visibility = Visibility::from_access(&page.access);
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
        let node_data = self.node_data_for(&page);
        self.graph_store.save_node(&node_data).await?;

        // Re-add to the in-memory graph (archive removed it).
        {
            let mut g = self.graph.write().await;
            if let Some(node) = g.get_node_mut(&page.id) {
                node.visibility = page.visibility.clone();
                node.access = page.access.clone();
            } else {
                g.add_node(GraphNode {
                    page_id: page.id.clone(),
                    slug: page.slug.clone(),
                    title: page.title.clone(),
                    summary: page.summary.clone(),
                    visibility: page.visibility.clone(),
                    access: page.access.clone(),
                    page_type: page.page_type.clone(),
                });
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

    /// Build the effective [`PageAccess`] for a newly created page (Spec 2 §4).
    ///
    /// - owner = the caller's authenticated identity (email). Anonymous callers
    ///   are denied creation of non-Public pages (and by default cannot create).
    /// - base_visibility comes from explicit input, else inferred from the
    ///   legacy `visibility` (General → Public, otherwise Private).
    /// - a self grant + tenant/group grants are derived from the legacy
    ///   visibility to preserve existing behavior.
    fn access_for_new_page(
        &self,
        input: &CreatePageInput,
        ctx: &TenantContext,
    ) -> Result<PageAccess, MindPalaceError> {
        // Start from the migration mapping of the legacy visibility so tenant/user
        // scoping is preserved, then layer owner + explicit base_visibility.
        let mut access = PageAccess::from_visibility(&input.visibility);

        // Owner is the caller when authenticated.
        if let Some(email) = ctx.user_id() {
            access.owner = Some(email.to_string());
            // Ensure the owner has an explicit self grant for clarity/portability.
            if !access
                .grants
                .iter()
                .any(|g| g.principal == Principal::User(email.to_string()))
            {
                access.grants.push(Grant {
                    principal: Principal::User(email.to_string()),
                    level: Level::Edit,
                });
            }
        }

        // Explicit base_visibility overrides the inferred one.
        if let Some(bv) = input.base_visibility {
            access.base_visibility = bv;
        }

        // Anonymous callers may only create Public pages (Spec 2 §4). The default
        // deployment denies anonymous creation entirely; the MCP layer enforces
        // the deny, and here we defensively reject a non-Public anonymous create.
        if ctx.identity.is_anonymous() && access.base_visibility != BaseVisibility::Public {
            return Err(MindPalaceError::AccessDenied(
                "anonymous callers may only create Public pages".into(),
            ));
        }

        Ok(access)
    }

    /// Assemble graph node data (with access) from a page.
    fn node_data_for(&self, page: &Page) -> GraphNodeData {
        GraphNodeData {
            page_id: page.id.clone(),
            slug: page.slug.clone(),
            title: page.title.clone(),
            summary: page.summary.clone(),
            visibility: page.visibility.clone(),
            access: page.access.clone(),
            page_type: page.page_type.clone(),
        }
    }

    /// Resolve the group membership for the context's identity and return a
    /// context carrying it (Spec 2 §3.3 — resolved once, cached per request).
    /// A no-op (empty membership) when no group store is configured or the
    /// identity is not an authenticated user.
    async fn resolve_ctx(&self, ctx: &TenantContext) -> TenantContext {
        let membership = self.resolve_membership(ctx).await;
        ctx.clone().with_membership(membership)
    }

    async fn resolve_membership(&self, ctx: &TenantContext) -> GroupMembership {
        let Some(store) = &self.group_store else {
            return GroupMembership::empty();
        };
        let Some(email) = ctx.user_id() else {
            return GroupMembership::empty();
        };
        match store.list_groups().await {
            Ok(groups) => GroupMembership::new(
                groups
                    .into_iter()
                    .filter(|g| g.is_member(email))
                    .map(|g| g.id)
                    .collect(),
            ),
            Err(e) => {
                tracing::warn!(error = %e, "failed to resolve group membership; treating as none");
                GroupMembership::empty()
            }
        }
    }

    /// Add a grant to a page (Spec 2 §4 `wiki_share`). Privileged: only the page
    /// owner (or untenanted Global admin) may change grants (§5), which blocks
    /// the Outline-class escalation where a writer grants themselves access.
    pub async fn share_page(
        &self,
        slug: &Slug,
        principal: Principal,
        level: Level,
        ctx: &TenantContext,
    ) -> Result<(), MindPalaceError> {
        let ctx = self.resolve_ctx(ctx).await;
        let mut page = self.page_store.get_page_by_slug_unfiltered(slug).await?;
        if !ctx.can_manage_grants(&page.access) {
            return Err(MindPalaceError::AccessDenied(format!(
                "only the owner may change grants for page '{}'",
                slug.as_str()
            )));
        }
        // Upsert the grant: if the principal already has a grant, raise the level.
        if let Some(existing) = page
            .access
            .grants
            .iter_mut()
            .find(|g| g.principal == principal)
        {
            existing.level = existing.level.max(level);
        } else {
            page.access.grants.push(Grant { principal, level });
        }
        self.persist_access_change(&mut page, &ctx, "Shared").await
    }

    /// Remove a grant from a page (Spec 2 §4 `wiki_unshare`). Owner-only (§5).
    pub async fn unshare_page(
        &self,
        slug: &Slug,
        principal: &Principal,
        ctx: &TenantContext,
    ) -> Result<(), MindPalaceError> {
        let ctx = self.resolve_ctx(ctx).await;
        let mut page = self.page_store.get_page_by_slug_unfiltered(slug).await?;
        if !ctx.can_manage_grants(&page.access) {
            return Err(MindPalaceError::AccessDenied(format!(
                "only the owner may change grants for page '{}'",
                slug.as_str()
            )));
        }
        page.access.grants.retain(|g| &g.principal != principal);
        self.persist_access_change(&mut page, &ctx, "Unshared").await
    }

    /// Persist an access change (share/unshare): bump version, save page, refresh
    /// the graph node's access, re-index the partition, record a changelog entry.
    async fn persist_access_change(
        &self,
        page: &mut Page,
        ctx: &TenantContext,
        action: &str,
    ) -> Result<(), MindPalaceError> {
        page.version += 1;
        page.updated_at = chrono::Utc::now();
        // Partition may shift (e.g. owner set) — recompute from access.
        page.visibility = Visibility::from_access(&page.access);
        self.page_store.save_page(page).await?;

        let node_data = self.node_data_for(page);
        self.graph_store.save_node(&node_data).await?;
        {
            let mut g = self.graph.write().await;
            if let Some(node) = g.get_node_mut(&page.id) {
                node.visibility = page.visibility.clone();
                node.access = page.access.clone();
            }
        }

        if let Some(ref changelog) = self.changelog {
            let entry = ChangelogEntry {
                timestamp: chrono::Utc::now(),
                slug: page.slug.clone(),
                page_id: page.id.clone(),
                action: ChangeAction::Updated,
                agent_id: ctx.user_id().map(|s| s.to_string()),
                summary: Some(action.to_string()),
            };
            changelog.append(&entry).await?;
        }
        Ok(())
    }

    fn require_group_store(&self) -> Result<&Arc<dyn GroupStore>, MindPalaceError> {
        self.group_store
            .as_ref()
            .ok_or_else(|| MindPalaceError::Validation("no group store configured".into()))
    }

    /// Create a group (Spec 2 §4/§5). The caller becomes the sole initial
    /// manager and member. Anonymous callers cannot create groups.
    pub async fn group_create(
        &self,
        id: GroupId,
        name: &str,
        ctx: &TenantContext,
    ) -> Result<Group, MindPalaceError> {
        let store = self.require_group_store()?;
        let creator = ctx.user_id().ok_or_else(|| {
            MindPalaceError::AccessDenied("must be authenticated to create a group".into())
        })?;
        if store.get_group(&id).await?.is_some() {
            return Err(MindPalaceError::Validation(format!(
                "group '{}' already exists",
                id.as_str()
            )));
        }
        let group = Group::new(id, name, creator);
        store.save_group(&group).await?;
        Ok(group)
    }

    /// Add a member to a group (Spec 2 §5). Privileged: the caller MUST be a
    /// manager of the group. A regular member cannot add members — this blocks
    /// self-service escalation.
    pub async fn group_add_member(
        &self,
        id: &GroupId,
        member_email: &str,
        ctx: &TenantContext,
    ) -> Result<Group, MindPalaceError> {
        let store = self.require_group_store()?;
        let caller = ctx.user_id();
        let mut group = store
            .get_group(id)
            .await?
            .ok_or_else(|| MindPalaceError::PageNotFound(format!("group: {}", id.as_str())))?;
        if !self.caller_is_manager(&group, caller, ctx) {
            return Err(MindPalaceError::AccessDenied(format!(
                "only a manager may add members to group '{}'",
                id.as_str()
            )));
        }
        group.add_member(member_email);
        store.save_group(&group).await?;
        Ok(group)
    }

    /// Remove a member from a group (Spec 2 §5). Manager-only.
    pub async fn group_remove_member(
        &self,
        id: &GroupId,
        member_email: &str,
        ctx: &TenantContext,
    ) -> Result<Group, MindPalaceError> {
        let store = self.require_group_store()?;
        let caller = ctx.user_id();
        let mut group = store
            .get_group(id)
            .await?
            .ok_or_else(|| MindPalaceError::PageNotFound(format!("group: {}", id.as_str())))?;
        if !self.caller_is_manager(&group, caller, ctx) {
            return Err(MindPalaceError::AccessDenied(format!(
                "only a manager may remove members from group '{}'",
                id.as_str()
            )));
        }
        group.remove_member(member_email);
        store.save_group(&group).await?;
        Ok(group)
    }

    /// List all groups (Spec 2 §4 `wiki_group_list`).
    pub async fn group_list(&self, _ctx: &TenantContext) -> Result<Vec<Group>, MindPalaceError> {
        let store = self.require_group_store()?;
        store.list_groups().await
    }

    /// A caller may manage a group's membership if they are a manager of it, or
    /// are the untenanted Global admin.
    fn caller_is_manager(&self, group: &Group, caller: Option<&str>, ctx: &TenantContext) -> bool {
        if ctx.tenant_id.is_none() && ctx.identity.is_global() {
            return true;
        }
        matches!(caller, Some(email) if group.is_manager(email))
    }

    /// Best-effort embedding + vector upsert. The page is already durably saved
    /// before this runs, so a failure here must never propagate as an error —
    /// doing so previously caused agents to treat a successful write as failed
    /// and delete data. Failures are logged; the page stays readable and can be
    /// re-indexed later.
    async fn reindex_embedding(&self, page: &Page) {
        let text = page.full_content();
        match self.embedding.embed_text(&text).await {
            Ok(embedding) => {
                let meta = EmbeddingMetadata {
                    page_id: page.id.clone(),
                    slug: page.slug.clone(),
                    title: page.title.clone(),
                    visibility: page.visibility.clone(),
                };
                if let Err(e) = self.vector_search.upsert_embedding(&meta, &embedding).await {
                    tracing::warn!(
                        slug = %page.slug.as_str(), error = %e,
                        "page saved but vector upsert failed; not searchable until reindexed"
                    );
                }
            }
            Err(e) => {
                tracing::warn!(
                    slug = %page.slug.as_str(), error = %e,
                    "page saved but embedding failed; not searchable until reindexed"
                );
            }
        }
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
            base_visibility: None,
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
                    replace_sections: false,
                    delete_sections: Vec::new(),
                },
                &ctx,
            )
            .await
            .unwrap();
        assert_eq!(updated.title, "Advanced Rust");
        assert_eq!(updated.version, 2);
    }

    #[tokio::test]
    async fn update_merges_sections_by_heading() {
        let svc = make_service();
        let ctx = TenantContext::global();
        svc.create_page(sample_input(), &ctx).await.unwrap();
        // sample_input has one section: "Overview"

        // Update the existing "Overview" and add a new "Examples" — "Overview"
        // content changes, "Examples" is appended, nothing is lost.
        let (updated, _) = svc
            .update_page(
                &Slug::new("rust-basics").unwrap(),
                UpdatePageInput {
                    title: None,
                    summary: None,
                    sections: Some(vec![
                        Section {
                            heading: "Overview".into(),
                            content: "Updated overview.".into(),
                        },
                        Section {
                            heading: "Examples".into(),
                            content: "An example.".into(),
                        },
                    ]),
                    links: None,
                    replace_sections: false,
                    delete_sections: Vec::new(),
                },
                &ctx,
            )
            .await
            .unwrap();

        assert_eq!(updated.sections.len(), 2, "merge keeps + adds, not replace");
        let overview = updated
            .sections
            .iter()
            .find(|s| s.heading == "Overview")
            .unwrap();
        assert_eq!(overview.content, "Updated overview.");
        assert!(updated.sections.iter().any(|s| s.heading == "Examples"));
    }

    #[tokio::test]
    async fn update_can_delete_sections() {
        let svc = make_service();
        let ctx = TenantContext::global();
        svc.create_page(sample_input(), &ctx).await.unwrap();
        // Add a second section, then delete the original "Overview".
        svc.update_page(
            &Slug::new("rust-basics").unwrap(),
            UpdatePageInput {
                sections: Some(vec![Section {
                    heading: "Extra".into(),
                    content: "extra".into(),
                }]),
                ..Default::default()
            },
            &ctx,
        )
        .await
        .unwrap();

        let (updated, _) = svc
            .update_page(
                &Slug::new("rust-basics").unwrap(),
                UpdatePageInput {
                    delete_sections: vec!["Overview".into()],
                    ..Default::default()
                },
                &ctx,
            )
            .await
            .unwrap();

        assert!(!updated.sections.iter().any(|s| s.heading == "Overview"));
        assert!(updated.sections.iter().any(|s| s.heading == "Extra"));
    }

    #[tokio::test]
    async fn update_replace_sections_overwrites_all() {
        let svc = make_service();
        let ctx = TenantContext::global();
        svc.create_page(sample_input(), &ctx).await.unwrap();

        let (updated, _) = svc
            .update_page(
                &Slug::new("rust-basics").unwrap(),
                UpdatePageInput {
                    title: None,
                    summary: None,
                    sections: Some(vec![Section {
                        heading: "Only Section".into(),
                        content: "New content.".into(),
                    }]),
                    links: None,
                    replace_sections: true,
                    delete_sections: Vec::new(),
                },
                &ctx,
            )
            .await
            .unwrap();

        assert_eq!(updated.sections.len(), 1);
        assert_eq!(updated.sections[0].heading, "Only Section");
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

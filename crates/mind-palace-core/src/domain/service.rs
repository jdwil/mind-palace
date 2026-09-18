use std::sync::Arc;
use tokio::sync::RwLock;

use super::graph::{GraphNode, KnowledgeGraph, NeighborInfo};
use super::group::Group;
use super::lint::{LintIssue, lint_page};
use super::page::{Page, ReadLevel};
use super::tenant::{GroupMembership, TenantContext};
use super::value_objects::{
    BaseVisibility, EdgeKind, Grant, GroupId, Level, PageAccess, PageType, Principal, SecretRef,
    Section, Slug, Visibility,
};
use crate::error::MindPalaceError;
use crate::ports::changelog::{ChangeAction, ChangelogEntry, ChangelogStore};
use crate::ports::embedding::EmbeddingPort;
use crate::ports::graph::{GraphEdgeData, GraphNodeData, GraphStore};
use crate::ports::group_store::GroupStore;
use crate::ports::page_store::PageStore;
use crate::ports::secrets::{SecretError, SecretValue, SecretsResolver};
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
    /// Optional containment parent (Spec 3). When `Some`, this page is placed
    /// *under* that container and inherits its access grants downward. Setting
    /// a parent requires `can_edit` on BOTH the child and the parent, and must
    /// not create a cycle. Distinct from `links` (which are `Related` edges and
    /// carry no access semantics).
    pub parent: Option<Slug>,
    /// Structured secret references (Spec 4). Opaque backend locators (e.g.
    /// ARNs), NEVER secret values. Each is validated against the configured
    /// backend on create; a value that looks like a raw secret is rejected.
    pub secret_refs: Vec<SecretRef>,
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
    /// Containment parent change (Spec 3). Semantics:
    /// - `None`         → leave the parent unchanged.
    /// - `Some(None)`   → detach: make this page a root (clear its parent).
    /// - `Some(Some(s))`→ reparent under `s`. Requires `can_edit` on both this
    ///   page and `s`, and must not create a cycle.
    pub parent: Option<Option<Slug>>,
    /// Replace the page's secret references (Spec 4). `None` = leave unchanged;
    /// `Some(vec)` = set the full list (validated against the backend on save).
    pub secret_refs: Option<Vec<SecretRef>>,
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

/// Access details for a page plus the requesting caller's own capabilities
/// (Spec 5 §2 `GET /access`). Returned by [`WikiService::page_access`].
///
/// `can_manage`/`can_edit` let a UI hide controls the caller can't use — but
/// the server still enforces every write independently (Spec 5 §5: hiding is
/// UX, not security).
#[derive(Debug, Clone)]
pub struct PageAccessInfo {
    pub access: PageAccess,
    /// Whether the caller may change grants / owner (owner-only, `can_manage_grants`).
    pub can_manage: bool,
    /// Whether the caller may edit the page content (effective-access edit gate).
    pub can_edit: bool,
}

/// A single audit event (Spec 5 §2 `GET /audit`). Projected from the changelog
/// store — grant/membership/secret-resolution mutations all land there with the
/// acting identity. Secret VALUES are never present (only the fact of a
/// resolution attempt and its outcome).
#[derive(Debug, Clone, serde::Serialize)]
pub struct AuditEvent {
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub slug: String,
    pub action: String,
    pub agent_id: Option<String>,
    pub summary: Option<String>,
}

pub struct WikiService {
    page_store: Arc<dyn PageStore>,
    vector_search: Arc<dyn VectorSearchPort>,
    embedding: Arc<dyn EmbeddingPort>,
    graph_store: Arc<dyn GraphStore>,
    graph: Arc<RwLock<KnowledgeGraph>>,
    changelog: Option<Arc<dyn ChangelogStore>>,
    group_store: Option<Arc<dyn GroupStore>>,
    secrets_resolver: Option<Arc<dyn SecretsResolver>>,
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
            secrets_resolver: None,
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

    /// Attach a pluggable secrets backend (Spec 4). Enables reference
    /// validation on create/update and the access-checked, audited
    /// `get_secret` resolution. Without it, resolution is denied
    /// (`SecretError::NotConfigured`) and reference validation is skipped in
    /// lint (the Public-page-with-secrets rule still applies).
    pub fn with_secrets_resolver(mut self, resolver: Arc<dyn SecretsResolver>) -> Self {
        self.secrets_resolver = Some(resolver);
        self
    }

    /// Validate a page's secret references against the configured backend
    /// (Spec 4 §2). Rejects any reference that fails `validate_reference` — the
    /// code-level guardrail that stops a raw secret value being stored as if it
    /// were a locator. A no-op when no backend is configured (references are
    /// stored as-is; resolution will simply be denied later).
    fn validate_secret_refs(&self, page: &Page) -> Result<(), MindPalaceError> {
        let Some(resolver) = &self.secrets_resolver else {
            return Ok(());
        };
        for sref in &page.secret_refs {
            resolver
                .validate_reference(&sref.reference)
                .map_err(|e| match e {
                    SecretError::InvalidReference(msg) => MindPalaceError::Validation(format!(
                        "secret ref '{}' is not a valid backend reference (looks like a raw \
                         secret?): {msg}",
                        sref.name
                    )),
                    other => MindPalaceError::Secret(other),
                })?;
        }
        Ok(())
    }

    /// Resolve a named secret reference on a page to its value (Spec 4 §4).
    ///
    /// This is the SEPARATE channel from `read_page`: the resolved value is
    /// returned ONLY here, never inline in page content. Steps:
    /// 1. Load the page and enforce the SAME view gate as `read_page`
    ///    (`can_see_page` on the page's own access). If you can see the page,
    ///    you can resolve its secret refs (Spec 4 access rule); otherwise the
    ///    result is a clean "not found" (`PageNotFound`), never a raw crash.
    /// 2. Find the [`SecretRef`] named `name`; absent → `PageNotFound` (clean).
    /// 3. Resolve via the pluggable backend; no backend → `NotConfigured`.
    /// 4. Audit-log the attempt (identity, slug, name, success/failure) via the
    ///    changelog hook. The value is never logged.
    ///
    /// The caller (MCP tool) is responsible for attaching the sensitivity
    /// contract (do not echo the value) to the response.
    pub async fn get_secret(
        &self,
        slug: &Slug,
        name: &str,
        ctx: &TenantContext,
    ) -> Result<SecretValue, MindPalaceError> {
        let ctx = self.resolve_ctx(ctx).await;
        let page = self.page_store.get_page_by_slug_unfiltered(slug).await?;

        // Same gate as read_page: archived is invisible; otherwise the page's
        // own access decides. Anonymous / non-grantee callers get a clean
        // PageNotFound (mirrors read) and never learn a secret exists.
        if page.visibility == Visibility::Archived || !ctx.can_see_page(&page.access) {
            return Err(MindPalaceError::PageNotFound(slug.as_str().to_string()));
        }

        let Some(sref) = page.secret_refs.iter().find(|s| s.name == name) else {
            return Err(MindPalaceError::PageNotFound(format!(
                "secret '{name}' on page '{}'",
                slug.as_str()
            )));
        };

        let resolver = self.secrets_resolver.as_ref().ok_or_else(|| {
            MindPalaceError::Secret(SecretError::NotConfigured(
                "no secrets backend configured for this deployment".into(),
            ))
        })?;

        let result = resolver.resolve(&sref.reference).await;
        // Audit every resolution attempt (Spec 4 §4.5), success or failure. The
        // value is NEVER included. Reuses the changelog sink as the audit hook.
        self.audit_secret_resolution(slug, &page.id, name, &ctx, result.is_ok())
            .await;
        Ok(result?)
    }

    /// List a page's secret REFERENCES (names + opaque refs, NEVER values) for
    /// the management UI (Spec 5 §2). Requires that the caller can see the page
    /// (same gate as `read_page`); a stranger gets a clean `PageNotFound`.
    ///
    /// This never resolves anything — resolution stays on the audited
    /// `get_secret` path. The UI manages references, not values.
    pub async fn list_secret_refs(
        &self,
        slug: &Slug,
        ctx: &TenantContext,
    ) -> Result<Vec<SecretRef>, MindPalaceError> {
        let ctx = self.resolve_ctx(ctx).await;
        let page = self.page_store.get_page_by_slug_unfiltered(slug).await?;
        let effective = self.effective_access_for(&page).await;
        if page.visibility == Visibility::Archived || !ctx.can_see_page(&effective) {
            return Err(MindPalaceError::PageNotFound(slug.as_str().to_string()));
        }
        Ok(page.secret_refs.clone())
    }

    /// Attach a secret reference to a page (Spec 5 §2). Goes through
    /// [`update_page`] so it shares the identical edit gate, reference
    /// validation (raw secrets rejected), and lint the MCP path uses. An
    /// existing reference with the same name is replaced (upsert by name).
    pub async fn add_secret_ref(
        &self,
        slug: &Slug,
        secret_ref: SecretRef,
        ctx: &TenantContext,
    ) -> Result<(), MindPalaceError> {
        let page = self.page_store.get_page_by_slug_unfiltered(slug).await?;
        let mut refs = page.secret_refs.clone();
        refs.retain(|r| r.name != secret_ref.name);
        refs.push(secret_ref);
        let input = UpdatePageInput {
            secret_refs: Some(refs),
            ..Default::default()
        };
        self.update_page(slug, input, ctx).await.map(|_| ())
    }

    /// Remove a secret reference by name (Spec 5 §2). Goes through
    /// [`update_page`] (same edit gate). Removing a non-existent name is a no-op.
    pub async fn remove_secret_ref(
        &self,
        slug: &Slug,
        name: &str,
        ctx: &TenantContext,
    ) -> Result<(), MindPalaceError> {
        let page = self.page_store.get_page_by_slug_unfiltered(slug).await?;
        let mut refs = page.secret_refs.clone();
        refs.retain(|r| r.name != name);
        let input = UpdatePageInput {
            secret_refs: Some(refs),
            ..Default::default()
        };
        self.update_page(slug, input, ctx).await.map(|_| ())
    }

    /// Read audit events (Spec 5 §2 `GET /audit`). Projects the changelog store
    /// into a UI-friendly shape, optionally filtered by `resource` (slug
    /// substring) or `subject` (acting identity substring). Returns an empty
    /// list when no changelog store is configured. The caller must be an
    /// authenticated user (audit is an admin/owner-scope view); anonymous
    /// callers get `AccessDenied`.
    ///
    /// Secret VALUES never appear here — the changelog only records the fact and
    /// outcome of a resolution attempt (Spec 4 §4.5).
    pub async fn audit_events(
        &self,
        resource: Option<&str>,
        subject: Option<&str>,
        ctx: &TenantContext,
    ) -> Result<Vec<AuditEvent>, MindPalaceError> {
        if ctx.user_id().is_none() && !ctx.identity.is_global() {
            return Err(MindPalaceError::AccessDenied(
                "authentication required to view audit events".into(),
            ));
        }
        let Some(changelog) = &self.changelog else {
            return Ok(Vec::new());
        };
        // Read from the beginning of time; the store bounds the result.
        let since =
            chrono::DateTime::<chrono::Utc>::from_timestamp(0, 0).unwrap_or_else(chrono::Utc::now);
        let entries = changelog.since(since, Some(500)).await?;
        let events = entries
            .into_iter()
            .filter(|e| match resource {
                Some(r) => e.slug.as_str().contains(r),
                None => true,
            })
            .filter(|e| match subject {
                Some(s) => e
                    .agent_id
                    .as_deref()
                    .map(|a| a.contains(s))
                    .unwrap_or(false),
                None => true,
            })
            .map(|e| AuditEvent {
                timestamp: e.timestamp,
                slug: e.slug.as_str().to_string(),
                action: format!("{:?}", e.action),
                agent_id: e.agent_id,
                summary: e.summary,
            })
            .collect();
        Ok(events)
    }

    /// Record an audited secret-resolution attempt via the changelog hook.
    /// Never records the secret value — only identity, slug, name, outcome.
    async fn audit_secret_resolution(
        &self,
        slug: &Slug,
        page_id: &super::value_objects::PageId,
        name: &str,
        ctx: &TenantContext,
        success: bool,
    ) {
        let identity = ctx.user_id().unwrap_or("<unauthenticated>");
        let outcome = if success { "success" } else { "failure" };
        // Structured trace line is always emitted (works even without a
        // changelog store). DashLX wires its log sink to this.
        tracing::info!(
            target: "mind_palace::secret_audit",
            identity = %identity,
            slug = %slug.as_str(),
            secret = %name,
            outcome = %outcome,
            "secret resolution attempt"
        );
        if let Some(ref changelog) = self.changelog {
            let entry = ChangelogEntry {
                timestamp: chrono::Utc::now(),
                slug: slug.clone(),
                page_id: page_id.clone(),
                action: ChangeAction::Updated,
                agent_id: ctx.user_id().map(|s| s.to_string()),
                summary: Some(format!("SecretResolve name={name} outcome={outcome}")),
            };
            // Best-effort audit: a failed audit write must not deny an otherwise
            // authorized resolution, but it is logged loudly.
            if let Err(e) = changelog.append(&entry).await {
                tracing::warn!(error = %e, "failed to write secret-resolution audit entry");
            }
        }
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

        // Spec 3 §4: setting a parent requires can_edit on the PARENT (the child
        // does not exist yet, so only the parent side is checked here) and must
        // not create a cycle. Resolve group membership for the edit check.
        let parent = input.parent.clone();
        let parent_id = if let Some(parent_slug) = &parent {
            let ctx = self.resolve_ctx(ctx).await;
            Some(self.validate_parent(&input.slug, parent_slug, &ctx).await?)
        } else {
            None
        };

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
        page.parent = parent;
        page.secret_refs = input.secret_refs;

        // Spec 4 §2/§7: reject any secret ref that is not a well-formed backend
        // reference (i.e. looks like a raw secret) BEFORE it can be stored. This
        // is the code-level guardrail, independent of agent instructions.
        self.validate_secret_refs(&page)?;

        let issues = {
            let g = self.graph.read().await;
            lint_page(&page, Some(&g), self.secrets_resolver.as_deref())
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
                parent: page.parent.clone(),
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
            // Spec 3: persist the CONTAINMENT edge distinct from Related. The
            // Child edge points parent → child (used by traversal/inheritance);
            // save_edge also writes the reverse backlink automatically.
            if let Some(pid) = &parent_id {
                let edge = GraphEdgeData {
                    source: pid.clone(),
                    target: page.id.clone(),
                    kind: EdgeKind::Child,
                };
                self.graph_store.save_edge(&edge).await?;
                g.add_edge(pid, &page.id, EdgeKind::Child);
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

        // Spec 2 §3 + Spec 3: content mutation requires edit access. The
        // untenanted Global admin and the page owner always pass; grantees need
        // Level::Edit — including an Edit grant INHERITED from a containment
        // ancestor, so we check against effective access.
        let effective = self.effective_access_for(&page).await;
        if !ctx.can_edit_page(&effective) {
            return Err(MindPalaceError::AccessDenied(format!(
                "no edit permission for page '{}'",
                slug.as_str()
            )));
        }

        // Spec 3 §4: reparenting. `input.parent` is a three-state:
        //   None            → leave parent unchanged
        //   Some(None)      → detach to root
        //   Some(Some(new)) → reparent under `new` (requires can_edit on the new
        //                     parent + no cycle). We record the old/new parent
        //                     ids so the containment edge is re-pointed below.
        let mut reparent: Option<(
            Option<super::value_objects::PageId>,
            Option<super::value_objects::PageId>,
        )> = None;
        if let Some(new_parent) = &input.parent {
            let old_parent_id = {
                let g = self.graph.read().await;
                page.parent
                    .as_ref()
                    .and_then(|s| self.find_page_id_by_slug(&g, s))
            };
            let new_parent_id = match new_parent {
                Some(parent_slug) => Some(self.validate_parent(slug, parent_slug, &ctx).await?),
                None => None,
            };
            page.parent = new_parent.clone();
            reparent = Some((old_parent_id, new_parent_id));
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
        if let Some(secret_refs) = input.secret_refs {
            page.secret_refs = secret_refs;
        }
        // Spec 4 §2/§7: reject raw-secret-looking refs before persisting.
        self.validate_secret_refs(&page)?;
        page.version += 1;
        page.updated_at = chrono::Utc::now();

        let issues = {
            let g = self.graph.read().await;
            lint_page(&page, Some(&g), self.secrets_resolver.as_deref())
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
                node.parent = page.parent.clone();
            }
            // Spec 3: apply a containment edge change if the parent was updated.
            if let Some((old_parent_id, new_parent_id)) = &reparent {
                if let Some(old_pid) = old_parent_id {
                    self.graph_store.delete_edge(old_pid, &page.id).await?;
                    g.remove_edge(old_pid, &page.id);
                }
                if let Some(new_pid) = new_parent_id {
                    let edge = GraphEdgeData {
                        source: new_pid.clone(),
                        target: page.id.clone(),
                        kind: EdgeKind::Child,
                    };
                    self.graph_store.save_edge(&edge).await?;
                    g.add_edge(new_pid, &page.id, EdgeKind::Child);
                }
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
        if page.visibility == Visibility::Archived {
            return Err(MindPalaceError::PageNotFound(slug.as_str().to_string()));
        }
        // Spec 3: enforce against effective access (own grants ∪ inherited from
        // containment ancestors), so a grant on a parent lets the grantee read a
        // descendant. Root pages resolve to their own access (== Spec 2).
        let effective = self.effective_access_for(&page).await;
        if !ctx.can_see_page(&effective) {
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
                Some(node) => {
                    // Spec 3: inherited grants from containment ancestors count.
                    let access = g
                        .effective_access(&node.page_id)
                        .unwrap_or_else(|| node.access.clone());
                    ctx.can_see_page(&access)
                }
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
        if page.visibility == Visibility::Archived {
            return Err(MindPalaceError::PageNotFound(slug.as_str().to_string()));
        }
        let effective = self.effective_access_for(&page).await;
        if !ctx.can_see_page(&effective) {
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
                secret_refs: vec![],
                confidence: super::value_objects::Confidence::default(),
                version: 0,
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
                links: vec![],
                parent: node.parent.clone(),
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
            parent: page.parent.clone(),
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
                    parent: page.parent.clone(),
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

    /// Validate a proposed containment parent for `child_slug` (Spec 3 §4):
    /// - the parent page MUST exist;
    /// - the caller MUST have `can_edit` on the parent (you can't file your page
    ///   under a container you can't edit) — checked against the parent's OWN
    ///   access with the resolved membership;
    /// - the assignment MUST NOT create a cycle.
    ///
    /// `ctx` must already carry resolved group membership (call `resolve_ctx`
    /// first). Returns the parent's `PageId` on success.
    async fn validate_parent(
        &self,
        child_slug: &Slug,
        parent_slug: &Slug,
        ctx: &TenantContext,
    ) -> Result<super::value_objects::PageId, MindPalaceError> {
        // Load the parent authoritatively from the page store (its access is the
        // source of truth for the edit check). Ancestors above the parent may
        // add grants but do not affect can_edit on the parent itself here — we
        // use effective access to honor inherited edit grants on the parent.
        let parent_page = self
            .page_store
            .get_page_by_slug_unfiltered(parent_slug)
            .await
            .map_err(|_| {
                MindPalaceError::Validation(format!(
                    "parent page '{}' does not exist",
                    parent_slug.as_str()
                ))
            })?;

        // Effective access of the parent (its own grants ∪ its ancestors') so an
        // inherited Edit grant also authorizes attaching a child.
        let parent_access = {
            let g = self.graph.read().await;
            g.effective_access_by_slug(parent_slug)
                .unwrap_or_else(|| parent_page.access.clone())
        };
        if !ctx.can_edit_page(&parent_access) {
            return Err(MindPalaceError::AccessDenied(format!(
                "no edit permission on parent page '{}'",
                parent_slug.as_str()
            )));
        }

        // Cycle check against the in-memory containment graph.
        {
            let g = self.graph.read().await;
            if g.would_create_cycle(child_slug, parent_slug) {
                return Err(MindPalaceError::Validation(format!(
                    "setting parent '{}' on '{}' would create a containment cycle",
                    parent_slug.as_str(),
                    child_slug.as_str()
                )));
            }
        }

        Ok(parent_page.id)
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

    /// Resolve the EFFECTIVE access for a loaded page (Spec 3): its own access
    /// unioned with the grants of all containment ancestors, resolved via the
    /// in-memory graph. Falls back to the page's own access when the page is a
    /// root or not yet present in the graph (eventual consistency), so a page
    /// with no parent behaves exactly as in Spec 2 (acceptance criterion 4).
    async fn effective_access_for(&self, page: &Page) -> PageAccess {
        if page.parent.is_none() {
            return page.access.clone();
        }
        let g = self.graph.read().await;
        g.effective_access(&page.id)
            .or_else(|| g.effective_access_by_slug(&page.slug))
            .unwrap_or_else(|| page.access.clone())
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
            parent: page.parent.clone(),
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
        // Spec 4 §6: widening access to a page that gates secret refs widens who
        // can resolve those secrets. Warn loudly so the owner is aware.
        if !page.secret_refs.is_empty() {
            tracing::warn!(
                target: "mind_palace::secret_audit",
                slug = %slug.as_str(),
                secret_refs = page.secret_refs.len(),
                "sharing a page that carries secret references — this widens who can resolve its secrets"
            );
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
        self.persist_access_change(&mut page, &ctx, "Unshared")
            .await
    }

    /// Read a page's access model + the caller's capabilities (Spec 5 §2).
    ///
    /// Requires that the caller can SEE the page (effective access), matching
    /// `read_page` — otherwise a clean `PageNotFound` (never leaks existence).
    /// `can_manage` is owner-only (`can_manage_grants`); `can_edit` uses the
    /// effective (inherited) access, so the UI reflects exactly what the server
    /// will allow.
    pub async fn page_access(
        &self,
        slug: &Slug,
        ctx: &TenantContext,
    ) -> Result<PageAccessInfo, MindPalaceError> {
        let ctx = self.resolve_ctx(ctx).await;
        let page = self.page_store.get_page_by_slug_unfiltered(slug).await?;
        let effective = self.effective_access_for(&page).await;
        if page.visibility == Visibility::Archived || !ctx.can_see_page(&effective) {
            return Err(MindPalaceError::PageNotFound(slug.as_str().to_string()));
        }
        Ok(PageAccessInfo {
            can_manage: ctx.can_manage_grants(&page.access),
            can_edit: ctx.can_edit_page(&effective),
            access: page.access,
        })
    }

    /// Set a page's base visibility and/or owner (Spec 5 §2 `PUT /access`).
    ///
    /// Owner-only, enforced via the SAME `can_manage_grants` gate that
    /// `share_page`/`unshare_page` use (Spec 5 §5). `base_visibility` / `owner`
    /// are each optional: `None` leaves the field unchanged. `owner` is a nested
    /// option so a caller can explicitly clear it (`Some(None)`).
    pub async fn set_page_access(
        &self,
        slug: &Slug,
        base_visibility: Option<BaseVisibility>,
        owner: Option<Option<String>>,
        ctx: &TenantContext,
    ) -> Result<(), MindPalaceError> {
        let ctx = self.resolve_ctx(ctx).await;
        let mut page = self.page_store.get_page_by_slug_unfiltered(slug).await?;
        if !ctx.can_manage_grants(&page.access) {
            return Err(MindPalaceError::AccessDenied(format!(
                "only the owner may change access for page '{}'",
                slug.as_str()
            )));
        }
        if let Some(bv) = base_visibility {
            // Spec 4 §2: a Public page must never gate a secret. Refuse to make a
            // page carrying secret refs public (the lint rule as a hard guard).
            if bv == BaseVisibility::Public && !page.secret_refs.is_empty() {
                return Err(MindPalaceError::Validation(format!(
                    "cannot make page '{}' public while it carries {} secret reference(s)",
                    slug.as_str(),
                    page.secret_refs.len()
                )));
            }
            page.access.base_visibility = bv;
        }
        if let Some(new_owner) = owner {
            page.access.owner = new_owner;
        }
        self.persist_access_change(&mut page, &ctx, "AccessChanged")
            .await
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

    /// Add a manager to a group (Spec 5 §2). Manager-only, mirroring
    /// `group_add_member`: only an existing manager (or untenanted Global admin)
    /// may promote another principal to manager. This shares the SAME
    /// authorization function the membership mutations use, so the web UI and
    /// any MCP tool enforce identical rules (Spec 5 §5). A new manager is also
    /// made a member (see [`Group::add_manager`]).
    pub async fn group_add_manager(
        &self,
        id: &GroupId,
        manager_email: &str,
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
                "only a manager may add managers to group '{}'",
                id.as_str()
            )));
        }
        group.add_manager(manager_email);
        store.save_group(&group).await?;
        Ok(group)
    }

    /// Remove a manager from a group (Spec 5 §2). Manager-only. Refuses to
    /// remove the LAST manager so a group can never become unmanageable
    /// (there would be no principal left able to add members/managers).
    pub async fn group_remove_manager(
        &self,
        id: &GroupId,
        manager_email: &str,
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
                "only a manager may remove managers from group '{}'",
                id.as_str()
            )));
        }
        if group.is_manager(manager_email) && group.managers.len() == 1 {
            return Err(MindPalaceError::Validation(format!(
                "cannot remove the last manager of group '{}'",
                id.as_str()
            )));
        }
        group.remove_manager(manager_email);
        store.save_group(&group).await?;
        Ok(group)
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
            parent: None,
            secret_refs: vec![],
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
                    parent: None,
                    secret_refs: None,
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
                    parent: None,
                    secret_refs: None,
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
                    parent: None,
                    secret_refs: None,
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

//! Web API route handlers (Spec 5 §2).
//!
//! Every handler derives a PER-REQUEST [`TenantContext`] from the authenticated
//! [`Identity`] the auth middleware injected (via the [`Extension`] extractor).
//! There is NO server-wide "current user" — authorization is decided per request
//! by the SAME domain functions the MCP tools call (`can_edit`, manager checks,
//! cycle rejection, reference validation). The client is never trusted; hiding a
//! control in the UI is UX, the server still enforces (Spec 5 §5).

use axum::{
    Extension, Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use mind_palace_core::domain::{
    page::ReadLevel,
    service::{CreatePageInput, UpdatePageInput},
    tenant::{Identity, TenantContext},
    value_objects::{
        BaseVisibility, GroupId, Level, PageType, Principal, SecretRef, Section, Slug, Visibility,
    },
};
use mind_palace_core::error::MindPalaceError;
use mind_palace_core::ports::page_store::PageFilter;

use crate::app::AppState;

/// Turn an authenticated [`Identity`] into a per-request domain context.
fn ctx(identity: Identity) -> TenantContext {
    TenantContext::from_identity(identity)
}

/// Map a domain error to an HTTP status, without leaking which pages exist.
fn map_err(e: MindPalaceError) -> StatusCode {
    match e {
        MindPalaceError::PageNotFound(_) => StatusCode::NOT_FOUND,
        MindPalaceError::AccessDenied(_) => StatusCode::FORBIDDEN,
        MindPalaceError::Validation(_) => StatusCode::BAD_REQUEST,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

// ---------------------------------------------------------------------------
// Page CRUD (existing, now per-request identity)
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct PageSummary {
    slug: String,
    title: String,
    summary: String,
    page_type: PageType,
}

pub async fn list_pages(
    Extension(identity): Extension<Identity>,
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, StatusCode> {
    let ctx = ctx(identity);
    let filter = PageFilter::default();
    let pages = state
        .service
        .list_pages(&filter, &ctx)
        .await
        .map_err(map_err)?;
    let summaries: Vec<PageSummary> = pages
        .into_iter()
        .map(|p| PageSummary {
            slug: p.slug.as_str().to_string(),
            title: p.title,
            summary: p.summary,
            page_type: p.page_type,
        })
        .collect();
    Ok(Json(summaries))
}

pub async fn get_page(
    Extension(identity): Extension<Identity>,
    State(state): State<Arc<AppState>>,
    Path(slug): Path<String>,
) -> Result<impl IntoResponse, StatusCode> {
    let ctx = ctx(identity);
    let slug = Slug::new(&slug).map_err(|_| StatusCode::BAD_REQUEST)?;
    let resp = state
        .service
        .read_page(&slug, ReadLevel::Full, &ctx)
        .await
        .map_err(map_err)?;
    match resp {
        mind_palace_core::domain::service::PageResponse::Full(page) => {
            Ok(Json(serde_json::to_value(page).unwrap_or_default()))
        }
        _ => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

#[derive(Deserialize)]
pub struct SectionInput {
    pub heading: String,
    pub content: String,
}

#[derive(Deserialize)]
pub struct CreatePageRequest {
    pub title: String,
    pub slug: String,
    pub summary: String,
    pub sections: Vec<SectionInput>,
    pub page_type: Option<PageType>,
    pub links: Option<Vec<String>>,
    /// Optional base visibility: "public" | "private" (default private for
    /// authenticated creators, matching wiki_create).
    pub base_visibility: Option<String>,
    pub parent: Option<String>,
}

fn parse_base_visibility(s: Option<&str>) -> Option<BaseVisibility> {
    match s.map(|s| s.trim().to_ascii_lowercase()) {
        Some(v) if v == "public" => Some(BaseVisibility::Public),
        Some(v) if v == "private" || v == "user" => Some(BaseVisibility::Private),
        _ => None,
    }
}

pub async fn create_page(
    Extension(identity): Extension<Identity>,
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreatePageRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    let ctx = ctx(identity);
    let slug = Slug::new(&body.slug).map_err(|_| StatusCode::BAD_REQUEST)?;
    let links = body
        .links
        .unwrap_or_default()
        .into_iter()
        .filter_map(|s| Slug::new(&s).ok())
        .collect();
    let parent = match body.parent {
        Some(p) => Some(Slug::new(&p).map_err(|_| StatusCode::BAD_REQUEST)?),
        None => None,
    };
    let input = CreatePageInput {
        title: body.title,
        slug,
        summary: body.summary,
        sections: body
            .sections
            .into_iter()
            .map(|s| Section {
                heading: s.heading,
                content: s.content,
            })
            .collect(),
        page_type: body.page_type.unwrap_or(PageType::Concept),
        visibility: Visibility::General,
        links,
        base_visibility: parse_base_visibility(body.base_visibility.as_deref()),
        parent,
        secret_refs: vec![],
    };
    let (page, _issues) = state
        .service
        .create_page(input, &ctx)
        .await
        .map_err(map_err)?;
    Ok((
        StatusCode::CREATED,
        Json(serde_json::to_value(page).unwrap_or_default()),
    ))
}

#[derive(Deserialize)]
pub struct UpdatePageRequest {
    pub title: Option<String>,
    pub summary: Option<String>,
    pub sections: Option<Vec<SectionInput>>,
    pub links: Option<Vec<String>>,
    /// Web editor sends the full page, so this defaults to true (replace).
    pub replace_sections: Option<bool>,
}

pub async fn update_page(
    Extension(identity): Extension<Identity>,
    State(state): State<Arc<AppState>>,
    Path(slug): Path<String>,
    Json(body): Json<UpdatePageRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    let ctx = ctx(identity);
    let slug = Slug::new(&slug).map_err(|_| StatusCode::BAD_REQUEST)?;
    let input = UpdatePageInput {
        title: body.title,
        summary: body.summary,
        sections: body.sections.map(|ss| {
            ss.into_iter()
                .map(|s| Section {
                    heading: s.heading,
                    content: s.content,
                })
                .collect()
        }),
        links: body
            .links
            .map(|ls| ls.into_iter().filter_map(|s| Slug::new(&s).ok()).collect()),
        replace_sections: body.replace_sections.unwrap_or(true),
        delete_sections: Vec::new(),
        parent: None,
        secret_refs: None,
    };
    let (page, _issues) = state
        .service
        .update_page(&slug, input, &ctx)
        .await
        .map_err(map_err)?;
    Ok(Json(serde_json::to_value(page).unwrap_or_default()))
}

pub async fn delete_page(
    Extension(identity): Extension<Identity>,
    State(state): State<Arc<AppState>>,
    Path(slug): Path<String>,
) -> Result<impl IntoResponse, StatusCode> {
    let ctx = ctx(identity);
    let slug = Slug::new(&slug).map_err(|_| StatusCode::BAD_REQUEST)?;
    state
        .service
        .delete_page(&slug, &ctx)
        .await
        .map_err(map_err)?;
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// Graph + search (existing)
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct GraphResponse {
    nodes: Vec<GraphNode>,
    edges: Vec<GraphEdge>,
}

#[derive(Serialize)]
struct GraphNode {
    slug: String,
    title: String,
    page_type: PageType,
}

#[derive(Serialize)]
struct GraphEdge {
    source: String,
    target: String,
    kind: String,
}

pub async fn get_graph(
    Extension(identity): Extension<Identity>,
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, StatusCode> {
    let ctx = ctx(identity);
    let filter = PageFilter::default();
    let pages = state
        .service
        .list_pages(&filter, &ctx)
        .await
        .map_err(map_err)?;

    let nodes: Vec<GraphNode> = pages
        .iter()
        .map(|p| GraphNode {
            slug: p.slug.as_str().to_string(),
            title: p.title.clone(),
            page_type: p.page_type.clone(),
        })
        .collect();

    let mut edges = Vec::new();
    for page in &pages {
        let neighbors = state
            .service
            .traverse(&page.slug, 1, &ctx)
            .await
            .unwrap_or_default();
        for n in neighbors {
            edges.push(GraphEdge {
                source: page.slug.as_str().to_string(),
                target: n.slug.as_str().to_string(),
                kind: format!("{:?}", n.edge_kind),
            });
        }
    }

    Ok(Json(GraphResponse { nodes, edges }))
}

#[derive(Deserialize)]
pub struct SearchQuery {
    pub q: String,
}

pub async fn search(
    Extension(identity): Extension<Identity>,
    State(state): State<Arc<AppState>>,
    Query(params): Query<SearchQuery>,
) -> Result<impl IntoResponse, StatusCode> {
    let ctx = ctx(identity);
    let results = state
        .service
        .search(&params.q, &ctx, 20)
        .await
        .map_err(map_err)?;
    Ok(Json(serde_json::to_value(results).unwrap_or_default()))
}

// ---------------------------------------------------------------------------
// Page access + grants (Spec 5 §2, Spec 2)
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct GrantView {
    principal_type: String,
    principal: String,
    level: String,
}

#[derive(Serialize)]
struct AccessView {
    slug: String,
    owner: Option<String>,
    base_visibility: String,
    grants: Vec<GrantView>,
    /// Whether the requesting identity may change grants (owner-only). Lets the
    /// UI hide controls; the server still enforces on every write.
    can_manage: bool,
    /// Whether the requesting identity may edit the page's content.
    can_edit: bool,
}

fn grant_view(g: &mind_palace_core::domain::value_objects::Grant) -> GrantView {
    let (principal_type, principal) = match &g.principal {
        Principal::User(email) => ("user".to_string(), email.clone()),
        Principal::Group(id) => ("group".to_string(), id.as_str().to_string()),
    };
    GrantView {
        principal_type,
        principal,
        level: match g.level {
            Level::View => "view".to_string(),
            Level::Edit => "edit".to_string(),
        },
    }
}

fn base_visibility_str(b: BaseVisibility) -> String {
    match b {
        BaseVisibility::Public => "public".to_string(),
        BaseVisibility::Private => "private".to_string(),
    }
}

/// GET /api/pages/{slug}/access — owner, base_visibility, resolved grants, and
/// the caller's own capabilities. Requires that the caller can SEE the page
/// (otherwise 404, never leaking existence).
pub async fn get_access(
    Extension(identity): Extension<Identity>,
    State(state): State<Arc<AppState>>,
    Path(slug): Path<String>,
) -> Result<impl IntoResponse, StatusCode> {
    let ctx = ctx(identity);
    let slug = Slug::new(&slug).map_err(|_| StatusCode::BAD_REQUEST)?;
    let access = state
        .service
        .page_access(&slug, &ctx)
        .await
        .map_err(map_err)?;
    let view = AccessView {
        slug: slug.as_str().to_string(),
        owner: access.access.owner.clone(),
        base_visibility: base_visibility_str(access.access.base_visibility),
        grants: access.access.grants.iter().map(grant_view).collect(),
        can_manage: access.can_manage,
        can_edit: access.can_edit,
    };
    Ok(Json(view))
}

#[derive(Deserialize)]
pub struct SetAccessRequest {
    /// "public" | "private"
    pub base_visibility: Option<String>,
    /// Set/clear the page owner (owner-only operation).
    pub owner: Option<Option<String>>,
}

/// PUT /api/pages/{slug}/access — set base_visibility and/or owner. Owner-only
/// (enforced in the service via `can_manage_grants`).
pub async fn set_access(
    Extension(identity): Extension<Identity>,
    State(state): State<Arc<AppState>>,
    Path(slug): Path<String>,
    Json(body): Json<SetAccessRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    let ctx = ctx(identity);
    let slug = Slug::new(&slug).map_err(|_| StatusCode::BAD_REQUEST)?;
    let base_visibility = match body.base_visibility.as_deref() {
        Some(_) => Some(
            parse_base_visibility(body.base_visibility.as_deref())
                .ok_or(StatusCode::BAD_REQUEST)?,
        ),
        None => None,
    };
    state
        .service
        .set_page_access(&slug, base_visibility, body.owner, &ctx)
        .await
        .map_err(map_err)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub struct GrantRequest {
    /// "user" | "group"
    pub principal_type: String,
    pub principal: String,
    /// "view" | "edit" (for POST). Ignored for DELETE.
    pub level: Option<String>,
}

fn parse_principal(principal_type: &str, principal: &str) -> Result<Principal, StatusCode> {
    match principal_type.trim().to_ascii_lowercase().as_str() {
        "user" => Ok(Principal::User(principal.to_string())),
        "group" => Ok(Principal::Group(GroupId::new(principal.to_string()))),
        _ => Err(StatusCode::BAD_REQUEST),
    }
}

fn parse_level(s: Option<&str>) -> Level {
    match s.map(|s| s.trim().to_ascii_lowercase()) {
        Some(v) if v == "edit" => Level::Edit,
        _ => Level::View,
    }
}

/// POST /api/pages/{slug}/grants — add/raise a grant. Owner-only (service).
pub async fn add_grant(
    Extension(identity): Extension<Identity>,
    State(state): State<Arc<AppState>>,
    Path(slug): Path<String>,
    Json(body): Json<GrantRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    let ctx = ctx(identity);
    let slug = Slug::new(&slug).map_err(|_| StatusCode::BAD_REQUEST)?;
    let principal = parse_principal(&body.principal_type, &body.principal)?;
    let level = parse_level(body.level.as_deref());
    state
        .service
        .share_page(&slug, principal, level, &ctx)
        .await
        .map_err(map_err)?;
    Ok(StatusCode::NO_CONTENT)
}

/// DELETE /api/pages/{slug}/grants — remove a grant. Owner-only (service).
pub async fn remove_grant(
    Extension(identity): Extension<Identity>,
    State(state): State<Arc<AppState>>,
    Path(slug): Path<String>,
    Json(body): Json<GrantRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    let ctx = ctx(identity);
    let slug = Slug::new(&slug).map_err(|_| StatusCode::BAD_REQUEST)?;
    let principal = parse_principal(&body.principal_type, &body.principal)?;
    state
        .service
        .unshare_page(&slug, &principal, &ctx)
        .await
        .map_err(map_err)?;
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// Groups (Spec 5 §2, Spec 2)
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct GroupView {
    id: String,
    name: String,
    members: Vec<String>,
    managers: Vec<String>,
    /// Whether the requesting identity may mutate this group's membership/
    /// managers (a manager of the group, or the global admin). Lets the UI hide
    /// controls; the server still enforces every mutation independently.
    can_manage: bool,
}

fn group_view(g: mind_palace_core::domain::group::Group, ctx: &TenantContext) -> GroupView {
    // Mirror the domain's caller_is_manager gate: global admin, or a manager.
    let can_manage = (ctx.tenant_id.is_none() && ctx.identity.is_global())
        || ctx
            .user_id()
            .map(|email| g.managers.iter().any(|m| m == email))
            .unwrap_or(false);
    GroupView {
        id: g.id.as_str().to_string(),
        name: g.name,
        members: g.members,
        managers: g.managers,
        can_manage,
    }
}

pub async fn list_groups(
    Extension(identity): Extension<Identity>,
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, StatusCode> {
    let ctx = ctx(identity);
    let groups = state.service.group_list(&ctx).await.map_err(map_err)?;
    let views: Vec<GroupView> = groups.into_iter().map(|g| group_view(g, &ctx)).collect();
    Ok(Json(views))
}

#[derive(Deserialize)]
pub struct CreateGroupRequest {
    pub id: String,
    pub name: String,
}

pub async fn create_group(
    Extension(identity): Extension<Identity>,
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateGroupRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    let ctx = ctx(identity);
    let group = state
        .service
        .group_create(GroupId::new(body.id), &body.name, &ctx)
        .await
        .map_err(map_err)?;
    Ok((StatusCode::CREATED, Json(group_view(group, &ctx))))
}

pub async fn get_group(
    Extension(identity): Extension<Identity>,
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, StatusCode> {
    let ctx = ctx(identity);
    // group_list is the read path we have; find the requested id.
    let groups = state.service.group_list(&ctx).await.map_err(map_err)?;
    let group = groups
        .into_iter()
        .find(|g| g.id == GroupId::new(id.clone()))
        .ok_or(StatusCode::NOT_FOUND)?;
    Ok(Json(group_view(group, &ctx)))
}

#[derive(Deserialize)]
pub struct GroupMemberRequest {
    pub email: String,
}

pub async fn add_group_member(
    Extension(identity): Extension<Identity>,
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<GroupMemberRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    let ctx = ctx(identity);
    let group = state
        .service
        .group_add_member(&GroupId::new(id), &body.email, &ctx)
        .await
        .map_err(map_err)?;
    Ok(Json(group_view(group, &ctx)))
}

pub async fn remove_group_member(
    Extension(identity): Extension<Identity>,
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<GroupMemberRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    let ctx = ctx(identity);
    let group = state
        .service
        .group_remove_member(&GroupId::new(id), &body.email, &ctx)
        .await
        .map_err(map_err)?;
    Ok(Json(group_view(group, &ctx)))
}

pub async fn add_group_manager(
    Extension(identity): Extension<Identity>,
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<GroupMemberRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    let ctx = ctx(identity);
    let group = state
        .service
        .group_add_manager(&GroupId::new(id), &body.email, &ctx)
        .await
        .map_err(map_err)?;
    Ok(Json(group_view(group, &ctx)))
}

pub async fn remove_group_manager(
    Extension(identity): Extension<Identity>,
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<GroupMemberRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    let ctx = ctx(identity);
    let group = state
        .service
        .group_remove_manager(&GroupId::new(id), &body.email, &ctx)
        .await
        .map_err(map_err)?;
    Ok(Json(group_view(group, &ctx)))
}

// ---------------------------------------------------------------------------
// Hierarchy (Spec 5 §2, Spec 3)
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct SetParentRequest {
    /// The parent slug to set, or null to detach to root.
    pub parent: Option<String>,
}

/// PUT /api/pages/{slug}/parent — set or clear the containment parent. Enforces
/// can_edit on BOTH child and parent and rejects cycles (in the service).
pub async fn set_parent(
    Extension(identity): Extension<Identity>,
    State(state): State<Arc<AppState>>,
    Path(slug): Path<String>,
    Json(body): Json<SetParentRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    let ctx = ctx(identity);
    let slug = Slug::new(&slug).map_err(|_| StatusCode::BAD_REQUEST)?;
    // Three-state Option<Option<Slug>>: Some(Some) reparent, Some(None) detach.
    let parent = match body.parent {
        Some(p) => Some(Some(Slug::new(&p).map_err(|_| StatusCode::BAD_REQUEST)?)),
        None => Some(None),
    };
    let input = UpdatePageInput {
        title: None,
        summary: None,
        sections: None,
        links: None,
        replace_sections: false,
        delete_sections: Vec::new(),
        parent,
        secret_refs: None,
    };
    state
        .service
        .update_page(&slug, input, &ctx)
        .await
        .map_err(map_err)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize)]
struct TreeNode {
    slug: String,
    title: String,
    page_type: PageType,
    edge_kind: String,
}

/// GET /api/pages/{slug}/tree — the containment subtree (Child edges) for
/// display, distinct from the associative Related graph. Filtered by access.
pub async fn get_tree(
    Extension(identity): Extension<Identity>,
    State(state): State<Arc<AppState>>,
    Path(slug): Path<String>,
) -> Result<impl IntoResponse, StatusCode> {
    let ctx = ctx(identity);
    let slug = Slug::new(&slug).map_err(|_| StatusCode::BAD_REQUEST)?;
    let neighbors = state
        .service
        .traverse(&slug, 5, &ctx)
        .await
        .map_err(map_err)?;
    // Only containment edges form the tree; Related edges are excluded here.
    let tree: Vec<TreeNode> = neighbors
        .into_iter()
        .filter(|n| {
            matches!(
                n.edge_kind,
                mind_palace_core::domain::value_objects::EdgeKind::Child
                    | mind_palace_core::domain::value_objects::EdgeKind::Parent
            )
        })
        .map(|n| TreeNode {
            slug: n.slug.as_str().to_string(),
            title: n.title,
            page_type: n.page_type,
            edge_kind: format!("{:?}", n.edge_kind),
        })
        .collect();
    Ok(Json(tree))
}

// ---------------------------------------------------------------------------
// Secret references (Spec 5 §2, Spec 4) — names + refs only, NEVER values
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct SecretRefView {
    name: String,
    reference: String,
}

/// GET /api/pages/{slug}/secret-refs — list references (names + opaque refs).
/// There is deliberately NO value-reveal endpoint here (Spec 5 §2).
pub async fn list_secret_refs(
    Extension(identity): Extension<Identity>,
    State(state): State<Arc<AppState>>,
    Path(slug): Path<String>,
) -> Result<impl IntoResponse, StatusCode> {
    let ctx = ctx(identity);
    let slug = Slug::new(&slug).map_err(|_| StatusCode::BAD_REQUEST)?;
    let refs = state
        .service
        .list_secret_refs(&slug, &ctx)
        .await
        .map_err(map_err)?;
    let views: Vec<SecretRefView> = refs
        .into_iter()
        .map(|r| SecretRefView {
            name: r.name,
            reference: r.reference,
        })
        .collect();
    Ok(Json(views))
}

#[derive(Deserialize)]
pub struct AddSecretRefRequest {
    pub name: String,
    pub reference: String,
}

/// POST /api/pages/{slug}/secret-refs — attach a reference. Validated in the
/// service (a raw-secret-looking value is rejected). Requires edit access.
pub async fn add_secret_ref(
    Extension(identity): Extension<Identity>,
    State(state): State<Arc<AppState>>,
    Path(slug): Path<String>,
    Json(body): Json<AddSecretRefRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    let ctx = ctx(identity);
    let slug = Slug::new(&slug).map_err(|_| StatusCode::BAD_REQUEST)?;
    state
        .service
        .add_secret_ref(&slug, SecretRef::new(body.name, body.reference), &ctx)
        .await
        .map_err(map_err)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub struct RemoveSecretRefRequest {
    pub name: String,
}

/// DELETE /api/pages/{slug}/secret-refs — remove a reference by name. Edit access.
pub async fn remove_secret_ref(
    Extension(identity): Extension<Identity>,
    State(state): State<Arc<AppState>>,
    Path(slug): Path<String>,
    Json(body): Json<RemoveSecretRefRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    let ctx = ctx(identity);
    let slug = Slug::new(&slug).map_err(|_| StatusCode::BAD_REQUEST)?;
    state
        .service
        .remove_secret_ref(&slug, &body.name, &ctx)
        .await
        .map_err(map_err)?;
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// Audit (Spec 5 §2) — read-only view of change events
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct AuditQuery {
    pub subject: Option<String>,
    pub resource: Option<String>,
}

/// GET /api/audit — grant/membership/secret-resolution events (owner/admin
/// scope). Low priority per the spec; backed by the changelog store when
/// configured, otherwise an empty list.
pub async fn get_audit(
    Extension(identity): Extension<Identity>,
    State(state): State<Arc<AppState>>,
    Query(params): Query<AuditQuery>,
) -> Result<impl IntoResponse, StatusCode> {
    let ctx = ctx(identity);
    let events = state
        .service
        .audit_events(params.resource.as_deref(), params.subject.as_deref(), &ctx)
        .await
        .map_err(map_err)?;
    Ok(Json(events))
}

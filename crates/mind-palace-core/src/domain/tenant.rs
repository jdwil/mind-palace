use serde::{Deserialize, Serialize};

use super::value_objects::{TenantId, Visibility};

/// Who is making a request, independent of tenant scoping.
///
/// Identity is the generic authentication result threaded from a transport
/// (the remote MCP server's auth middleware, or the stdio binary's env config)
/// into the domain. It deliberately knows nothing about *how* it was proven
/// (OIDC/JWT, shared token, env var) — only the resulting principal.
///
/// Authorization semantics differ per variant:
/// - `Global`  — trusted, unauthenticated single-user/admin context. Sees
///   everything (subject to tenant scoping). This is the historical behavior of
///   `TenantContext::global()` and is what the local stdio binary uses so its
///   single-user experience is unchanged.
/// - `Anonymous` — an unauthenticated *remote* request. May see ONLY
///   fully-open (unrestricted / `General`) pages. Any restriction (tenant or
///   user scope) is invisible. This makes a no-auth remote deployment safe to
///   expose public knowledge only.
/// - `User { id }` — an authenticated principal, keyed on a stable identity
///   string (email preferred). Enables per-user visibility.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Identity {
    /// Trusted local/admin context — sees everything (subject to tenant rules).
    Global,
    /// Unauthenticated remote request — sees only fully-open pages.
    Anonymous,
    /// An authenticated principal, keyed on a stable id string (email preferred).
    User { id: String },
}

impl Identity {
    /// The stable identity string, if this is an authenticated user.
    pub fn user_id(&self) -> Option<&str> {
        match self {
            Identity::User { id } => Some(id.as_str()),
            _ => None,
        }
    }

    /// True for the trusted, everything-visible local/admin context.
    pub fn is_global(&self) -> bool {
        matches!(self, Identity::Global)
    }

    /// True for an unauthenticated remote request (public-only).
    pub fn is_anonymous(&self) -> bool {
        matches!(self, Identity::Anonymous)
    }
}

/// Context representing who is making the request and what they can see.
///
/// `identity` is the authenticated principal (see [`Identity`]) and is the
/// primary authorization input. `tenant_id`/`visible_tenants` layer optional
/// multi-tenant scoping on top of it (existing behavior; later specs build
/// grants/groups here).
///
/// `visible_tenants` contains all tenant IDs whose pages this context can access.
/// For a parent tenant (e.g. a parent org), this includes itself + all child
/// tenant IDs. For a leaf tenant, this is just that tenant. When tenancy is
/// disabled, `tenant_id` is None.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TenantContext {
    pub identity: Identity,
    pub tenant_id: Option<TenantId>,
    pub visible_tenants: Vec<TenantId>,
}

impl TenantContext {
    /// Trusted, unauthenticated single-user/admin context — tenancy disabled and
    /// everything visible. Used by the local stdio binary and tests.
    pub fn global() -> Self {
        Self {
            identity: Identity::Global,
            tenant_id: None,
            visible_tenants: Vec::new(),
        }
    }

    /// An unauthenticated remote request that may see ONLY fully-open pages.
    pub fn anonymous() -> Self {
        Self {
            identity: Identity::Anonymous,
            tenant_id: None,
            visible_tenants: Vec::new(),
        }
    }

    /// An authenticated user context (no tenant scoping) keyed on a stable id.
    pub fn user(id: impl Into<String>) -> Self {
        Self {
            identity: Identity::User { id: id.into() },
            tenant_id: None,
            visible_tenants: Vec::new(),
        }
    }

    /// Build a context directly from an [`Identity`] (no tenant scoping).
    pub fn from_identity(identity: Identity) -> Self {
        Self {
            identity,
            tenant_id: None,
            visible_tenants: Vec::new(),
        }
    }

    /// A tenant that can only see its own pages + General.
    pub fn leaf(tenant_id: TenantId) -> Self {
        let visible = vec![tenant_id.clone()];
        Self {
            identity: Identity::Global,
            tenant_id: Some(tenant_id),
            visible_tenants: visible,
        }
    }

    /// A parent tenant that can see its own + all descendant tenant pages.
    pub fn parent(tenant_id: TenantId, descendants: Vec<TenantId>) -> Self {
        let mut visible = vec![tenant_id.clone()];
        visible.extend(descendants);
        Self {
            identity: Identity::Global,
            tenant_id: Some(tenant_id),
            visible_tenants: visible,
        }
    }

    /// Attach a user identity to this context (builder-style).
    ///
    /// Sets `identity` to `Identity::User { id }`. Preserves any tenant scoping
    /// already set on the context.
    pub fn with_user(mut self, user_id: impl Into<String>) -> Self {
        self.identity = Identity::User {
            id: user_id.into(),
        };
        self
    }

    /// Attach an explicit identity to this context (builder-style), preserving
    /// tenant scoping.
    pub fn with_identity(mut self, identity: Identity) -> Self {
        self.identity = identity;
        self
    }

    /// Returns the user ID if this context's identity is an authenticated user.
    pub fn user_id(&self) -> Option<&str> {
        self.identity.user_id()
    }

    /// Check if this context can see a page with the given visibility.
    ///
    /// Rules:
    /// - `Visibility::Archived` → NEVER visible through normal operations.
    /// - `Identity::Anonymous` → visible ONLY for fully-open (`General`) pages;
    ///   any restricted page is invisible.
    /// - `Identity::Global` with NO tenant scoping → sees everything (the
    ///   historical single-user/admin behavior).
    /// - Otherwise tenant/user scoping is enforced:
    ///   - `General` → visible to everyone (except anonymous handled above).
    ///   - `Tenant(tid)` → visible if `tid` is in `visible_tenants`.
    ///   - `User(uid)` → visible if the context's user id matches, OR the
    ///     context is an untenanted `Global` admin.
    pub fn can_see(&self, visibility: &Visibility) -> bool {
        // Archived pages are never visible through normal operations.
        if matches!(visibility, Visibility::Archived) {
            return false;
        }

        // Anonymous remote requests may see ONLY fully-open pages.
        if self.identity.is_anonymous() {
            return matches!(visibility, Visibility::General);
        }

        // Untenanted trusted/admin context sees everything (except archived).
        if self.tenant_id.is_none() && self.identity.is_global() {
            return true;
        }

        match visibility {
            Visibility::Archived => false,
            // General pages are visible to any non-anonymous context.
            Visibility::General => true,
            // Tenant-scoped pages visible if the tenant is in the visible set.
            Visibility::Tenant(page_tenant) => self.visible_tenants.contains(page_tenant),
            // User-scoped pages visible only if the user id matches.
            Visibility::User(page_user) => self.user_id() == Some(page_user.as_str()),
        }
    }
}

impl Default for TenantContext {
    fn default() -> Self {
        Self::global()
    }
}

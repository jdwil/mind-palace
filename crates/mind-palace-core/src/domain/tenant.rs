use serde::{Deserialize, Serialize};

use super::value_objects::{TenantId, Visibility};

/// Context representing who is making the request and what they can see.
///
/// `visible_tenants` contains all tenant IDs whose pages this context can access.
/// For a parent tenant (DashLX), this includes itself + all child tenant IDs.
/// For a leaf tenant (ClientA), this is just [ClientA].
/// When tenancy is disabled, `tenant_id` is None and everything is visible.
///
/// `user_id` optionally identifies the specific user making the request,
/// enabling user-scoped visibility (pages only that user can see).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TenantContext {
    pub tenant_id: Option<TenantId>,
    pub visible_tenants: Vec<TenantId>,
    pub user_id: Option<String>,
}

impl TenantContext {
    /// Tenancy disabled — everything is visible.
    pub fn global() -> Self {
        Self {
            tenant_id: None,
            visible_tenants: Vec::new(),
            user_id: None,
        }
    }

    /// A tenant that can only see its own pages + General.
    pub fn leaf(tenant_id: TenantId) -> Self {
        let visible = vec![tenant_id.clone()];
        Self {
            tenant_id: Some(tenant_id),
            visible_tenants: visible,
            user_id: None,
        }
    }

    /// A parent tenant that can see its own + all descendant tenant pages.
    pub fn parent(tenant_id: TenantId, descendants: Vec<TenantId>) -> Self {
        let mut visible = vec![tenant_id.clone()];
        visible.extend(descendants);
        Self {
            tenant_id: Some(tenant_id),
            visible_tenants: visible,
            user_id: None,
        }
    }

    /// Attach a user identity to this context (builder-style).
    pub fn with_user(mut self, user_id: impl Into<String>) -> Self {
        self.user_id = Some(user_id.into());
        self
    }

    /// Returns the user ID if one is set on this context.
    pub fn user_id(&self) -> Option<&str> {
        self.user_id.as_deref()
    }

    /// Check if this context can see a page with the given visibility.
    ///
    /// Rules:
    /// - `Visibility::Archived` → NEVER visible through normal operations
    /// - `Visibility::General` → visible to ALL contexts
    /// - `Visibility::Tenant(tid)` → visible to that tenant + parent tenants + global
    /// - `Visibility::User(uid)` → visible to that user + global (global sees everything)
    pub fn can_see(&self, visibility: &Visibility) -> bool {
        match (&self.tenant_id, visibility) {
            // Archived pages are never visible through normal operations
            (_, Visibility::Archived) => false,
            // Global context sees everything (except archived)
            (None, _) => true,
            // General pages are visible to everyone
            (_, Visibility::General) => true,
            // Tenant-scoped pages visible if tenant is in visible_tenants list
            (Some(_), Visibility::Tenant(page_tenant)) => {
                self.visible_tenants.contains(page_tenant)
            }
            // User-scoped pages visible only if user_id matches
            (Some(_), Visibility::User(page_user)) => {
                self.user_id.as_deref() == Some(page_user.as_str())
            }
        }
    }
}

impl Default for TenantContext {
    fn default() -> Self {
        Self::global()
    }
}

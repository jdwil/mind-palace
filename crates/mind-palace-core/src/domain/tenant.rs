use serde::{Deserialize, Serialize};

use super::value_objects::{TenantId, Visibility};

/// Context representing who is making the request and what they can see.
///
/// `visible_tenants` contains all tenant IDs whose pages this context can access.
/// For a parent tenant (DashLX), this includes itself + all child tenant IDs.
/// For a leaf tenant (ClientA), this is just [ClientA].
/// When tenancy is disabled, `tenant_id` is None and everything is visible.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TenantContext {
    pub tenant_id: Option<TenantId>,
    pub visible_tenants: Vec<TenantId>,
}

impl TenantContext {
    /// Tenancy disabled — everything is visible.
    pub fn global() -> Self {
        Self {
            tenant_id: None,
            visible_tenants: Vec::new(),
        }
    }

    /// A tenant that can only see its own pages + General.
    pub fn leaf(tenant_id: TenantId) -> Self {
        let visible = vec![tenant_id.clone()];
        Self {
            tenant_id: Some(tenant_id),
            visible_tenants: visible,
        }
    }

    /// A parent tenant that can see its own + all descendant tenant pages.
    pub fn parent(tenant_id: TenantId, descendants: Vec<TenantId>) -> Self {
        let mut visible = vec![tenant_id.clone()];
        visible.extend(descendants);
        Self {
            tenant_id: Some(tenant_id),
            visible_tenants: visible,
        }
    }

    /// Check if this context can see a page with the given visibility.
    pub fn can_see(&self, visibility: &Visibility) -> bool {
        match (&self.tenant_id, visibility) {
            (None, _) => true,
            (_, Visibility::General) => true,
            (Some(_), Visibility::Tenant(page_tenant)) => {
                self.visible_tenants.contains(page_tenant)
            }
        }
    }
}

impl Default for TenantContext {
    fn default() -> Self {
        Self::global()
    }
}

use serde::{Deserialize, Serialize};

use super::value_objects::{
    BaseVisibility, GroupId, Level, PageAccess, Principal, TenantId, Visibility,
};

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

/// The set of group ids an identity belongs to, resolved once and cached for the
/// duration of a request (Spec 2 §3.3: "Resolve group membership via the group
/// store (cache per request)").
///
/// The domain authorization functions ([`TenantContext::can_see_access`],
/// [`TenantContext::can_edit`]) take this by reference so they stay pure and
/// synchronous — the async group-store fetch happens once in the service layer.
#[derive(Debug, Clone, Default)]
pub struct GroupMembership {
    groups: Vec<GroupId>,
}

impl GroupMembership {
    /// Build from the group ids the current identity is a member of.
    pub fn new(groups: Vec<GroupId>) -> Self {
        Self { groups }
    }

    /// An empty membership (no groups) — used for anonymous/global contexts and
    /// in tests where group grants are not exercised.
    pub fn empty() -> Self {
        Self { groups: Vec::new() }
    }

    /// True if the identity belongs to `group`.
    pub fn contains(&self, group: &GroupId) -> bool {
        self.groups.iter().any(|g| g == group)
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
    /// Group ids the identity belongs to, resolved per request by the service
    /// layer (Spec 2 §3.3). Empty for anonymous/global and in most tests.
    #[serde(skip, default)]
    pub membership: GroupMembership,
}

impl TenantContext {
    /// Trusted, unauthenticated single-user/admin context — tenancy disabled and
    /// everything visible. Used by the local stdio binary and tests.
    pub fn global() -> Self {
        Self {
            identity: Identity::Global,
            tenant_id: None,
            visible_tenants: Vec::new(),
            membership: GroupMembership::empty(),
        }
    }

    /// An unauthenticated remote request that may see ONLY fully-open pages.
    pub fn anonymous() -> Self {
        Self {
            identity: Identity::Anonymous,
            tenant_id: None,
            visible_tenants: Vec::new(),
            membership: GroupMembership::empty(),
        }
    }

    /// An authenticated user context (no tenant scoping) keyed on a stable id.
    pub fn user(id: impl Into<String>) -> Self {
        Self {
            identity: Identity::User { id: id.into() },
            tenant_id: None,
            visible_tenants: Vec::new(),
            membership: GroupMembership::empty(),
        }
    }

    /// Build a context directly from an [`Identity`] (no tenant scoping).
    pub fn from_identity(identity: Identity) -> Self {
        Self {
            identity,
            tenant_id: None,
            visible_tenants: Vec::new(),
            membership: GroupMembership::empty(),
        }
    }

    /// A tenant that can only see its own pages + General.
    pub fn leaf(tenant_id: TenantId) -> Self {
        let visible = vec![tenant_id.clone()];
        Self {
            identity: Identity::Global,
            tenant_id: Some(tenant_id),
            visible_tenants: visible,
            membership: GroupMembership::empty(),
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
            membership: GroupMembership::empty(),
        }
    }

    /// Attach a user identity to this context (builder-style).
    ///
    /// Sets `identity` to `Identity::User { id }`. Preserves any tenant scoping
    /// already set on the context.
    pub fn with_user(mut self, user_id: impl Into<String>) -> Self {
        self.identity = Identity::User { id: user_id.into() };
        self
    }

    /// Attach an explicit identity to this context (builder-style), preserving
    /// tenant scoping.
    pub fn with_identity(mut self, identity: Identity) -> Self {
        self.identity = identity;
        self
    }

    /// Attach the resolved group membership for this request (builder-style).
    pub fn with_membership(mut self, membership: GroupMembership) -> Self {
        self.membership = membership;
        self
    }

    /// Convenience: view check using this context's own resolved membership.
    /// Used by the storage/graph filtering paths that only have a `&TenantContext`.
    pub fn can_see_page(&self, access: &PageAccess) -> bool {
        self.can_see_access(access, &self.membership)
    }

    /// Convenience: edit check using this context's own resolved membership.
    pub fn can_edit_page(&self, access: &PageAccess) -> bool {
        self.can_edit(access, &self.membership)
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

    /// Authorization check against the Spec 2 [`PageAccess`] model (owner +
    /// base_visibility + grants), with group membership resolved by the caller.
    ///
    /// This is the authoritative view check. It is a strict superset of the
    /// legacy [`can_see`]: `Public` behaves like `General`, and `Private` is
    /// visible only to the owner, a `User` grantee, or a member of a `Group`
    /// grantee. Archived state is tracked separately (storage `Visibility`), so
    /// it is not represented here.
    ///
    /// Rules (Spec 2 §3):
    /// 1. `Anonymous`  → visible iff `base_visibility == Public`.
    /// 2. `Global` (untenanted admin/local) → sees everything.
    /// 3. `User{email}`:
    ///    - owner → visible (view + edit).
    ///    - `Public` → visible.
    ///    - a `User(email)` grant, or a `Group(g)` grant where the identity is a
    ///      member of `g` → visible.
    ///    - else → not visible.
    pub fn can_see_access(&self, access: &PageAccess, membership: &GroupMembership) -> bool {
        // Anonymous: only fully-open pages.
        if self.identity.is_anonymous() {
            return access.base_visibility == BaseVisibility::Public;
        }

        // Untenanted trusted/admin context sees everything.
        if self.tenant_id.is_none() && self.identity.is_global() {
            return true;
        }

        // Public pages are visible to any non-anonymous context.
        if access.base_visibility == BaseVisibility::Public {
            return true;
        }

        // Private: owner or a matching grant.
        if let Some(email) = self.user_id() {
            if access.is_owner(email) {
                return true;
            }
            return access
                .grants
                .iter()
                .any(|g| Self::grant_matches(&g.principal, email, membership));
        }

        // A tenanted Global context (parent/leaf) with no user id falls back to
        // tenant-group grants: a Group grant whose id is a visible tenant.
        access.grants.iter().any(|g| match &g.principal {
            Principal::Group(gid) => {
                membership.contains(gid) || self.visible_tenants.iter().any(|t| t.0 == gid.0)
            }
            Principal::User(_) => false,
        })
    }

    /// Whether this identity may EDIT the page (Spec 2 §3 `can_edit`).
    ///
    /// - `Anonymous` → never.
    /// - `Global` (untenanted admin) → always.
    /// - owner → always.
    /// - a `User(email)` grant with `Level::Edit`, or a `Group(g)` Edit grant
    ///   where the identity is a member of `g`.
    ///   `Level::Edit` implies view; a View-only grant does NOT grant edit.
    pub fn can_edit(&self, access: &PageAccess, membership: &GroupMembership) -> bool {
        if self.identity.is_anonymous() {
            return false;
        }
        if self.tenant_id.is_none() && self.identity.is_global() {
            return true;
        }
        if let Some(email) = self.user_id() {
            if access.is_owner(email) {
                return true;
            }
            return access.grants.iter().any(|g| {
                g.level == Level::Edit && Self::grant_matches(&g.principal, email, membership)
            });
        }
        false
    }

    /// Whether this identity may change the grants of a page (share/unshare) —
    /// the privileged mutation from Spec 2 §5. v1 rule: owner only (plus the
    /// untenanted `Global` admin). A non-owner cannot add a grant, which blocks
    /// the Outline-class self-escalation.
    pub fn can_manage_grants(&self, access: &PageAccess) -> bool {
        if self.tenant_id.is_none() && self.identity.is_global() {
            return true;
        }
        match self.user_id() {
            Some(email) => access.is_owner(email),
            None => false,
        }
    }

    fn grant_matches(principal: &Principal, email: &str, membership: &GroupMembership) -> bool {
        match principal {
            Principal::User(u) => u == email,
            Principal::Group(g) => membership.contains(g),
        }
    }
}

impl Default for TenantContext {
    fn default() -> Self {
        Self::global()
    }
}

#[cfg(test)]
mod access_tests {
    use super::*;
    use crate::domain::value_objects::{
        BaseVisibility, Grant, GroupId, Level, PageAccess, Principal, Visibility,
    };

    fn public() -> PageAccess {
        PageAccess {
            owner: Some("owner@x.com".into()),
            base_visibility: BaseVisibility::Public,
            grants: vec![],
        }
    }

    fn private_owned() -> PageAccess {
        PageAccess {
            owner: Some("owner@x.com".into()),
            base_visibility: BaseVisibility::Private,
            grants: vec![],
        }
    }

    // --- Criterion 2: Public pages remain visible to everyone incl. anonymous ---

    #[test]
    fn anonymous_sees_public_not_private() {
        let ctx = TenantContext::anonymous();
        let none = GroupMembership::empty();
        assert!(ctx.can_see_access(&public(), &none));
        assert!(!ctx.can_see_access(&private_owned(), &none));
        assert!(!ctx.can_edit(&public(), &none));
    }

    // --- Criterion 1: Private page visible to owner, user grantee, group member ---

    #[test]
    fn owner_sees_and_edits_private() {
        let ctx = TenantContext::user("owner@x.com");
        let none = GroupMembership::empty();
        assert!(ctx.can_see_access(&private_owned(), &none));
        assert!(ctx.can_edit(&private_owned(), &none));
    }

    #[test]
    fn non_owner_without_grant_cannot_see_private() {
        let ctx = TenantContext::user("stranger@x.com");
        let none = GroupMembership::empty();
        assert!(!ctx.can_see_access(&private_owned(), &none));
        assert!(!ctx.can_edit(&private_owned(), &none));
    }

    #[test]
    fn user_grantee_sees_private() {
        let mut access = private_owned();
        access.grants.push(Grant {
            principal: Principal::User("grantee@x.com".into()),
            level: Level::View,
        });
        let ctx = TenantContext::user("grantee@x.com");
        let none = GroupMembership::empty();
        assert!(ctx.can_see_access(&access, &none));
        // View grant does not confer edit (criterion 3).
        assert!(!ctx.can_edit(&access, &none));
    }

    #[test]
    fn group_member_sees_private_via_group_grant() {
        let gid = GroupId::new("eng");
        let mut access = private_owned();
        access.grants.push(Grant {
            principal: Principal::Group(gid.clone()),
            level: Level::View,
        });
        let ctx = TenantContext::user("member@x.com");
        let member_of = GroupMembership::new(vec![gid.clone()]);
        let not_member = GroupMembership::empty();
        assert!(ctx.can_see_access(&access, &member_of));
        assert!(!ctx.can_see_access(&access, &not_member));
    }

    // --- Criterion 3: Edit gating; Edit implies View; View-only cannot edit ---

    #[test]
    fn edit_grant_confers_edit_and_view() {
        let mut access = private_owned();
        access.grants.push(Grant {
            principal: Principal::User("editor@x.com".into()),
            level: Level::Edit,
        });
        let ctx = TenantContext::user("editor@x.com");
        let none = GroupMembership::empty();
        assert!(ctx.can_see_access(&access, &none));
        assert!(ctx.can_edit(&access, &none));
    }

    #[test]
    fn group_edit_grant_confers_edit_to_member() {
        let gid = GroupId::new("writers");
        let mut access = private_owned();
        access.grants.push(Grant {
            principal: Principal::Group(gid.clone()),
            level: Level::Edit,
        });
        let ctx = TenantContext::user("w@x.com");
        let member = GroupMembership::new(vec![gid]);
        assert!(ctx.can_edit(&access, &member));
    }

    // --- Criterion 4 (partial): only owner may manage grants ---

    #[test]
    fn only_owner_manages_grants() {
        let access = private_owned();
        assert!(TenantContext::user("owner@x.com").can_manage_grants(&access));
        assert!(!TenantContext::user("stranger@x.com").can_manage_grants(&access));
        // A view+edit grantee still cannot manage grants (no self-escalation).
        let mut with_edit = private_owned();
        with_edit.grants.push(Grant {
            principal: Principal::User("editor@x.com".into()),
            level: Level::Edit,
        });
        assert!(!TenantContext::user("editor@x.com").can_manage_grants(&with_edit));
        // Untenanted global admin can manage.
        assert!(TenantContext::global().can_manage_grants(&access));
    }

    #[test]
    fn global_admin_sees_and_edits_everything() {
        let ctx = TenantContext::global();
        let none = GroupMembership::empty();
        assert!(ctx.can_see_access(&private_owned(), &none));
        assert!(ctx.can_edit(&private_owned(), &none));
    }

    // --- Criterion 6: migration mapping has no increase in openness ---

    #[test]
    fn migration_general_to_public() {
        let a = PageAccess::from_visibility(&Visibility::General);
        assert_eq!(a.base_visibility, BaseVisibility::Public);
        assert!(a.owner.is_none());
        assert!(a.grants.is_empty());
    }

    #[test]
    fn migration_user_to_private_owner_grant() {
        let a = PageAccess::from_visibility(&Visibility::User("alice@x.com".into()));
        assert_eq!(a.base_visibility, BaseVisibility::Private);
        assert_eq!(a.owner.as_deref(), Some("alice@x.com"));
        assert_eq!(
            a.grants,
            vec![Grant {
                principal: Principal::User("alice@x.com".into()),
                level: Level::View
            }]
        );
        let none = GroupMembership::empty();
        assert!(!TenantContext::user("bob@x.com").can_see_access(&a, &none));
        assert!(TenantContext::user("alice@x.com").can_see_access(&a, &none));
    }

    #[test]
    fn migration_tenant_to_private_group_grant() {
        let a = PageAccess::from_visibility(&Visibility::Tenant(
            crate::domain::value_objects::TenantId::new("acme"),
        ));
        assert_eq!(a.base_visibility, BaseVisibility::Private);
        match &a.grants[..] {
            [
                Grant {
                    principal: Principal::Group(g),
                    level: Level::View,
                },
            ] => assert_eq!(g.as_str(), "acme"),
            _ => panic!("expected a single group view grant"),
        }
    }

    #[test]
    fn partition_projection_roundtrips_privacy() {
        assert_eq!(Visibility::from_access(&public()), Visibility::General);
        assert_eq!(
            Visibility::from_access(&private_owned()),
            Visibility::User("owner@x.com".into())
        );
    }
}

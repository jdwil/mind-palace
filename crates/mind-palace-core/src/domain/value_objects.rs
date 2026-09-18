use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PageId(pub Uuid);

impl PageId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for PageId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Slug(String);

impl Slug {
    pub fn new(value: &str) -> Result<Self, SlugError> {
        if value.is_empty() {
            return Err(SlugError::Empty);
        }
        if !value
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        {
            return Err(SlugError::InvalidChars);
        }
        if value.starts_with('-') || value.ends_with('-') {
            return Err(SlugError::InvalidFormat);
        }
        Ok(Self(value.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum SlugError {
    #[error("slug cannot be empty")]
    Empty,
    #[error("slug must contain only lowercase ascii, digits, or hyphens")]
    InvalidChars,
    #[error("slug cannot start or end with a hyphen")]
    InvalidFormat,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TenantId(pub String);

impl TenantId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

/// Legacy single-axis visibility, retained as the STORAGE + MIGRATION bridge.
///
/// Spec 2 moves authorization onto [`PageAccess`] (owner + base_visibility +
/// grants). `Visibility` is kept because it still drives *physical partitioning*
/// (S3 object key segment, vector-store filter partition) and preserves existing
/// multi-tenant deployments (a tenant is modelled as a group — see §6 of Spec 2).
/// It is derived from a page's [`PageAccess`] on save via [`Visibility::from_access`]
/// and migrated forward on load via [`PageAccess::from_visibility`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Visibility {
    General,
    Tenant(TenantId),
    User(String),
    Archived,
}

/// Identifier for a [`Group`](crate::domain::group::Group). Generic string key
/// (a slug-like name is fine); DashLX "teams" are just groups keyed here.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct GroupId(pub String);

impl GroupId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Base openness of a page, replacing the "role" the old `Visibility` enum played.
///
/// - `Public`  — fully open; visible to everyone including anonymous requests.
/// - `Private` — visible only to the owner + explicit grants (+ inherited, Spec 3).
///
/// `Archived` is NOT a base visibility; it remains a separate soft-delete state
/// tracked via the storage-level [`Visibility::Archived`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum BaseVisibility {
    Public,
    #[default]
    Private,
}

/// A principal a grant can be issued to: an individual user (by email) or a group.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Principal {
    /// An individual user keyed on a stable id string (email preferred).
    User(String),
    /// A group; members of the group inherit the grant.
    Group(GroupId),
}

/// Permission level. `Edit` implies `View`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Level {
    View,
    Edit,
}

/// A single access grant: a principal is allowed a level of access to a page.
///
/// Tuple-compatible with ReBAC `(resource, relation, subject)` for a future
/// migration to an external authz engine (see Spec 2 §2.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grant {
    pub principal: Principal,
    pub level: Level,
}

/// Full page-centric access model (Spec 2 §2.1).
///
/// - `owner`  — creator/owner (email). Always has view+edit and is the only
///   principal who may change grants by default.
/// - `base_visibility` — Public or Private.
/// - `grants` — explicit grants to users/groups.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageAccess {
    pub owner: Option<String>,
    pub base_visibility: BaseVisibility,
    pub grants: Vec<Grant>,
}

impl Default for PageAccess {
    fn default() -> Self {
        // A page with no explicit access defaults to Public with no owner — the
        // historical "General" behavior. Creation paths set an owner + Private
        // where appropriate.
        Self {
            owner: None,
            base_visibility: BaseVisibility::Public,
            grants: Vec::new(),
        }
    }
}

impl PageAccess {
    /// Migrate a legacy [`Visibility`] into a [`PageAccess`] with NO increase in
    /// openness (Spec 2 §2.1, acceptance criterion 6):
    /// - `General`   → Public, no owner, no grants.
    /// - `User(x)`   → Private, owner = x, grant(User(x), View).
    /// - `Tenant(t)` → Private, grant(Group(t-as-group), View). Owner unknown.
    /// - `Archived`  → Private (the Archived storage state is tracked separately).
    pub fn from_visibility(visibility: &Visibility) -> Self {
        match visibility {
            Visibility::General => Self {
                owner: None,
                base_visibility: BaseVisibility::Public,
                grants: Vec::new(),
            },
            Visibility::User(uid) => Self {
                owner: Some(uid.clone()),
                base_visibility: BaseVisibility::Private,
                grants: vec![Grant {
                    principal: Principal::User(uid.clone()),
                    level: Level::View,
                }],
            },
            Visibility::Tenant(tid) => Self {
                owner: None,
                base_visibility: BaseVisibility::Private,
                grants: vec![Grant {
                    principal: Principal::Group(GroupId::new(tid.0.clone())),
                    level: Level::View,
                }],
            },
            // Archived is a soft-delete storage state; access is Private by
            // default. The storage-level Visibility::Archived still hides it.
            Visibility::Archived => Self {
                owner: None,
                base_visibility: BaseVisibility::Private,
                grants: Vec::new(),
            },
        }
    }

    /// True if this principal string is the owner.
    pub fn is_owner(&self, email: &str) -> bool {
        self.owner.as_deref() == Some(email)
    }
}

impl Visibility {
    /// Derive the STORAGE-level partition [`Visibility`] from a [`PageAccess`].
    ///
    /// This keeps physical partitioning (S3 key segment, vector filter) working
    /// while access is authoritatively decided by `PageAccess`. Mapping:
    /// - `Public`  → `General` (open partition).
    /// - `Private` with an owner → `User(owner)` (owner's partition).
    /// - `Private` with a single `Group` grant and no owner → `Tenant(group)`
    ///   (preserves the tenant partition for migrated tenant pages).
    /// - `Private` otherwise → `User(owner-or-empty)` / `General` fallback.
    ///
    /// Note: this is a lossy projection used only for partitioning. The full
    /// grant list is persisted separately (frontmatter + graph node) and is what
    /// authorization actually consults.
    pub fn from_access(access: &PageAccess) -> Self {
        match access.base_visibility {
            BaseVisibility::Public => Visibility::General,
            BaseVisibility::Private => {
                if let Some(owner) = &access.owner {
                    Visibility::User(owner.clone())
                } else if let Some(Grant {
                    principal: Principal::Group(gid),
                    ..
                }) = access.grants.first()
                {
                    Visibility::Tenant(TenantId::new(gid.0.clone()))
                } else {
                    // Private with no owner and no group grant: fall back to a
                    // stable non-general partition so it is not world-open on disk.
                    Visibility::User(String::new())
                }
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PageType {
    Index,
    Concept,
    Entity,
    Decision,
    Leaf,
    Sop,
    Skill,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Section {
    pub heading: String,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TableOfContents {
    pub entries: Vec<TocEntry>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TocEntry {
    pub title: String,
    pub anchor: String,
    pub depth: u8,
}

impl TableOfContents {
    pub fn from_sections(sections: &[Section]) -> Self {
        let entries = sections
            .iter()
            .map(|s| TocEntry {
                title: s.heading.clone(),
                anchor: s.heading.to_lowercase().replace(' ', "-"),
                depth: 1,
            })
            .collect();
        Self { entries }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum EdgeKind {
    Parent,
    Child,
    Related,
    Backlink,
}

/// Confidence score for a page (0.0 to 1.0).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Confidence(f32);

impl Confidence {
    pub fn new(value: f32) -> Option<Self> {
        if (0.0..=1.0).contains(&value) {
            Some(Self(value))
        } else {
            None
        }
    }

    pub fn value(&self) -> f32 {
        self.0
    }
}

impl Default for Confidence {
    fn default() -> Self {
        Self(0.5)
    }
}

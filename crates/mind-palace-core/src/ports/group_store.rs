use async_trait::async_trait;

use crate::domain::group::Group;
use crate::domain::value_objects::GroupId;
use crate::error::MindPalaceError;

/// Persistence port for [`Group`]s (Spec 2 §2.2, §7).
///
/// Backed by DynamoDB in production (new item type in the existing table, e.g.
/// `PK=GROUP#<id>`). Membership lookups must be fast; the domain layer caches
/// per request via [`crate::domain::tenant::GroupMembership`].
#[async_trait]
pub trait GroupStore: Send + Sync {
    /// Fetch a single group, or `None` if it does not exist.
    async fn get_group(&self, id: &GroupId) -> Result<Option<Group>, MindPalaceError>;

    /// List all groups. Used to resolve the set of groups an identity belongs to.
    async fn list_groups(&self) -> Result<Vec<Group>, MindPalaceError>;

    /// Create or replace a group.
    async fn save_group(&self, group: &Group) -> Result<(), MindPalaceError>;

    /// Delete a group by id.
    async fn delete_group(&self, id: &GroupId) -> Result<(), MindPalaceError>;
}

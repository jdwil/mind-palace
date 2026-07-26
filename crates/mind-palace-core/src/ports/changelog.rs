use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::domain::value_objects::{PageId, Slug};
use crate::error::MindPalaceError;

/// The action that was performed on a page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChangeAction {
    Created,
    Updated,
    Deleted,
}

/// A single changelog entry recording a wiki mutation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangelogEntry {
    pub timestamp: DateTime<Utc>,
    pub slug: Slug,
    pub page_id: PageId,
    pub action: ChangeAction,
    pub agent_id: Option<String>,
    pub summary: Option<String>,
}

/// Port trait for persisting and querying changelog entries.
#[async_trait]
pub trait ChangelogStore: Send + Sync {
    /// Append a new entry to the changelog.
    async fn append(&self, entry: &ChangelogEntry) -> Result<(), MindPalaceError>;

    /// Query entries since a given timestamp, ordered by time ascending.
    async fn since(
        &self,
        since: DateTime<Utc>,
        limit: Option<usize>,
    ) -> Result<Vec<ChangelogEntry>, MindPalaceError>;

    /// Delete all entries older than the given timestamp (used after dreaming).
    async fn prune_before(&self, before: DateTime<Utc>) -> Result<u64, MindPalaceError>;
}

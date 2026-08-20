use async_trait::async_trait;

use crate::domain::page::Page;
use crate::domain::tenant::TenantContext;
use crate::domain::value_objects::{PageId, PageType, Slug, Visibility};
use crate::error::MindPalaceError;

#[derive(Debug, Clone, Default)]
pub struct PageFilter {
    pub page_type: Option<PageType>,
    pub visibility: Option<Visibility>,
    pub limit: Option<usize>,
}

#[async_trait]
pub trait PageStore: Send + Sync {
    async fn get_page(&self, id: &PageId, ctx: &TenantContext) -> Result<Page, MindPalaceError>;

    async fn get_page_by_slug(
        &self,
        slug: &Slug,
        ctx: &TenantContext,
    ) -> Result<Page, MindPalaceError>;

    /// Read a page by slug without visibility filtering.
    /// Used for operations that need to access pages regardless of visibility state
    /// (e.g., unarchiving an archived page).
    async fn get_page_by_slug_unfiltered(&self, slug: &Slug) -> Result<Page, MindPalaceError>;

    async fn save_page(&self, page: &Page) -> Result<(), MindPalaceError>;

    async fn delete_page(&self, id: &PageId) -> Result<(), MindPalaceError>;

    async fn list_pages(
        &self,
        filter: &PageFilter,
        ctx: &TenantContext,
    ) -> Result<Vec<Page>, MindPalaceError>;
}

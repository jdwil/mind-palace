use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::value_objects::{
    Confidence, PageAccess, PageId, PageType, Section, Slug, TableOfContents, Visibility,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Page {
    pub id: PageId,
    pub slug: Slug,
    pub title: String,
    pub summary: String,
    pub toc: TableOfContents,
    pub sections: Vec<Section>,
    pub page_type: PageType,
    /// STORAGE-level partition (see [`Visibility`]). Derived from `access` on
    /// save; kept in sync so physical partitioning and legacy filtering work.
    pub visibility: Visibility,
    /// Authoritative access model (Spec 2): owner + base_visibility + grants.
    #[serde(default)]
    pub access: PageAccess,
    pub confidence: Confidence,
    pub version: u32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub links: Vec<Slug>,
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum PageValidationError {
    #[error("summary is required")]
    MissingSummary,
    #[error("title is required")]
    MissingTitle,
    #[error("at least one section is required")]
    NoSections,
}

impl Page {
    pub fn new(
        title: String,
        slug: Slug,
        summary: String,
        sections: Vec<Section>,
        page_type: PageType,
        visibility: Visibility,
    ) -> Result<Self, PageValidationError> {
        // Legacy constructor: derive the Spec 2 access model from the passed
        // storage visibility so old callers keep working with migrated semantics.
        let access = PageAccess::from_visibility(&visibility);
        Self::new_with_access(title, slug, summary, sections, page_type, access)
    }

    /// Create a page with an explicit [`PageAccess`] (Spec 2). The storage-level
    /// `visibility` partition is derived from `access`.
    pub fn new_with_access(
        title: String,
        slug: Slug,
        summary: String,
        sections: Vec<Section>,
        page_type: PageType,
        access: PageAccess,
    ) -> Result<Self, PageValidationError> {
        if title.is_empty() {
            return Err(PageValidationError::MissingTitle);
        }
        if summary.is_empty() {
            return Err(PageValidationError::MissingSummary);
        }
        if sections.is_empty() {
            return Err(PageValidationError::NoSections);
        }

        let toc = TableOfContents::from_sections(&sections);
        let now = Utc::now();
        let visibility = Visibility::from_access(&access);

        Ok(Self {
            id: PageId::new(),
            slug,
            title,
            summary,
            toc,
            sections,
            page_type,
            visibility,
            access,
            confidence: Confidence::default(),
            version: 1,
            created_at: now,
            updated_at: now,
            links: Vec::new(),
        })
    }

    pub fn summary_text(&self) -> &str {
        &self.summary
    }

    pub fn section_by_heading(&self, heading: &str) -> Option<&Section> {
        self.sections.iter().find(|s| s.heading == heading)
    }

    pub fn full_content(&self) -> String {
        let mut out = format!("# {}\n\n{}\n\n", self.title, self.summary);
        for section in &self.sections {
            out.push_str(&format!(
                "## {}\n\n{}\n\n",
                section.heading, section.content
            ));
        }
        out
    }
}

/// What level of detail to return when reading a page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadLevel {
    Summary,
    Section(String),
    Full,
}

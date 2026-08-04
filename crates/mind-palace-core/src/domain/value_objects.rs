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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Visibility {
    General,
    Tenant(TenantId),
    User(String),
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

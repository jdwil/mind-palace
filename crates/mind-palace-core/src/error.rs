use crate::domain::value_objects::SlugError;

#[derive(Debug, thiserror::Error)]
pub enum MindPalaceError {
    #[error("page not found: {0}")]
    PageNotFound(String),

    #[error("access denied: {0}")]
    AccessDenied(String),

    #[error("validation error: {0}")]
    Validation(String),

    #[error("slug error: {0}")]
    Slug(#[from] SlugError),

    #[error("store error: {0}")]
    Store(String),

    #[error("embedding error: {0}")]
    Embedding(String),

    #[error("graph error: {0}")]
    Graph(String),

    #[error("serialization error: {0}")]
    Serialization(String),
}

use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use axum_extra::extract::CookieJar;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use mind_palace_core::domain::{
    page::ReadLevel,
    service::{CreatePageInput, UpdatePageInput},
    value_objects::{PageType, Section, Slug, Visibility},
};
use mind_palace_core::ports::page_store::PageFilter;

use crate::app::AppState;
use crate::auth;

fn require_auth(jar: &CookieJar, state: &AppState) -> Result<auth::Claims, StatusCode> {
    auth::extract_claims(jar, &state.config.session_secret).ok_or(StatusCode::UNAUTHORIZED)
}

pub async fn list_pages(
    jar: CookieJar,
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, StatusCode> {
    require_auth(&jar, &state)?;
    let filter = PageFilter::default();
    let pages = state
        .service
        .list_pages(&filter, &state.ctx)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let summaries: Vec<PageSummary> = pages
        .into_iter()
        .map(|p| PageSummary {
            slug: p.slug.as_str().to_string(),
            title: p.title,
            summary: p.summary,
            page_type: p.page_type,
        })
        .collect();
    Ok(Json(summaries))
}

#[derive(Serialize)]
struct PageSummary {
    slug: String,
    title: String,
    summary: String,
    page_type: PageType,
}

pub async fn get_page(
    jar: CookieJar,
    State(state): State<Arc<AppState>>,
    Path(slug): Path<String>,
) -> Result<impl IntoResponse, StatusCode> {
    require_auth(&jar, &state)?;
    let slug = Slug::new(&slug).map_err(|_| StatusCode::BAD_REQUEST)?;
    let resp = state
        .service
        .read_page(&slug, ReadLevel::Full, &state.ctx)
        .await
        .map_err(|e| match e {
            mind_palace_core::error::MindPalaceError::PageNotFound(_) => StatusCode::NOT_FOUND,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        })?;
    match resp {
        mind_palace_core::domain::service::PageResponse::Full(page) => {
            Ok(Json(serde_json::to_value(page).unwrap_or_default()))
        }
        _ => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

#[derive(Deserialize)]
pub struct CreatePageRequest {
    pub title: String,
    pub slug: String,
    pub summary: String,
    pub sections: Vec<SectionInput>,
    pub page_type: Option<PageType>,
    pub links: Option<Vec<String>>,
}

#[derive(Deserialize)]
pub struct SectionInput {
    pub heading: String,
    pub content: String,
}

pub async fn create_page(
    jar: CookieJar,
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreatePageRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    require_auth(&jar, &state)?;
    let slug = Slug::new(&body.slug).map_err(|_| StatusCode::BAD_REQUEST)?;
    let links = body
        .links
        .unwrap_or_default()
        .into_iter()
        .filter_map(|s| Slug::new(&s).ok())
        .collect();
    let input = CreatePageInput {
        title: body.title,
        slug,
        summary: body.summary,
        sections: body
            .sections
            .into_iter()
            .map(|s| Section {
                heading: s.heading,
                content: s.content,
            })
            .collect(),
        page_type: body.page_type.unwrap_or(PageType::Concept),
        visibility: Visibility::General,
        links,
    };
    let (page, _issues) = state
        .service
        .create_page(input, &state.ctx)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok((
        StatusCode::CREATED,
        Json(serde_json::to_value(page).unwrap_or_default()),
    ))
}

#[derive(Deserialize)]
pub struct UpdatePageRequest {
    pub title: Option<String>,
    pub summary: Option<String>,
    pub sections: Option<Vec<SectionInput>>,
    pub links: Option<Vec<String>>,
}

pub async fn update_page(
    jar: CookieJar,
    State(state): State<Arc<AppState>>,
    Path(slug): Path<String>,
    Json(body): Json<UpdatePageRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    require_auth(&jar, &state)?;
    let slug = Slug::new(&slug).map_err(|_| StatusCode::BAD_REQUEST)?;
    let input = UpdatePageInput {
        title: body.title,
        summary: body.summary,
        sections: body.sections.map(|ss| {
            ss.into_iter()
                .map(|s| Section {
                    heading: s.heading,
                    content: s.content,
                })
                .collect()
        }),
        links: body
            .links
            .map(|ls| ls.into_iter().filter_map(|s| Slug::new(&s).ok()).collect()),
    };
    let (page, _issues) = state
        .service
        .update_page(&slug, input, &state.ctx)
        .await
        .map_err(|e| match e {
            mind_palace_core::error::MindPalaceError::PageNotFound(_) => StatusCode::NOT_FOUND,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        })?;
    Ok(Json(serde_json::to_value(page).unwrap_or_default()))
}

pub async fn delete_page(
    jar: CookieJar,
    State(state): State<Arc<AppState>>,
    Path(slug): Path<String>,
) -> Result<impl IntoResponse, StatusCode> {
    require_auth(&jar, &state)?;
    let slug = Slug::new(&slug).map_err(|_| StatusCode::BAD_REQUEST)?;
    state
        .service
        .delete_page(&slug, &state.ctx)
        .await
        .map_err(|e| match e {
            mind_palace_core::error::MindPalaceError::PageNotFound(_) => StatusCode::NOT_FOUND,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        })?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize)]
struct GraphResponse {
    nodes: Vec<GraphNode>,
    edges: Vec<GraphEdge>,
}

#[derive(Serialize)]
struct GraphNode {
    slug: String,
    title: String,
    page_type: PageType,
}

#[derive(Serialize)]
struct GraphEdge {
    source: String,
    target: String,
    kind: String,
}

pub async fn get_graph(
    jar: CookieJar,
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, StatusCode> {
    require_auth(&jar, &state)?;
    // Get all nodes via list_pages, then traverse each for edges
    let filter = PageFilter::default();
    let pages = state
        .service
        .list_pages(&filter, &state.ctx)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let nodes: Vec<GraphNode> = pages
        .iter()
        .map(|p| GraphNode {
            slug: p.slug.as_str().to_string(),
            title: p.title.clone(),
            page_type: p.page_type.clone(),
        })
        .collect();

    let mut edges = Vec::new();
    for page in &pages {
        let neighbors = state
            .service
            .traverse(&page.slug, 1, &state.ctx)
            .await
            .unwrap_or_default();
        for n in neighbors {
            edges.push(GraphEdge {
                source: page.slug.as_str().to_string(),
                target: n.slug.as_str().to_string(),
                kind: format!("{:?}", n.edge_kind),
            });
        }
    }

    Ok(Json(GraphResponse { nodes, edges }))
}

#[derive(Deserialize)]
pub struct SearchQuery {
    pub q: String,
}

pub async fn search(
    jar: CookieJar,
    State(state): State<Arc<AppState>>,
    Query(params): Query<SearchQuery>,
) -> Result<impl IntoResponse, StatusCode> {
    require_auth(&jar, &state)?;
    let results = state
        .service
        .search(&params.q, &state.ctx, 20)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(serde_json::to_value(results).unwrap_or_default()))
}

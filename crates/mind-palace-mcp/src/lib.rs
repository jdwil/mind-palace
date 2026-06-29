use std::sync::Arc;

use mind_palace_core::domain::page::ReadLevel;
use mind_palace_core::domain::service::{
    CreatePageInput, PageResponse, UpdatePageInput, WikiService,
};
use mind_palace_core::domain::tenant::TenantContext;
use mind_palace_core::domain::value_objects::{PageType, Section, Slug, Visibility};
use mind_palace_core::ports::page_store::PageFilter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, Content, ServerCapabilities, ServerInfo};
use rmcp::{ErrorData as McpError, ServerHandler, tool, tool_handler, tool_router};
use serde::Deserialize;

#[derive(Clone)]
pub struct MindPalaceMcpServer {
    service: Arc<WikiService>,
    ctx: TenantContext,
    #[allow(dead_code)]
    tool_router: rmcp::handler::server::tool::ToolRouter<Self>,
}

// --- Parameter structs ---

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SearchParams {
    #[schemars(description = "Search query")]
    pub query: String,
    #[schemars(description = "Max results (default 5)")]
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ReadParams {
    #[schemars(description = "Page slug")]
    pub slug: String,
    #[schemars(description = "Read level: summary, section, or full")]
    pub level: Option<String>,
    #[schemars(description = "Section heading (required if level=section)")]
    pub section: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct TraverseParams {
    #[schemars(description = "Starting page slug")]
    pub slug: String,
    #[schemars(description = "Traversal depth (default 2)")]
    pub depth: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SectionInput {
    pub heading: String,
    pub content: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CreateParams {
    pub title: String,
    pub slug: String,
    pub summary: String,
    pub sections: Vec<SectionInput>,
    #[schemars(description = "One of: Index, Concept, Entity, Decision, Leaf")]
    pub page_type: String,
    pub links: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct UpdateParams {
    #[schemars(description = "Page slug to update")]
    pub slug: String,
    pub title: Option<String>,
    pub summary: Option<String>,
    pub sections: Option<Vec<SectionInput>>,
    pub links: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ListParams {
    #[schemars(description = "Filter by page type: Index, Concept, Entity, Decision, Leaf")]
    pub page_type: Option<String>,
    #[schemars(description = "Max results (default 20)")]
    pub limit: Option<usize>,
}

// --- Tool router ---

#[tool_router]
impl MindPalaceMcpServer {
    pub fn new(service: Arc<WikiService>, ctx: TenantContext) -> Self {
        Self {
            service,
            ctx,
            tool_router: Self::tool_router(),
        }
    }

    #[tool(description = "Semantic search across wiki pages, returns ranked summaries")]
    async fn wiki_search(
        &self,
        Parameters(params): Parameters<SearchParams>,
    ) -> Result<CallToolResult, McpError> {
        let limit = params.limit.unwrap_or(5);
        let results = self
            .service
            .search(&params.query, &self.ctx, limit)
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        let output: Vec<serde_json::Value> = results
            .into_iter()
            .map(|r| {
                serde_json::json!({
                    "slug": r.slug.as_str(),
                    "title": r.title,
                    "summary": r.summary,
                    "score": r.score,
                })
            })
            .collect();

        let content =
            Content::json(output).map_err(|e| McpError::internal_error(e.to_string(), None))?;
        Ok(CallToolResult::success(vec![content]))
    }

    #[tool(description = "Read a wiki page at a given detail level (summary, section, or full)")]
    async fn wiki_read(
        &self,
        Parameters(params): Parameters<ReadParams>,
    ) -> Result<CallToolResult, McpError> {
        let slug =
            Slug::new(&params.slug).map_err(|e| McpError::internal_error(e.to_string(), None))?;
        let level = match params.level.as_deref().unwrap_or("summary") {
            "full" => ReadLevel::Full,
            "section" => {
                let heading = params.section.ok_or_else(|| {
                    McpError::internal_error("missing section heading".to_string(), None)
                })?;
                ReadLevel::Section(heading)
            }
            _ => ReadLevel::Summary,
        };
        let resp = self
            .service
            .read_page(&slug, level, &self.ctx)
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        let value = page_response_to_value(&resp);
        let content =
            Content::json(value).map_err(|e| McpError::internal_error(e.to_string(), None))?;
        Ok(CallToolResult::success(vec![content]))
    }

    #[tool(
        description = "Graph traversal from a page, returning connected pages up to a given depth"
    )]
    async fn wiki_traverse(
        &self,
        Parameters(params): Parameters<TraverseParams>,
    ) -> Result<CallToolResult, McpError> {
        let slug =
            Slug::new(&params.slug).map_err(|e| McpError::internal_error(e.to_string(), None))?;
        let depth = params.depth.unwrap_or(2);
        let neighbors = self
            .service
            .traverse(&slug, depth, &self.ctx)
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        let output: Vec<serde_json::Value> = neighbors
            .into_iter()
            .map(|n| {
                serde_json::json!({
                    "slug": n.slug.as_str(),
                    "title": n.title,
                    "summary": n.summary,
                    "page_type": format!("{:?}", n.page_type),
                    "edge_kind": format!("{:?}", n.edge_kind),
                })
            })
            .collect();

        let content =
            Content::json(output).map_err(|e| McpError::internal_error(e.to_string(), None))?;
        Ok(CallToolResult::success(vec![content]))
    }

    #[tool(description = "Create a new wiki page, returns lint issues")]
    async fn wiki_create(
        &self,
        Parameters(params): Parameters<CreateParams>,
    ) -> Result<CallToolResult, McpError> {
        let slug =
            Slug::new(&params.slug).map_err(|e| McpError::internal_error(e.to_string(), None))?;
        let sections = params
            .sections
            .into_iter()
            .map(|s| Section {
                heading: s.heading,
                content: s.content,
            })
            .collect();
        let links: Vec<Slug> = params
            .links
            .unwrap_or_default()
            .into_iter()
            .filter_map(|s| Slug::new(&s).ok())
            .collect();
        let input = CreatePageInput {
            title: params.title,
            slug,
            summary: params.summary,
            sections,
            page_type: parse_page_type(&params.page_type),
            visibility: Visibility::General,
            links,
        };
        let (page, issues) = self
            .service
            .create_page(input, &self.ctx)
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        let output = serde_json::json!({
            "slug": page.slug.as_str(),
            "title": page.title,
            "lint_issues": issues.len(),
        });
        let content =
            Content::json(output).map_err(|e| McpError::internal_error(e.to_string(), None))?;
        Ok(CallToolResult::success(vec![content]))
    }

    #[tool(description = "Update an existing wiki page, returns lint issues")]
    async fn wiki_update(
        &self,
        Parameters(params): Parameters<UpdateParams>,
    ) -> Result<CallToolResult, McpError> {
        let slug =
            Slug::new(&params.slug).map_err(|e| McpError::internal_error(e.to_string(), None))?;
        let sections = params.sections.map(|ss| {
            ss.into_iter()
                .map(|s| Section {
                    heading: s.heading,
                    content: s.content,
                })
                .collect()
        });
        let links = params
            .links
            .map(|ls| ls.into_iter().filter_map(|s| Slug::new(&s).ok()).collect());
        let input = UpdatePageInput {
            title: params.title,
            summary: params.summary,
            sections,
            links,
        };
        let (page, issues) = self
            .service
            .update_page(&slug, input, &self.ctx)
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        let output = serde_json::json!({
            "slug": page.slug.as_str(),
            "version": page.version,
            "lint_issues": issues.len(),
        });
        let content =
            Content::json(output).map_err(|e| McpError::internal_error(e.to_string(), None))?;
        Ok(CallToolResult::success(vec![content]))
    }

    #[tool(description = "List wiki pages, optionally filtered by type")]
    async fn wiki_list(
        &self,
        Parameters(params): Parameters<ListParams>,
    ) -> Result<CallToolResult, McpError> {
        let filter = PageFilter {
            page_type: params.page_type.as_deref().map(parse_page_type),
            visibility: None,
            limit: Some(params.limit.unwrap_or(20)),
        };
        let pages = self
            .service
            .list_pages(&filter, &self.ctx)
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        let output: Vec<serde_json::Value> = pages
            .into_iter()
            .map(|p| {
                serde_json::json!({
                    "slug": p.slug.as_str(),
                    "title": p.title,
                    "page_type": format!("{:?}", p.page_type),
                    "summary": p.summary,
                })
            })
            .collect();

        let content =
            Content::json(output).map_err(|e| McpError::internal_error(e.to_string(), None))?;
        Ok(CallToolResult::success(vec![content]))
    }
}

// --- ServerHandler ---

#[tool_handler]
impl ServerHandler for MindPalaceMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
    }
}

// --- Helpers ---

fn parse_page_type(s: &str) -> PageType {
    match s {
        "Index" => PageType::Index,
        "Concept" => PageType::Concept,
        "Entity" => PageType::Entity,
        "Decision" => PageType::Decision,
        _ => PageType::Leaf,
    }
}

fn page_response_to_value(resp: &PageResponse) -> serde_json::Value {
    match resp {
        PageResponse::Summary {
            title,
            slug,
            summary,
            page_type,
        } => serde_json::json!({
            "level": "summary",
            "title": title,
            "slug": slug.as_str(),
            "summary": summary,
            "page_type": format!("{:?}", page_type),
        }),
        PageResponse::Section { heading, content } => serde_json::json!({
            "level": "section",
            "heading": heading,
            "content": content,
        }),
        PageResponse::Full(page) => serde_json::json!({
            "level": "full",
            "title": page.title,
            "slug": page.slug.as_str(),
            "summary": page.summary,
            "page_type": format!("{:?}", page.page_type),
            "sections": page.sections.iter().map(|s| serde_json::json!({
                "heading": s.heading,
                "content": s.content,
            })).collect::<Vec<_>>(),
            "links": page.links.iter().map(|l| l.as_str()).collect::<Vec<_>>(),
        }),
    }
}

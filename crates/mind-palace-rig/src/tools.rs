use std::sync::Arc;

use mind_palace_core::domain::page::ReadLevel;
use mind_palace_core::domain::service::{
    CreatePageInput, PageResponse, UpdatePageInput, WikiService,
};
use mind_palace_core::domain::tenant::TenantContext;
use mind_palace_core::domain::value_objects::{PageType, Section, Slug, Visibility};
use mind_palace_core::ports::page_store::PageFilter;
use rig_core::completion::ToolDefinition;
use rig_core::tool::Tool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

// --- Shared Error ---

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct MindPalaceToolError(pub String);

impl From<mind_palace_core::error::MindPalaceError> for MindPalaceToolError {
    fn from(e: mind_palace_core::error::MindPalaceError) -> Self {
        Self(e.to_string())
    }
}

// --- WikiSearchTool ---

pub struct WikiSearchTool {
    pub service: Arc<WikiService>,
    pub ctx: TenantContext,
}

#[derive(Deserialize, JsonSchema)]
pub struct WikiSearchArgs {
    /// Search query
    pub query: String,
    /// Max results (default 5)
    pub limit: Option<usize>,
}

#[derive(Serialize)]
pub struct WikiSearchOutput {
    pub results: Vec<SearchResultItem>,
}

#[derive(Serialize)]
pub struct SearchResultItem {
    pub slug: String,
    pub title: String,
    pub summary: String,
    pub score: f64,
}

impl Tool for WikiSearchTool {
    const NAME: &'static str = "wiki_search";
    type Error = MindPalaceToolError;
    type Args = WikiSearchArgs;
    type Output = WikiSearchOutput;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Semantic search across wiki pages, returns ranked summaries".to_string(),
            parameters: serde_json::to_value(schemars::schema_for!(WikiSearchArgs)).unwrap(),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let limit = args.limit.unwrap_or(5);
        let results = self.service.search(&args.query, &self.ctx, limit).await?;
        Ok(WikiSearchOutput {
            results: results
                .into_iter()
                .map(|r| SearchResultItem {
                    slug: r.slug.as_str().to_string(),
                    title: r.title,
                    summary: r.summary,
                    score: r.score,
                })
                .collect(),
        })
    }
}

// --- WikiReadTool ---

pub struct WikiReadTool {
    pub service: Arc<WikiService>,
    pub ctx: TenantContext,
}

#[derive(Deserialize, JsonSchema)]
pub struct WikiReadArgs {
    /// Page slug
    pub slug: String,
    /// Detail level (default: "summary"). "summary" = title + one-line summary only
    /// (cheapest, NOT the full page). "section" = one named section (requires `section`).
    /// "full" = the ENTIRE page with all section bodies and links. Use "full" for complete content.
    pub level: Option<String>,
    /// Section heading (required if level=section)
    pub section: Option<String>,
}

#[derive(Serialize)]
pub struct WikiReadOutput {
    pub content: Value,
}

impl Tool for WikiReadTool {
    const NAME: &'static str = "wiki_read";
    type Error = MindPalaceToolError;
    type Args = WikiReadArgs;
    type Output = WikiReadOutput;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Read a wiki page. Defaults to 'summary' level (title + one-line \
                summary only). Pass level='full' to get the complete page with all section \
                content and links. Levels: summary (cheapest), section (one section), full (everything)."
                .to_string(),
            parameters: serde_json::to_value(schemars::schema_for!(WikiReadArgs)).unwrap(),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let slug = Slug::new(&args.slug).map_err(|e| MindPalaceToolError(e.to_string()))?;
        let level = match args.level.as_deref().unwrap_or("summary") {
            "full" => ReadLevel::Full,
            "section" => {
                let heading = args
                    .section
                    .ok_or_else(|| MindPalaceToolError("missing section heading".into()))?;
                ReadLevel::Section(heading)
            }
            _ => ReadLevel::Summary,
        };
        let resp = self.service.read_page(&slug, level, &self.ctx).await?;
        Ok(WikiReadOutput {
            content: page_response_to_value(&resp),
        })
    }
}

// --- WikiTraverseTool ---

pub struct WikiTraverseTool {
    pub service: Arc<WikiService>,
    pub ctx: TenantContext,
}

#[derive(Deserialize, JsonSchema)]
pub struct WikiTraverseArgs {
    /// Starting page slug
    pub slug: String,
    /// Traversal depth (default 2)
    pub depth: Option<usize>,
}

#[derive(Serialize)]
pub struct WikiTraverseOutput {
    pub neighbors: Vec<NeighborItem>,
}

#[derive(Serialize)]
pub struct NeighborItem {
    pub slug: String,
    pub title: String,
    pub summary: String,
    pub page_type: String,
    pub edge_kind: String,
}

impl Tool for WikiTraverseTool {
    const NAME: &'static str = "wiki_traverse";
    type Error = MindPalaceToolError;
    type Args = WikiTraverseArgs;
    type Output = WikiTraverseOutput;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description:
                "Graph traversal from a page, returning connected pages up to a given depth"
                    .to_string(),
            parameters: serde_json::to_value(schemars::schema_for!(WikiTraverseArgs)).unwrap(),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let slug = Slug::new(&args.slug).map_err(|e| MindPalaceToolError(e.to_string()))?;
        let depth = args.depth.unwrap_or(2);
        let neighbors = self.service.traverse(&slug, depth, &self.ctx).await?;
        Ok(WikiTraverseOutput {
            neighbors: neighbors
                .into_iter()
                .map(|n| NeighborItem {
                    slug: n.slug.as_str().to_string(),
                    title: n.title,
                    summary: n.summary,
                    page_type: format!("{:?}", n.page_type),
                    edge_kind: format!("{:?}", n.edge_kind),
                })
                .collect(),
        })
    }
}

// --- WikiCreateTool ---

pub struct WikiCreateTool {
    pub service: Arc<WikiService>,
    pub ctx: TenantContext,
}

#[derive(Deserialize, JsonSchema)]
pub struct SectionInput {
    pub heading: String,
    pub content: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct WikiCreateArgs {
    pub title: String,
    pub slug: String,
    pub summary: String,
    pub sections: Vec<SectionInput>,
    /// One of: Index, Concept, Entity, Decision, Leaf, Sop, Skill
    pub page_type: String,
    pub links: Option<Vec<String>>,
    /// Page visibility: "general" (default) or "user" (scoped to the current user only)
    pub visibility: Option<String>,
}

#[derive(Serialize)]
pub struct WikiCreateOutput {
    pub slug: String,
    pub title: String,
    pub lint_issues: usize,
}

impl Tool for WikiCreateTool {
    const NAME: &'static str = "wiki_create";
    type Error = MindPalaceToolError;
    type Args = WikiCreateArgs;
    type Output = WikiCreateOutput;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Create a new wiki page".to_string(),
            parameters: serde_json::to_value(schemars::schema_for!(WikiCreateArgs)).unwrap(),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let slug = Slug::new(&args.slug).map_err(|e| MindPalaceToolError(e.to_string()))?;
        let sections = args
            .sections
            .into_iter()
            .map(|s| Section {
                heading: s.heading,
                content: s.content,
            })
            .collect();
        let links: Vec<Slug> = args
            .links
            .unwrap_or_default()
            .into_iter()
            .filter_map(|s| Slug::new(&s).ok())
            .collect();
        let input = CreatePageInput {
            title: args.title,
            slug,
            summary: args.summary,
            sections,
            page_type: parse_page_type(&args.page_type),
            visibility: match args.visibility.as_deref() {
                Some("user") => {
                    Visibility::User(self.ctx.user_id().unwrap_or("unknown").to_string())
                }
                _ => Visibility::General,
            },
            links,
        };
        let (page, issues) = self.service.create_page(input, &self.ctx).await?;
        Ok(WikiCreateOutput {
            slug: page.slug.as_str().to_string(),
            title: page.title,
            lint_issues: issues.len(),
        })
    }
}

// --- WikiUpdateTool ---

pub struct WikiUpdateTool {
    pub service: Arc<WikiService>,
    pub ctx: TenantContext,
}

#[derive(Deserialize, JsonSchema)]
pub struct WikiUpdateArgs {
    /// Page slug to update
    pub slug: String,
    pub title: Option<String>,
    pub summary: Option<String>,
    pub sections: Option<Vec<SectionInput>>,
    pub links: Option<Vec<String>>,
}

#[derive(Serialize)]
pub struct WikiUpdateOutput {
    pub slug: String,
    pub version: u32,
    pub lint_issues: usize,
}

impl Tool for WikiUpdateTool {
    const NAME: &'static str = "wiki_update";
    type Error = MindPalaceToolError;
    type Args = WikiUpdateArgs;
    type Output = WikiUpdateOutput;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Update an existing wiki page, returns lint issues".to_string(),
            parameters: serde_json::to_value(schemars::schema_for!(WikiUpdateArgs)).unwrap(),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let slug = Slug::new(&args.slug).map_err(|e| MindPalaceToolError(e.to_string()))?;
        let sections = args.sections.map(|ss| {
            ss.into_iter()
                .map(|s| Section {
                    heading: s.heading,
                    content: s.content,
                })
                .collect()
        });
        let links = args
            .links
            .map(|ls| ls.into_iter().filter_map(|s| Slug::new(&s).ok()).collect());
        let input = UpdatePageInput {
            title: args.title,
            summary: args.summary,
            sections,
            links,
        };
        let (page, issues) = self.service.update_page(&slug, input, &self.ctx).await?;
        Ok(WikiUpdateOutput {
            slug: page.slug.as_str().to_string(),
            version: page.version,
            lint_issues: issues.len(),
        })
    }
}

// --- WikiListTool ---

pub struct WikiListTool {
    pub service: Arc<WikiService>,
    pub ctx: TenantContext,
}

#[derive(Deserialize, JsonSchema)]
pub struct WikiListArgs {
    /// Filter by page type: Index, Concept, Entity, Decision, Leaf, Sop, Skill
    pub page_type: Option<String>,
    /// Max results (default 20)
    pub limit: Option<usize>,
}

#[derive(Serialize)]
pub struct WikiListOutput {
    pub pages: Vec<PageSummaryItem>,
}

#[derive(Serialize)]
pub struct PageSummaryItem {
    pub slug: String,
    pub title: String,
    pub page_type: String,
    pub summary: String,
}

impl Tool for WikiListTool {
    const NAME: &'static str = "wiki_list";
    type Error = MindPalaceToolError;
    type Args = WikiListArgs;
    type Output = WikiListOutput;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "List wiki pages, optionally filtered by type".to_string(),
            parameters: serde_json::to_value(schemars::schema_for!(WikiListArgs)).unwrap(),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let filter = PageFilter {
            page_type: args.page_type.as_deref().map(parse_page_type),
            visibility: None,
            limit: Some(args.limit.unwrap_or(20)),
        };
        let pages = self.service.list_pages(&filter, &self.ctx).await?;
        Ok(WikiListOutput {
            pages: pages
                .into_iter()
                .map(|p| PageSummaryItem {
                    slug: p.slug.as_str().to_string(),
                    title: p.title,
                    page_type: format!("{:?}", p.page_type),
                    summary: p.summary,
                })
                .collect(),
        })
    }
}

// --- Helpers ---

fn parse_page_type(s: &str) -> PageType {
    match s {
        "Index" => PageType::Index,
        "Concept" => PageType::Concept,
        "Entity" => PageType::Entity,
        "Decision" => PageType::Decision,
        "Sop" => PageType::Sop,
        "Skill" => PageType::Skill,
        _ => PageType::Leaf,
    }
}

fn page_response_to_value(resp: &PageResponse) -> Value {
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

// --- WikiArchiveTool ---

pub struct WikiArchiveTool {
    pub service: Arc<WikiService>,
    pub ctx: TenantContext,
}

#[derive(Deserialize, JsonSchema)]
pub struct WikiArchiveArgs {
    /// Page slug to archive
    pub slug: String,
}

#[derive(Serialize)]
pub struct WikiArchiveOutput {
    pub slug: String,
    pub archived: bool,
}

impl Tool for WikiArchiveTool {
    const NAME: &'static str = "wiki_archive";
    type Error = MindPalaceToolError;
    type Args = WikiArchiveArgs;
    type Output = WikiArchiveOutput;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Archive a wiki page (soft delete). Removes from search/list/traverse but can be unarchived later.".to_string(),
            parameters: serde_json::to_value(schemars::schema_for!(WikiArchiveArgs)).unwrap(),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let slug = Slug::new(&args.slug).map_err(|e| MindPalaceToolError(e.to_string()))?;
        self.service.archive_page(&slug, &self.ctx).await?;
        Ok(WikiArchiveOutput {
            slug: args.slug,
            archived: true,
        })
    }
}

// --- WikiUnarchiveTool ---

pub struct WikiUnarchiveTool {
    pub service: Arc<WikiService>,
    pub ctx: TenantContext,
}

#[derive(Deserialize, JsonSchema)]
pub struct WikiUnarchiveArgs {
    /// Page slug to unarchive
    pub slug: String,
}

#[derive(Serialize)]
pub struct WikiUnarchiveOutput {
    pub slug: String,
    pub unarchived: bool,
}

impl Tool for WikiUnarchiveTool {
    const NAME: &'static str = "wiki_unarchive";
    type Error = MindPalaceToolError;
    type Args = WikiUnarchiveArgs;
    type Output = WikiUnarchiveOutput;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description:
                "Unarchive a previously archived wiki page, restoring it to General visibility."
                    .to_string(),
            parameters: serde_json::to_value(schemars::schema_for!(WikiUnarchiveArgs)).unwrap(),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let slug = Slug::new(&args.slug).map_err(|e| MindPalaceToolError(e.to_string()))?;
        self.service.unarchive_page(&slug).await?;
        Ok(WikiUnarchiveOutput {
            slug: args.slug,
            unarchived: true,
        })
    }
}

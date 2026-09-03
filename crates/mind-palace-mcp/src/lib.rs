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

/// The Mind Palace operating manual, returned by the `wiki_instructions` tool.
/// Shipped in the binary so instructions version with the code — installers only
/// need a one-line prompt ("call wiki_instructions before doing any work").
const WIKI_INSTRUCTIONS: &str = r#"# Knowledge Base (Mind Palace)

You have access to a persistent wiki-style knowledge base that stores synthesized knowledge across all interactions. This is your long-term memory. Use it constantly — it compounds over time and makes you more effective with every interaction.

## Core Behavior

1. **Search before answering.** Before responding to any knowledge-dependent question or starting a task, call `wiki_search` with relevant keywords. If results exist, read them before forming your answer. Do NOT rely solely on your training data when the wiki might have more current, project-specific, or user-specific information.

2. **Read progressively.** Start with summaries (cheap). The `wiki_read` tool defaults to the `summary` level (title + one-line summary only). When you need the actual page content, call `wiki_read` with `level="full"` to get every section body and links. Use `level="section"` for a single named section. Use `wiki_traverse` to explore connected pages when you need broader context.

3. **Write after learning.** After any interaction where you gained new information, resolved ambiguity, made a decision, or completed a non-trivial task:
   - Search for existing pages on the topic first
   - If a page exists, UPDATE it (`wiki_update`) — do not create duplicates
   - If no page exists, CREATE one (`wiki_create`)
   - Synthesize — store the insight, not the raw conversation

4. **Link everything.** Always add relevant slugs to the `links` field when creating or updating. This builds the graph that makes traversal useful.

5. **Archive, don't delete.** When a page is obsolete (e.g., a completed spec), call `wiki_archive` to hide it from search/list/traverse. It can be restored with `wiki_unarchive`. Only humans hard-delete via the web UI.

## Page Types

| Type | Use For | Example |
|------|---------|---------|
| `Index` | Lightweight hub linking to related pages | "deployment-index" linking to all deploy-related pages |
| `Concept` | Mid-level synthesis of a topic | "rust-error-handling", "multi-tenancy-design" |
| `Entity` | Specific thing: person, project, service | "dashlx-ecs-cluster", "client-acme-corp" |
| `Decision` | Record of a decision + rationale | "decision-use-s3-vectors-over-pinecone" |
| `Leaf` | Deep reference material | "aws-sdk-dynamodb-single-table-patterns" |
| `Sop` | Step-by-step procedure any agent can follow | "sop-deploy-to-production" |
| `Skill` | Claude-optimized prompt pattern/technique | "skill-progressive-disclosure-prompting" |

## Page Structure Rules

- **Summary** (required): 1-2 sentences. This is what search results show. Make it count.
- **Sections** (at least one required): Use clear headings. Content is Markdown.
- **Slug**: lowercase, hyphens only. Descriptive: `rust-ownership-patterns` not `page-47`.
- **Links**: slugs of related pages. Builds the knowledge graph.
- **Visibility**: `general` (default) or `user` (personal). See below.

## Visibility — General vs User-Scoped Pages

Most pages should be **general** (visible to all users). Use general for:
- Technical decisions, architecture, patterns
- Project knowledge, domain concepts, SOPs
- Anything the team should share

Use **user-scoped** (`visibility: "user"`) only for:
- Personal preferences (coding style, tool preferences, workflow habits)
- Individual context (what this person is working on, their ramp-up status)
- Opinions that are explicitly personal and should not be applied to others

**When in doubt, default to general.** Knowledge is more valuable when shared. Only scope to user when the content is genuinely personal and would be noise or misleading for other team members.

Examples:
- "We use Result<T, Error> everywhere" -> general (team decision)
- "JD prefers verbose variable names over abbreviations" -> user
- "The data pipeline architecture uses X" -> general
- "Sarah is currently ramping up on the auth module" -> user (Sarah's)

## SOP Pages (required sections)

| Section | Purpose |
|---------|---------|
| Prerequisites | What must be true before starting |
| Steps | Numbered actions to perform |
| Constraints | MUST/SHOULD/MAY rules |
| Verification | How to confirm success |

## Skill Pages (required sections)

| Section | Purpose |
|---------|---------|
| When to Use | Conditions that trigger this skill |
| Prompt Pattern | The actual technique |
| Example | Concrete demonstration |
| Limitations | When it doesn't work |

## What NOT to Write

- Trivial one-off facts that won't matter in future interactions
- Information already well-captured in an existing page (update that page instead)
- Raw conversation logs (synthesize first, then store the synthesis)
- Speculative content without basis — only store what you know or have decided

## Maintenance Habits

- When you notice outdated information while reading a page, update it immediately
- When lint issues are returned after create/update, fix them before moving on
- Prefer fewer, richer, well-linked pages over many shallow disconnected ones
"#;

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
    #[schemars(
        description = "Detail level (default: 'summary'). 'summary' = title + one-line summary only (cheapest, NOT the full page). 'section' = one named section (requires 'section'). 'full' = the ENTIRE page with all section bodies and links. Use 'full' when you need the complete content."
    )]
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
    #[schemars(
        description = "Page visibility: 'general' (default, everyone sees it) or 'user' (only the current user sees it). Use 'user' only for personal preferences, context, or opinions that should not apply to other users."
    )]
    pub visibility: Option<String>,
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

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ArchiveParams {
    #[schemars(description = "Page slug to archive or unarchive")]
    pub slug: String,
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

    #[tool(
        description = "Read this FIRST, before using any other wiki tool. Returns the operating manual for the Mind Palace knowledge base: when to search, when to write, page types, structure rules, and visibility. Call this at the start of your work."
    )]
    async fn wiki_instructions(&self) -> Result<CallToolResult, McpError> {
        Ok(CallToolResult::success(vec![Content::text(
            WIKI_INSTRUCTIONS,
        )]))
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

    #[tool(
        description = "Read a wiki page. Defaults to 'summary' level (title + one-line summary only). Pass level='full' to get the complete page with all section content and links. Levels: summary (cheapest), section (one section), full (everything)."
    )]
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
            visibility: match params.visibility.as_deref() {
                Some("user") => {
                    Visibility::User(self.ctx.user_id().unwrap_or("unknown").to_string())
                }
                _ => Visibility::General,
            },
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

    #[tool(
        description = "Archive a wiki page (soft delete). Removes from search/list/traverse but can be unarchived later."
    )]
    async fn wiki_archive(
        &self,
        Parameters(params): Parameters<ArchiveParams>,
    ) -> Result<CallToolResult, McpError> {
        let slug =
            Slug::new(&params.slug).map_err(|e| McpError::internal_error(e.to_string(), None))?;
        self.service
            .archive_page(&slug, &self.ctx)
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        let output = serde_json::json!({ "slug": params.slug, "archived": true });
        let content =
            Content::json(output).map_err(|e| McpError::internal_error(e.to_string(), None))?;
        Ok(CallToolResult::success(vec![content]))
    }

    #[tool(
        description = "Unarchive a previously archived wiki page, restoring it to General visibility."
    )]
    async fn wiki_unarchive(
        &self,
        Parameters(params): Parameters<ArchiveParams>,
    ) -> Result<CallToolResult, McpError> {
        let slug =
            Slug::new(&params.slug).map_err(|e| McpError::internal_error(e.to_string(), None))?;
        self.service
            .unarchive_page(&slug)
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        let output = serde_json::json!({ "slug": params.slug, "unarchived": true });
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

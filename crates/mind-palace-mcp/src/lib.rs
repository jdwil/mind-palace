use std::sync::Arc;

use mind_palace_core::domain::page::ReadLevel;
use mind_palace_core::domain::service::{
    CreatePageInput, PageResponse, UpdatePageInput, WikiService,
};
use mind_palace_core::domain::tenant::{Identity, TenantContext};
use mind_palace_core::domain::value_objects::{
    BaseVisibility, GroupId, Level, PageType, Principal, SecretRef, Section, Slug, Visibility,
};
use mind_palace_core::ports::page_store::PageFilter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, Content, Extensions, ServerCapabilities, ServerInfo};
use rmcp::{ErrorData as McpError, ServerHandler, tool, tool_handler, tool_router};
use serde::Deserialize;

pub mod auth;
pub mod setup;

/// Returned in the MCP `initialize` response `instructions` field. Clients that
/// surface server instructions to the model will show this automatically at
/// connection time — no base-prompt configuration needed by the operator.
/// Kept short: it points at the `wiki_instructions` tool, which holds the full manual.
const SERVER_INSTRUCTIONS: &str = "This server provides Mind Palace, a persistent wiki-style knowledge base that is your long-term memory across all sessions.\n\nYou MUST call the `wiki_instructions` tool before using any other wiki tool. It returns the operating manual (when to search, when to write, page types, structure, visibility). Follow it for the rest of the session.\n\nAt minimum: search the wiki before answering knowledge-dependent questions, and write synthesized knowledge back after you learn something or complete a non-trivial task.";

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
   - `wiki_update` MERGES sections by heading by default: a section whose heading matches is updated in place, new headings are appended, and headings you omit are preserved. So you can update one section without resending the whole page. To REMOVE a section, pass its heading in `delete_sections`. Pass `replace_sections: true` only when you intend to rewrite the entire page.

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

## Storing Code (scripts, queries, snippets)

You can store Python scripts, SQL queries, shell commands, config, etc. Follow two rules so it stores cleanly and stays findable:

1. **Always wrap code in a fenced block** with a language tag:
   ```
   ```python
   # your code here
   ```
   ```
   Fenced code is parsed as a single unit — lines inside it that start with `## ` will NOT be mistaken for section headings. Unfenced code with `## ` lines WILL corrupt the page on reload.

2. **Pair code with a prose description** in the same section (what it does, when to use it, inputs/outputs). Semantic search embeds the whole page; a bare code dump with no prose is hard to find. The prose is what makes the snippet retrievable.

Use a `Leaf` page for a standalone reusable script/query, or put the code in the `Example` section of a `Skill` page. There is no separate "code" page type — a fenced block in a normal section is the right home.

## Secrets — NEVER store a plaintext secret

A wiki page must NEVER contain a plaintext secret (API key, token, password, connection string with credentials). Page content is stored in S3, kept in version history, and flows through chat transcripts and the dreaming process — a secret written into a page leaks into all of them.

Rules:
- **If a user gives you a raw secret to "save", REFUSE.** Do not put it in a section, summary, or anywhere in page text. Instead, tell them to store it in the secrets backend (e.g. AWS Secrets Manager) and give you the *reference* (e.g. an ARN).
- **Store only references, in the structured `secret_refs` field** — never in free text. Each ref is `{ name, reference }` where `reference` is an opaque locator, never the value. The system REJECTS a `secret_refs` entry whose reference looks like a raw secret (code-level guardrail, not just this instruction).
- **A page carrying secret refs must be Private** (owner + explicit grants). A Public page with secret refs fails lint (Error) — anyone, including anonymous, could resolve it.
- **Resolve a secret only when you actually need it**, via the separate `wiki_get_secret(slug, name)` tool. `wiki_read` never returns secret values — only the reference/name.
- **Never echo a resolved value** into your visible output, another page, a log, or storage. Use it for the immediate operation only. Every resolution is audit-logged.

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
- After create/update, `lint_detail` lists each issue (code, severity, message). Fix Errors before moving on. Warnings/Info (e.g. a BrokenLink to a page you haven't created yet, or an Orphan) are advisory — address them when practical, but they are not failures and long-established pages may carry them.
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
    #[schemars(
        description = "Max results (default 5, hard max 100 — S3 Vectors limit). Search is for relevance ranking, NOT enumeration; to list every page use wiki_list."
    )]
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
    #[schemars(
        description = "Spec 2 base visibility: 'public' (open to everyone incl. anonymous) or 'private' (owner + explicit grants only). If omitted, inferred from `visibility`. Owner is set to the caller."
    )]
    pub base_visibility: Option<String>,
    #[schemars(
        description = "Optional containment parent slug (Spec 3). Places this page UNDER that container so it inherits the container's access grants downward. Distinct from `links` (which are 'see also' associations carrying NO access). Requires edit permission on BOTH this page and the parent, and must not form a cycle. Omit for a top-level page."
    )]
    pub parent: Option<String>,
    #[schemars(
        description = "Optional structured secret references (Spec 4). Each is { name, reference } where `reference` is an opaque backend locator (e.g. an AWS Secrets Manager ARN) — NEVER a raw secret value. Raw values are REJECTED. Resolve later with wiki_get_secret. Do NOT put secret references on a Public page (lint Error); the page must be Private."
    )]
    pub secret_refs: Option<Vec<SecretRefInput>>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SecretRefInput {
    #[schemars(description = "A name used to look up this reference via wiki_get_secret")]
    pub name: String,
    #[schemars(
        description = "Opaque backend locator (e.g. an AWS Secrets Manager ARN). NEVER a raw secret value — raw values are rejected."
    )]
    pub reference: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct UpdateParams {
    #[schemars(description = "Page slug to update")]
    pub slug: String,
    pub title: Option<String>,
    pub summary: Option<String>,
    #[schemars(
        description = "Sections to write. By DEFAULT these are MERGED into the page by heading: a section whose heading matches an existing one replaces that section's content; new headings are appended; headings you don't include are left untouched. So you can update a single section without resending the whole page. Set replace_sections=true to instead replace the entire section list."
    )]
    pub sections: Option<Vec<SectionInput>>,
    pub links: Option<Vec<String>>,
    #[schemars(
        description = "If true, fully REPLACE the page's section list with the provided sections (anything not included is deleted). Default false = merge by heading. Only use true when intentionally rewriting the whole page."
    )]
    pub replace_sections: Option<bool>,
    #[schemars(
        description = "Section headings to delete from the page. Use this to remove a stale/superseded section (applied after merge/replace)."
    )]
    pub delete_sections: Option<Vec<String>>,
    #[schemars(
        description = "Set/change the containment parent (Spec 3): a slug to (re)parent this page under that container (inherits its access grants; requires edit on both and no cycle). Omit to leave the parent unchanged. To DETACH to top-level, set detach_parent=true instead."
    )]
    pub parent: Option<String>,
    #[schemars(
        description = "If true, detach this page from its containment parent (make it top-level). Ignored if `parent` is also set."
    )]
    pub detach_parent: Option<bool>,
    #[schemars(
        description = "Optional: REPLACE the page's secret references (Spec 4) with this list of { name, reference }. Each `reference` is an opaque backend locator (e.g. an ARN), NEVER a raw secret value (rejected). Omit to leave secret refs unchanged; pass [] to clear them."
    )]
    pub secret_refs: Option<Vec<SecretRefInput>>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ListParams {
    #[schemars(description = "Filter by page type: Index, Concept, Entity, Decision, Leaf")]
    pub page_type: Option<String>,
    #[schemars(description = "Max results. Omit to return ALL pages (no cap).")]
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ArchiveParams {
    #[schemars(description = "Page slug to archive or unarchive")]
    pub slug: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ShareParams {
    #[schemars(description = "Page slug to share")]
    pub slug: String,
    #[schemars(
        description = "Grantee kind: 'user' (an email) or 'group' (a group id). Determines how `principal` is interpreted."
    )]
    pub principal_type: String,
    #[schemars(
        description = "The grantee: an email (principal_type=user) or group id (principal_type=group)"
    )]
    pub principal: String,
    #[schemars(description = "Permission level: 'view' or 'edit' (default 'view')")]
    pub level: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct UnshareParams {
    #[schemars(description = "Page slug to unshare")]
    pub slug: String,
    #[schemars(description = "Grantee kind: 'user' or 'group'")]
    pub principal_type: String,
    #[schemars(description = "The grantee: an email (user) or group id (group)")]
    pub principal: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GroupCreateParams {
    #[schemars(description = "Group id (a stable slug-like key, e.g. 'eng-team')")]
    pub id: String,
    #[schemars(description = "Human-readable group name")]
    pub name: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GroupMemberParams {
    #[schemars(description = "Group id")]
    pub id: String,
    #[schemars(description = "Member email to add or remove")]
    pub member: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GetSecretParams {
    #[schemars(description = "Slug of the page that carries the secret reference")]
    pub slug: String,
    #[schemars(
        description = "The `name` of the secret reference on that page (as set in secret_refs)"
    )]
    pub name: String,
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

    /// Resolve the effective [`TenantContext`] for a single request.
    ///
    /// The remote HTTP transport's auth middleware validates the request and
    /// inserts an [`Identity`] into the axum request extensions. rmcp threads
    /// the raw `http::request::Parts` into the MCP request [`Extensions`], so we
    /// read the identity per request here. When no per-request identity is
    /// present (e.g. the local stdio binary, which has no HTTP parts), we fall
    /// back to the server's base context built from env config — keeping the
    /// single-user stdio experience unchanged.
    fn ctx_for(&self, ext: &Extensions) -> TenantContext {
        ext.get::<axum::http::request::Parts>()
            .and_then(|parts| parts.extensions.get::<Identity>())
            .map(|id| TenantContext::from_identity(id.clone()))
            .unwrap_or_else(|| self.ctx.clone())
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
        ext: Extensions,
    ) -> Result<CallToolResult, McpError> {
        let ctx = self.ctx_for(&ext);
        let limit = params.limit.unwrap_or(5);
        let results = self
            .service
            .search(&params.query, &ctx, limit)
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
        ext: Extensions,
    ) -> Result<CallToolResult, McpError> {
        let ctx = self.ctx_for(&ext);
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
        let resp = match self.service.read_page(&slug, level, &ctx).await {
            Ok(r) => r,
            Err(mind_palace_core::error::MindPalaceError::PageNotFound(_)) => {
                // Either the page doesn't exist or it's scoped to another user
                // (visibility hides it). Return a clean, non-error result so the
                // agent handles it gracefully instead of surfacing a raw crash.
                let value = serde_json::json!({
                    "found": false,
                    "slug": params.slug,
                    "message": "Page not found, or not accessible with the current visibility (it may be scoped to another user).",
                });
                let content = Content::json(value)
                    .map_err(|e| McpError::internal_error(e.to_string(), None))?;
                return Ok(CallToolResult::success(vec![content]));
            }
            Err(e) => return Err(McpError::internal_error(e.to_string(), None)),
        };

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
        ext: Extensions,
    ) -> Result<CallToolResult, McpError> {
        let ctx = self.ctx_for(&ext);
        let slug =
            Slug::new(&params.slug).map_err(|e| McpError::internal_error(e.to_string(), None))?;
        let depth = params.depth.unwrap_or(2);
        let neighbors = self
            .service
            .traverse(&slug, depth, &ctx)
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
        ext: Extensions,
    ) -> Result<CallToolResult, McpError> {
        let ctx = self.ctx_for(&ext);
        // Spec 2 §4: anonymous callers cannot create pages by default.
        if ctx.identity.is_anonymous() {
            return Err(McpError::internal_error(
                "anonymous callers may not create pages".to_string(),
                None,
            ));
        }
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
        // Map the legacy `visibility` param + Spec 2 `base_visibility`:
        // - "user"/"private" → Private (owner-only + grants).
        // - "public"/"general"/default → Public.
        let (visibility, base_visibility) = match params
            .visibility
            .as_deref()
            .or(params.base_visibility.as_deref())
        {
            Some("user") | Some("private") => (
                Visibility::User(ctx.user_id().unwrap_or("unknown").to_string()),
                Some(BaseVisibility::Private),
            ),
            _ => (Visibility::General, Some(BaseVisibility::Public)),
        };
        let input = CreatePageInput {
            title: params.title,
            slug,
            summary: params.summary,
            sections,
            page_type: parse_page_type(&params.page_type),
            visibility,
            links,
            base_visibility,
            parent: params.parent.as_deref().and_then(|s| Slug::new(s).ok()),
            secret_refs: params
                .secret_refs
                .unwrap_or_default()
                .into_iter()
                .map(|r| SecretRef::new(r.name, r.reference))
                .collect(),
        };
        let (page, issues) = self
            .service
            .create_page(input, &ctx)
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        let output = serde_json::json!({
            "slug": page.slug.as_str(),
            "title": page.title,
            "lint_issues": issues.len(),
            "lint_detail": lint_issues_to_json(&issues),
        });
        let content =
            Content::json(output).map_err(|e| McpError::internal_error(e.to_string(), None))?;
        Ok(CallToolResult::success(vec![content]))
    }

    #[tool(description = "Update an existing wiki page, returns lint issues")]
    async fn wiki_update(
        &self,
        Parameters(params): Parameters<UpdateParams>,
        ext: Extensions,
    ) -> Result<CallToolResult, McpError> {
        let ctx = self.ctx_for(&ext);
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
        // Spec 3 three-state reparent:
        //   parent=Some(slug)      → reparent under slug
        //   detach_parent=true     → detach to root (Some(None))
        //   neither                → leave unchanged (None)
        let parent = if let Some(p) = params.parent.as_deref() {
            match Slug::new(p) {
                Ok(s) => Some(Some(s)),
                Err(e) => return Err(McpError::internal_error(e.to_string(), None)),
            }
        } else if params.detach_parent.unwrap_or(false) {
            Some(None)
        } else {
            None
        };
        let input = UpdatePageInput {
            title: params.title,
            summary: params.summary,
            sections,
            links,
            replace_sections: params.replace_sections.unwrap_or(false),
            delete_sections: params.delete_sections.unwrap_or_default(),
            parent,
            secret_refs: params.secret_refs.map(|v| {
                v.into_iter()
                    .map(|r| SecretRef::new(r.name, r.reference))
                    .collect()
            }),
        };
        let (page, issues) = self
            .service
            .update_page(&slug, input, &ctx)
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        let output = serde_json::json!({
            "slug": page.slug.as_str(),
            "version": page.version,
            "lint_issues": issues.len(),
            "lint_detail": lint_issues_to_json(&issues),
        });
        let content =
            Content::json(output).map_err(|e| McpError::internal_error(e.to_string(), None))?;
        Ok(CallToolResult::success(vec![content]))
    }

    #[tool(description = "List wiki pages, optionally filtered by type")]
    async fn wiki_list(
        &self,
        Parameters(params): Parameters<ListParams>,
        ext: Extensions,
    ) -> Result<CallToolResult, McpError> {
        let ctx = self.ctx_for(&ext);
        let filter = PageFilter {
            page_type: params.page_type.as_deref().map(parse_page_type),
            visibility: None,
            // None = return all pages (no silent cap). Callers may still pass a limit.
            limit: params.limit,
        };
        let pages = self
            .service
            .list_pages(&filter, &ctx)
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
        ext: Extensions,
    ) -> Result<CallToolResult, McpError> {
        let ctx = self.ctx_for(&ext);
        let slug =
            Slug::new(&params.slug).map_err(|e| McpError::internal_error(e.to_string(), None))?;
        self.service
            .archive_page(&slug, &ctx)
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

    #[tool(
        description = "Share a page: grant a user (email) or group access at a level (view/edit). Only the page owner may change grants."
    )]
    async fn wiki_share(
        &self,
        Parameters(params): Parameters<ShareParams>,
        ext: Extensions,
    ) -> Result<CallToolResult, McpError> {
        let ctx = self.ctx_for(&ext);
        let slug =
            Slug::new(&params.slug).map_err(|e| McpError::internal_error(e.to_string(), None))?;
        let principal = parse_principal(&params.principal_type, &params.principal)
            .map_err(|e| McpError::internal_error(e, None))?;
        let level = match params.level.as_deref() {
            Some("edit") => Level::Edit,
            _ => Level::View,
        };
        self.service
            .share_page(&slug, principal, level, &ctx)
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        let output = serde_json::json!({ "slug": params.slug, "shared": true });
        let content =
            Content::json(output).map_err(|e| McpError::internal_error(e.to_string(), None))?;
        Ok(CallToolResult::success(vec![content]))
    }

    #[tool(
        description = "Unshare a page: remove a user or group grant. Only the page owner may change grants."
    )]
    async fn wiki_unshare(
        &self,
        Parameters(params): Parameters<UnshareParams>,
        ext: Extensions,
    ) -> Result<CallToolResult, McpError> {
        let ctx = self.ctx_for(&ext);
        let slug =
            Slug::new(&params.slug).map_err(|e| McpError::internal_error(e.to_string(), None))?;
        let principal = parse_principal(&params.principal_type, &params.principal)
            .map_err(|e| McpError::internal_error(e, None))?;
        self.service
            .unshare_page(&slug, &principal, &ctx)
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        let output = serde_json::json!({ "slug": params.slug, "unshared": true });
        let content =
            Content::json(output).map_err(|e| McpError::internal_error(e.to_string(), None))?;
        Ok(CallToolResult::success(vec![content]))
    }

    #[tool(
        description = "Create a group. The caller becomes the sole initial manager and member. Members inherit any grant made to the group."
    )]
    async fn wiki_group_create(
        &self,
        Parameters(params): Parameters<GroupCreateParams>,
        ext: Extensions,
    ) -> Result<CallToolResult, McpError> {
        let ctx = self.ctx_for(&ext);
        let group = self
            .service
            .group_create(GroupId::new(params.id), &params.name, &ctx)
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        let output = group_to_json(&group);
        let content =
            Content::json(output).map_err(|e| McpError::internal_error(e.to_string(), None))?;
        Ok(CallToolResult::success(vec![content]))
    }

    #[tool(
        description = "Add a member to a group. Requires the caller to be a manager of the group (privileged operation)."
    )]
    async fn wiki_group_add_member(
        &self,
        Parameters(params): Parameters<GroupMemberParams>,
        ext: Extensions,
    ) -> Result<CallToolResult, McpError> {
        let ctx = self.ctx_for(&ext);
        let group = self
            .service
            .group_add_member(&GroupId::new(params.id), &params.member, &ctx)
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        let output = group_to_json(&group);
        let content =
            Content::json(output).map_err(|e| McpError::internal_error(e.to_string(), None))?;
        Ok(CallToolResult::success(vec![content]))
    }

    #[tool(
        description = "Remove a member from a group. Requires the caller to be a manager of the group (privileged operation)."
    )]
    async fn wiki_group_remove_member(
        &self,
        Parameters(params): Parameters<GroupMemberParams>,
        ext: Extensions,
    ) -> Result<CallToolResult, McpError> {
        let ctx = self.ctx_for(&ext);
        let group = self
            .service
            .group_remove_member(&GroupId::new(params.id), &params.member, &ctx)
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        let output = group_to_json(&group);
        let content =
            Content::json(output).map_err(|e| McpError::internal_error(e.to_string(), None))?;
        Ok(CallToolResult::success(vec![content]))
    }

    #[tool(description = "List all groups with their members and managers.")]
    async fn wiki_group_list(&self, ext: Extensions) -> Result<CallToolResult, McpError> {
        let ctx = self.ctx_for(&ext);
        let groups = self
            .service
            .group_list(&ctx)
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        let output: Vec<serde_json::Value> = groups.iter().map(group_to_json).collect();
        let content =
            Content::json(output).map_err(|e| McpError::internal_error(e.to_string(), None))?;
        Ok(CallToolResult::success(vec![content]))
    }

    #[tool(
        description = "Resolve a page's secret REFERENCE to its value (Spec 4). This is the ONLY way to obtain a secret value; wiki_read never returns values. SENSITIVITY CONTRACT: the returned `value` is a live credential — use it ONLY for the immediate operation, NEVER echo it into visible output, chat, logs, or a wiki page, and never store it. Access is gated: you must be able to see the page. Every call is audit-logged."
    )]
    async fn wiki_get_secret(
        &self,
        Parameters(params): Parameters<GetSecretParams>,
        ext: Extensions,
    ) -> Result<CallToolResult, McpError> {
        let ctx = self.ctx_for(&ext);
        let slug =
            Slug::new(&params.slug).map_err(|e| McpError::internal_error(e.to_string(), None))?;
        match self.service.get_secret(&slug, &params.name, &ctx).await {
            Ok(secret) => {
                let value = serde_json::json!({
                    "found": true,
                    "slug": params.slug,
                    "name": params.name,
                    "value": secret.expose(),
                    "sensitivity": "SECRET — do not echo, log, or store this value. Use it only for the immediate operation.",
                });
                let content = Content::json(value)
                    .map_err(|e| McpError::internal_error(e.to_string(), None))?;
                Ok(CallToolResult::success(vec![content]))
            }
            // Page invisible/absent OR the named ref is absent → clean not-found,
            // mirroring wiki_read (never a raw crash, never leaks existence).
            Err(mind_palace_core::error::MindPalaceError::PageNotFound(_)) => {
                let value = serde_json::json!({
                    "found": false,
                    "slug": params.slug,
                    "name": params.name,
                    "message": "No such secret reference, or the page is not accessible with the current identity.",
                });
                let content = Content::json(value)
                    .map_err(|e| McpError::internal_error(e.to_string(), None))?;
                Ok(CallToolResult::success(vec![content]))
            }
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }
}

// --- ServerHandler ---

#[tool_handler]
impl ServerHandler for MindPalaceMcpServer {
    fn get_info(&self) -> ServerInfo {
        let mut info = ServerInfo::new(ServerCapabilities::builder().enable_tools().build());
        info.instructions = Some(SERVER_INSTRUCTIONS.to_string());
        info
    }
}

// --- Helpers ---

fn lint_issues_to_json(issues: &[mind_palace_core::domain::lint::LintIssue]) -> serde_json::Value {
    serde_json::Value::Array(
        issues
            .iter()
            .map(|i| {
                serde_json::json!({
                    "code": format!("{:?}", i.code),
                    "severity": format!("{:?}", i.severity),
                    "message": i.message,
                })
            })
            .collect(),
    )
}

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

fn parse_principal(kind: &str, value: &str) -> Result<Principal, String> {
    match kind {
        "user" => Ok(Principal::User(value.to_string())),
        "group" => Ok(Principal::Group(GroupId::new(value.to_string()))),
        other => Err(format!(
            "invalid principal_type '{other}' (expected 'user' or 'group')"
        )),
    }
}

fn group_to_json(group: &mind_palace_core::domain::group::Group) -> serde_json::Value {
    serde_json::json!({
        "id": group.id.as_str(),
        "name": group.name,
        "members": group.members,
        "managers": group.managers,
    })
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

# AGENTS.md — Mind Palace Integration Guide

This file is for LLM agents integrating with or building on the Mind Palace crate.

## What This Is

Mind Palace is a wiki-style hierarchical knowledge base for LLM agents. It stores pages in S3, graph metadata in DynamoDB, and embeddings in S3 Vectors. Agents interact via 6 tools or RAG context injection.

## Quick Integration (Rust + Rig SDK)

Add to your `Cargo.toml`:
```toml
[dependencies]
mind-palace = { git = "https://github.com/jdwilliams/mind-palace", path = "crates/mind-palace" }
```

### Minimal Working Example

```rust
use mind_palace::{MindPalace, MindPalaceBuilder, S3Config, DynamoConfig, S3VectorsConfig, BedrockConfig};
use mind_palace::core::domain::tenant::TenantContext;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let palace = MindPalaceBuilder::new()
        .s3(S3Config { bucket_name: "your-bucket".into(), region: "us-east-1".into(), prefix: "v1".into() })
        .dynamo(DynamoConfig { table_name: "your-table".into(), region: "us-east-1".into() })
        .s3vectors(S3VectorsConfig { bucket_name: "your-vectors".into(), index_name: "wiki".into(), region: "us-east-1".into() })
        .bedrock(BedrockConfig { model_id: "amazon.titan-embed-text-v2:0".into(), region: "us-east-1".into() })
        .build()
        .await?;

    // Access the wiki service directly
    let service = palace.wiki_service();

    // Or use tools (for Rig agent registration)
    let tools = palace.tools();
    // tools.search, tools.read, tools.traverse, tools.create, tools.update, tools.list

    Ok(())
}
```

## Architecture (for understanding the code)

```
crates/
├── mind-palace-core/     # DOMAIN LAYER — start here
│   src/domain/
│   ├── page.rs           # Page entity (id, slug, title, summary, sections, visibility)
│   ├── tenant.rs         # TenantContext — controls visibility (global, leaf tenant, parent tenant)
│   ├── value_objects.rs  # PageId, Slug, TenantId, Visibility, PageType, Section, etc.
│   ├── graph.rs          # KnowledgeGraph (in-memory petgraph, tenant-scoped traversal)
│   ├── lint.rs           # lint_page() — validates pages, returns LintIssue[]
│   └── service.rs        # WikiService — the main orchestrator (create/read/update/delete/search/traverse)
│   src/ports/            # TRAIT INTERFACES (hexagonal architecture)
│   ├── page_store.rs     # PageStore trait
│   ├── vector_search.rs  # VectorSearchPort trait
│   ├── embedding.rs      # EmbeddingPort trait
│   └── graph.rs          # GraphStore trait
├── mind-palace-infra/    # AWS ADAPTERS (implement the port traits)
├── mind-palace-rig/      # RIG SDK TOOLS (6 Tool impls + VectorStoreIndex)
├── mind-palace-mcp/      # MCP SERVER (stdio, for CLI agents)
├── mind-palace-web/      # WEB API SERVER (Axum + Google SSO)
└── mind-palace/          # FACADE (MindPalaceBuilder → MindPalace)
```

## Key Types

| Type | Location | Purpose |
|------|----------|---------|
| `WikiService` | `core/domain/service.rs` | Main API — all operations go through here |
| `Page` | `core/domain/page.rs` | A wiki page with sections |
| `TenantContext` | `core/domain/tenant.rs` | Who's asking + what they can see |
| `KnowledgeGraph` | `core/domain/graph.rs` | In-memory graph for fast traversal |
| `ReadLevel` | `core/domain/page.rs` | `Summary`, `Section(heading)`, `Full` |
| `Visibility` | `core/domain/value_objects.rs` | `General` or `Tenant(id)` |
| `PageType` | `core/domain/value_objects.rs` | `Index`, `Concept`, `Entity`, `Decision`, `Leaf` |
| `Slug` | `core/domain/value_objects.rs` | Validated URL slug (lowercase, hyphens, no spaces) |

## WikiService Methods

```rust
// Create a page (validates, lints, embeds, saves to S3/DDB/Vectors, updates graph)
service.create_page(CreatePageInput { title, slug, summary, sections, page_type, visibility, links }, &ctx) -> (Page, Vec<LintIssue>)

// Read at different token-cost levels
service.read_page(&slug, ReadLevel::Summary, &ctx) -> PageResponse::Summary { title, slug, summary, page_type }
service.read_page(&slug, ReadLevel::Section("Details".into()), &ctx) -> PageResponse::Section { heading, content }
service.read_page(&slug, ReadLevel::Full, &ctx) -> PageResponse::Full(page)

// Semantic search (embeds query, searches S3 Vectors)
service.search("rust ownership", &ctx, 5) -> Vec<SearchResult>

// Graph traversal (BFS from a page, returns neighbor summaries)
service.traverse(&slug, depth, &ctx) -> Vec<NeighborInfo>

// Update (re-lints, re-embeds, version bump)
service.update_page(&slug, UpdatePageInput { title, summary, sections, links }, &ctx) -> (Page, Vec<LintIssue>)

// Delete (removes from S3, DDB, Vectors, graph)
service.delete_page(&slug, &ctx)

// List (uses in-memory graph, no S3 calls)
service.list_pages(&filter, &ctx) -> Vec<Page>
```

## Multi-Tenancy

```rust
// No tenancy (single user, everything visible)
let ctx = TenantContext::global();

// Leaf tenant (sees own pages + General)
let ctx = TenantContext::leaf(TenantId::new("client-a"));

// Parent tenant (sees own + all children)
let ctx = TenantContext::parent(TenantId::new("dashlx"), vec![
    TenantId::new("client-a"),
    TenantId::new("client-b"),
]);
```

## Page Hierarchy Pattern

Agents should follow this pattern for maximum context efficiency:
1. **Index pages** — lightweight, link to concepts. Agent reads these first.
2. **Concept pages** — mid-level synthesis. Agent reads summary, requests sections if needed.
3. **Leaf pages** — full detail. Agent only reads when it needs deep specifics.

This minimizes token cost: most interactions only need Index + Concept summaries.

## Linting Rules

Pages are validated on create/update. Issues returned to the agent:
- `MissingSummary` (Error) — every page needs a summary
- `MissingToc` (Error) — needs at least one section
- `EmptySection` (Warning) — section has no content
- `BrokenLink` (Warning) — links to a slug that doesn't exist in the graph
- `Orphan` (Warning) — page isn't connected to the graph
- `TitleSlugMismatch` (Info) — slug doesn't match lowercased title

## AWS Infrastructure

Deploy with: `cd infra/ && sam build && sam deploy --guided`

Required resources (all free-tier friendly):
- S3 bucket (page content, versioned)
- DynamoDB table (PK=String, SK=String, on-demand)
- S3 Vector bucket + index (1024 dims, cosine)
- Bedrock Titan Embeddings v2 model access

## Web UI (Svelte 5 Components)

Install from git in any Svelte project:
```json
"dependencies": {
  "mind-palace-ui": "github:jdwilliams/mind-palace#main"
}
```

Components: `MindPalaceProvider`, `WikiSearch`, `WikiBrowser`, `WikiPage`, `WikiEditor`, `WikiGraph`

The web API server (mind-palace-web) provides the backend these components talk to.

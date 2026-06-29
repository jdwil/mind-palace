# Mind Palace — Implementation Plan

## Problem Statement

Current agent memory/KB tools feel flat and incomplete — agents retrieve raw chunks but don't build or navigate synthesized understanding over time. Mind Palace replaces this with a wiki-style hierarchical knowledge system where lightweight indexes link to progressively richer content, agents navigate via graph traversal, and knowledge compounds with every meaningful interaction.

## Requirements

- Rust crate (library) built with DDD/hexagonal architecture
- Wiki-style pages: mandatory summary + table of contents + sectioned content
- Agents can fetch summaries, sections, or full pages (minimum token cost)
- AWS-backed: S3 (content), DynamoDB (metadata/relations), S3 Vectors (semantic search), Bedrock Titan Embeddings
- In-memory graph (petgraph) for fast traversal, loaded from DDB
- Multi-tenant optional: General vs Tenant-scoped visibility, hierarchical tenants, unified graph with visibility scoping
- Inline Rust-based linting on create/update, issues handed to agents if problems found
- Rig integration: Tool impls (primary), VectorStoreIndex impl (secondary), MCP (future/tertiary)
- Mantra: highest quality context, smallest token overhead

## Background

- Rig v0.39 `Tool` trait: `const NAME`, `type Args/Output/Error`, `fn definition()`, `fn call()`
- Rig `VectorStoreIndex` trait: `top_n(VectorSearchRequest) -> Vec<(f64, String, T)>`, `top_n_ids(...)`
- `rig-s3vectors` already exists — we can reference its patterns but our crate is independent
- `rig-bedrock` provides Bedrock embedding model support via `EmbeddingModel` trait
- `petgraph` provides directed graphs with BFS/DFS traversal iterators
- S3 Vectors: no free tier per se (pay per vector/query) but pennies at scale; vector buckets are free to create
- DynamoDB: always-free 25 RCU/WCU, 25 GB storage

## Proposed Solution

A hexagonal Rust workspace with:

- **`mind-palace-core`** — domain layer (entities, value objects, domain services, port traits)
- **`mind-palace-infra`** — adapter implementations (AWS DDB, S3, S3 Vectors, Bedrock)
- **`mind-palace-rig`** — Rig SDK integration (Tool impls, VectorStoreIndex impl)
- **`mind-palace`** — top-level facade crate re-exporting a builder/config API

```mermaid
graph TD
    subgraph Domain Core
        Page[Page Entity]
        Graph[Knowledge Graph]
        Linter[Linting Service]
        Tenant[Tenant Context]
    end

    subgraph Ports - Traits
        PS[PageStore]
        VS[VectorSearch]
        ES[EmbeddingService]
        GS[GraphStore]
    end

    subgraph Adapters
        S3[S3 PageStore]
        DDB[DynamoDB GraphStore + Metadata]
        S3V[S3 Vectors VectorSearch]
        BR[Bedrock EmbeddingService]
    end

    subgraph Rig Integration
        Tools[Wiki Tools]
        RAG[VectorStoreIndex impl]
    end

    Tools --> Domain Core
    RAG --> Domain Core
    Domain Core --> Ports - Traits
    Ports - Traits --> Adapters
```

## Task Breakdown

### Task 1: Project scaffolding and domain model

**Objective:** Set up the Rust workspace with the crate structure and define the core domain entities and value objects.

**Implementation guidance:**

- Create a Cargo workspace with 4 crates: `mind-palace-core`, `mind-palace-infra`, `mind-palace-rig`, `mind-palace` (facade)
- Define domain entities: `Page` (id, slug, title, summary, table_of_contents, sections, page_type, visibility, confidence, last_updated, version)
- Define value objects: `PageId`, `Slug`, `TenantId`, `Visibility` (General, Tenant(TenantId)), `PageType` (Index, Concept, Entity, Decision, Leaf), `Section`, `TableOfContents`
- Define `TenantContext` (current tenant, hierarchy/ancestors for parent-tenant access)
- Define the `PageContent` struct with Markdown body, frontmatter parsing

**Test requirements:**

- Unit tests for entity construction, validation (slug format, required summary, TOC generation)
- Unit tests for `TenantContext` visibility checks (can tenant A see tenant B's pages?)

**Demo:** `cargo build` succeeds. Unit tests pass showing page creation with validation and tenant visibility logic.

---

### Task 2: Port traits (hexagonal boundaries)

**Objective:** Define the port/trait interfaces that adapters will implement.

**Implementation guidance:**

- `PageStore` trait: `get_page(id)`, `get_page_by_slug(slug)`, `save_page(page)`, `delete_page(id)`, `list_pages(filter)`
- `VectorSearchPort` trait: `search(query_embedding, limit, filter) -> Vec<SearchResult>`, `upsert_embedding(page_id, embedding, metadata)`, `delete_embedding(page_id)`
- `EmbeddingPort` trait: `embed_text(text) -> Vec<f64>`, `embed_texts(texts) -> Vec<Vec<f64>>`
- `GraphStore` trait: `load_graph() -> GraphData`, `save_node(node)`, `save_edge(edge)`, `delete_node(id)`, `get_neighbors(id, direction)`
- All traits are async, use associated error types, and accept `TenantContext` where scoping applies
- Define a `MindPalaceError` enum for domain-level errors

**Test requirements:**

- Compile-time verification (traits are object-safe where needed)
- Mock implementations for each port for testing

**Demo:** `cargo build` succeeds. Port traits defined with mock impls. Domain services can be tested against mocks.

---

### Task 3: In-memory knowledge graph with petgraph

**Objective:** Build the in-memory graph service that enables fast traversal with tenant-scoped visibility.

**Implementation guidance:**

- Use `petgraph::DiGraph` with node weights as `GraphNode` (page_id, slug, title, summary, visibility, page_type) and edge weights as `EdgeRelation` (parent, child, related, backlink)
- `KnowledgeGraph` struct wraps the graph + a `HashMap<PageId, NodeIndex>` for O(1) lookups
- Implement traversal methods: `get_neighbors(page_id, direction, tenant_ctx)`, `find_path(from, to, tenant_ctx)`, `get_subtree(root, depth, tenant_ctx)`, `get_index_pages(tenant_ctx)`
- All traversal respects `TenantContext` visibility (filter nodes caller can't see)
- Implement `add_node`, `remove_node`, `add_edge`, `remove_edge` for mutations
- The graph is loaded from `GraphStore` on initialization and kept in sync via mutations

**Test requirements:**

- Unit tests for graph construction, node/edge CRUD
- Unit tests for visibility scoping (sibling tenants can't see each other, parent sees children, General visible to all)
- Unit tests for traversal (BFS from index page returns correct hierarchy)

**Demo:** In-memory graph can be built, mutated, and traversed with proper tenant isolation.

---

### Task 4: Linting service

**Objective:** Implement inline Rust-based linting that validates pages on create/update and returns actionable issues.

**Implementation guidance:**

- `LintService` with a `lint_page(page) -> Vec<LintIssue>` method
- Lint rules: missing summary, missing TOC, broken internal links (validated against graph), orphan detection (no incoming edges), empty sections, title/slug mismatch
- `LintIssue` enum with severity (Error, Warning, Info) and machine-readable codes
- Linting runs synchronously (Rust speed makes this instant for single pages)
- Returns issues that can be serialized and handed to an agent for fixing

**Test requirements:**

- Unit tests for each lint rule
- Test that a well-formed page passes all lints

**Demo:** Create a page with missing summary → linter catches it. Lint results are structured data an agent could act on.

---

### Task 5: Domain services (orchestration layer)

**Objective:** Build the domain services that orchestrate page CRUD, graph updates, embedding management, and linting into cohesive operations.

**Implementation guidance:**

- `WikiService` struct holding references to all ports + the `KnowledgeGraph`
- Methods: `create_page(input)`, `update_page(slug, input)`, `read_page(slug, read_level, tenant_ctx)`, `search(query, tenant_ctx, limit)`, `traverse(from_slug, depth, tenant_ctx)`, `delete_page(slug)`
- All methods respect tenant context

**Test requirements:**

- Integration tests using mock ports
- Test read_page at different levels returns appropriate content size
- Test search respects tenant visibility

**Demo:** Full create → search → read → traverse flow works against mocks.

---

### Task 6: S3 adapter (PageStore)

**Objective:** Implement the `PageStore` port against AWS S3.

**Implementation guidance:**

- Store pages as Markdown files with YAML frontmatter at keys like `{tenant_id}/pages/{slug}.md`
- Use `aws-sdk-s3` for GetObject, PutObject, DeleteObject, ListObjectsV2
- Configuration: bucket name, region, prefix

---

### Task 7: DynamoDB adapter (GraphStore + metadata)

**Objective:** Implement the `GraphStore` port against DynamoDB.

**Implementation guidance:**

- Single-table design: PK = `PAGE#{page_id}`, SK for different record types (`META`, `EDGE#{target_id}`, `BACKLINK#{source_id}`)
- GSI1: PK = `TENANT#{tenant_id}`, SK = `TYPE#{page_type}#SLUG#{slug}`
- GSI2: PK = `SLUG#{slug}`, SK = `TENANT#{tenant_id}`
- Use `aws-sdk-dynamodb`

---

### Task 8: S3 Vectors adapter (VectorSearchPort)

**Objective:** Implement the `VectorSearchPort` against S3 Vector buckets.

**Implementation guidance:**

- Use `aws-sdk-s3vectors` for put_vectors, query_vectors, delete_vectors
- Store embeddings with metadata: page_id, slug, title, tenant_id, page_type, visibility
- Use tenant_id + visibility as filterable metadata for scoped queries

---

### Task 9: Bedrock adapter (EmbeddingPort)

**Objective:** Implement the `EmbeddingPort` against Amazon Bedrock Titan Embeddings v2.

**Implementation guidance:**

- Model ID: `amazon.titan-embed-text-v2:0`
- Implement batching via parallel tokio tasks
- Normalize embeddings for cosine similarity

---

### Task 10: Rig Tool implementations

**Objective:** Expose Mind Palace operations as Rig `Tool` impls.

**Tools:**

- `WikiSearchTool` — semantic search, returns ranked summaries
- `WikiReadTool` — read page at specified level (summary, section, full)
- `WikiTraverseTool` — graph traversal from a page
- `WikiCreateTool` — create new page
- `WikiUpdateTool` — update existing page, returns lint issues
- `WikiListTool` — list pages by type or category

---

### Task 11: Rig VectorStoreIndex implementation

**Objective:** Implement Rig's `VectorStoreIndex` trait so Mind Palace can serve as a RAG context source.

**Implementation guidance:**

- `MindPalaceVectorIndex` wraps `WikiService`
- `top_n` returns page summaries (not full pages — token efficiency)
- `top_n_ids` returns page slugs

---

### Task 12: Facade crate and builder API

**Objective:** Create the top-level `mind-palace` crate with an ergonomic builder/configuration API.

**Implementation guidance:**

- `MindPalaceBuilder` with `.with_s3(config)`, `.with_dynamodb(config)`, `.with_s3vectors(config)`, `.with_bedrock(config)`, `.with_tenant(tenant_ctx)`, `.enable_tenancy(bool)`, `.build()`
- `MindPalace` struct exposes: `.wiki_service()`, `.tools()`, `.vector_index()`, `.graph()`

---

### Task 13: End-to-end integration test

**Objective:** Validate the full flow works together.

**Scenarios:**

1. Creates Index → Concept → Leaf pages with proper links
2. Verifies graph is connected correctly
3. Searches semantically → gets relevant result
4. Traverses from Index → Concept → Leaf via graph
5. Reads at summary level (small), then full level (large) — asserts token difference
6. Updates a page → verifies linting runs and graph/embeddings update
7. Tests tenant isolation

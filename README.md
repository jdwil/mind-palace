# Mind Palace

A wiki-style hierarchical knowledge base for LLM agents, built as a [Rig](https://rig.rs) SDK plugin in Rust.

Agents navigate lightweight indexes → concept pages → detailed leaves, fetching only the context they need. Knowledge compounds over time through structured pages, graph links, and semantic search.

## Design Principles

- **Maximum context quality, minimum token overhead** — progressive disclosure via summary/section/full reads
- **Hexagonal architecture** — domain logic is pure, adapters are swappable
- **Multi-tenant optional** — hierarchical tenant visibility or single-tenant mode
- **AWS free-tier friendly** — S3, DynamoDB, S3 Vectors, Bedrock Titan Embeddings

## Quick Start

```rust
let palace = MindPalace::builder()
    .s3(S3Config { bucket_name: "my-pages".into(), region: "us-east-1".into(), prefix: "v1".into() })
    .dynamo(DynamoConfig { table_name: "my-graph".into(), region: "us-east-1".into() })
    .s3vectors(S3VectorsConfig { bucket_name: "my-vectors".into(), index_name: "wiki".into(), region: "us-east-1".into() })
    .bedrock(BedrockConfig { model_id: "amazon.titan-embed-text-v2:0".into(), region: "us-east-1".into() })
    .build()
    .await?;

// Use as Rig tools
let agent = client.agent("claude-sonnet-4-20250514")
    .tool(palace.tools().search)
    .tool(palace.tools().read)
    .tool(palace.tools().traverse)
    .tool(palace.tools().create)
    .tool(palace.tools().update)
    .build();

// Or as RAG context source
let agent = client.agent("claude-sonnet-4-20250514")
    .dynamic_context(2, palace.vector_index())
    .build();
```

## Infrastructure

Deploy the required AWS resources:

```bash
cd infra/
sam build && sam deploy --guided
```

See [docs/infrastructure.md](docs/infrastructure.md) for details on what gets created and cost estimates (~$0.06/month for personal use).

## Architecture

```
mind-palace/
├── crates/
│   ├── mind-palace-core/    # Domain: entities, graph, linting, service, ports
│   ├── mind-palace-infra/   # AWS adapters: S3, DynamoDB, S3 Vectors, Bedrock
│   ├── mind-palace-rig/     # Rig SDK: 6 tools + VectorStoreIndex
│   └── mind-palace/         # Facade: builder API
├── docs/
│   ├── spec.md              # Full implementation spec
│   └── infrastructure.md    # AWS resource requirements
└── infra/
    └── template.yaml        # SAM/CloudFormation template
```

## Agent Tools

| Tool | Purpose | Token Cost |
|------|---------|-----------|
| `wiki_search` | Semantic search → ranked summaries | Low |
| `wiki_read` | Read page at summary/section/full level | Variable |
| `wiki_traverse` | Graph walk → neighboring page summaries | Low |
| `wiki_create` | Create new page, returns lint issues | Medium |
| `wiki_update` | Update page, returns lint issues | Medium |
| `wiki_list` | List pages by type | Low |

## License

MIT

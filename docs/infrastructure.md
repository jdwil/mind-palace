# Infrastructure Requirements

Mind Palace requires the following AWS resources. All fit comfortably within AWS free tier at personal/small-team scale.

## Resources

### 1. S3 Bucket (Page Content)

Stores wiki pages as Markdown files with JSON frontmatter.

- **Bucket name:** configurable (e.g., `mind-palace-pages`)
- **Versioning:** enabled (for page history/rollback)
- **Encryption:** AES-256 (SSE-S3)
- **Key format:** `{prefix}/{tenant_id}/pages/{slug}.md` or `{prefix}/general/pages/{slug}.md`
- **Free tier:** 5 GB storage, 20k GET, 2k PUT/month

### 2. DynamoDB Table (Graph Metadata)

Stores page metadata, edges, and backlinks in a single-table design.

- **Table name:** configurable (e.g., `mind-palace-graph`)
- **Billing mode:** PAY_PER_REQUEST (on-demand) — no provisioning needed
- **Key schema:**
  - Partition Key (PK): `String` — format `PAGE#{uuid}`
  - Sort Key (SK): `String` — format `META`, `EDGE#{uuid}`, or `BACKLINK#{uuid}`
- **No GSIs required for the base implementation** (GSIs can be added later for listing by tenant/type)
- **Free tier:** 25 RCU/WCU, 25 GB storage (always free)

#### Item Patterns

| PK | SK | Description |
|----|----|----|
| `PAGE#{id}` | `META` | Page metadata (slug, title, summary, visibility, page_type) |
| `PAGE#{id}` | `EDGE#{target_id}` | Outgoing edge (edge_kind attribute) |
| `PAGE#{id}` | `BACKLINK#{source_id}` | Incoming edge (reverse pointer) |

### 3. S3 Vector Bucket + Index (Semantic Search)

Stores page embeddings for similarity search.

- **Vector bucket name:** configurable (e.g., `mind-palace-vectors`)
- **Index name:** configurable (e.g., `wiki-pages`)
- **Dimensions:** 1024 (Titan Embeddings v2)
- **Distance metric:** cosine
- **Vector bucket:** free to create
- **Cost:** ~$0.06/GB storage, $2.50/M queries (pennies at personal scale)

### 4. Bedrock Model Access

Used to generate text embeddings.

- **Model:** `amazon.titan-embed-text-v2:0`
- **Region:** must be a region where Titan Embeddings is available (us-east-1, us-west-2, etc.)
- **Setup:** Enable model access in the Bedrock console (one-time, no cost)
- **Cost:** ~$0.00002 per 1k input tokens

## Deployment

### Option A: SAM Template (recommended)

```bash
cd infra/
sam build
sam deploy --guided
```

See `infra/template.yaml` for the full CloudFormation/SAM template.

### Option B: Manual Setup

1. Create S3 bucket with versioning enabled
2. Create DynamoDB table with PK (String) + SK (String), on-demand billing
3. Create S3 Vector bucket and index (1024 dims, cosine)
4. Enable Bedrock Titan Embeddings v2 model access in your region
5. Ensure your execution role/credentials have access to all four services

## IAM Permissions Required

The runtime role needs:

```json
{
  "Effect": "Allow",
  "Action": [
    "s3:GetObject", "s3:PutObject", "s3:DeleteObject", "s3:ListBucket",
    "dynamodb:Scan", "dynamodb:PutItem", "dynamodb:DeleteItem", "dynamodb:Query", "dynamodb:BatchWriteItem",
    "s3vectors:PutVectors", "s3vectors:QueryVectors", "s3vectors:DeleteVectors",
    "bedrock:InvokeModel"
  ],
  "Resource": "*"
}
```

Narrow `Resource` to specific ARNs in production.

## Configuration

Pass config to the builder:

```rust
let palace = MindPalace::builder()
    .s3(S3Config {
        bucket_name: "mind-palace-pages".into(),
        region: "us-east-1".into(),
        prefix: "v1".into(),
    })
    .dynamo(DynamoConfig {
        table_name: "mind-palace-graph".into(),
        region: "us-east-1".into(),
    })
    .s3vectors(S3VectorsConfig {
        bucket_name: "mind-palace-vectors".into(),
        index_name: "wiki-pages".into(),
        region: "us-east-1".into(),
    })
    .bedrock(BedrockConfig {
        model_id: "amazon.titan-embed-text-v2:0".into(),
        region: "us-east-1".into(),
    })
    .enable_tenancy(false)
    .build()
    .await?;
```

## Cost Estimate (Personal Use)

At ~100 wiki pages, ~50 queries/day:

| Service | Monthly Cost |
|---------|-------------|
| S3 (pages) | $0.00 (free tier) |
| DynamoDB | $0.00 (free tier) |
| S3 Vectors | ~$0.01 |
| Bedrock Embeddings | ~$0.05 |
| **Total** | **~$0.06/month** |

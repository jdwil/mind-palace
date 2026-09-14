# Terraform Module: Mind Palace Remote MCP

Deploys `mind-palace-mcp-remote` as an ECS Fargate service behind an
Application Load Balancer. Reusable across projects.

## What it creates

- ECS cluster + Fargate service (long-lived) running the remote MCP server
- Application Load Balancer (internal by default) with `/health` checks
- Security groups (ALB → service on the container port)
- Task IAM role with Mind Palace runtime permissions (S3 pages, DynamoDB,
  S3 Vectors, Bedrock embeddings)
- CloudWatch log group
- Optional Secrets Manager secret for the bearer auth token

## Prerequisites

- A VPC with subnets
- The Mind Palace backend resources already provisioned (pages bucket,
  vectors bucket + index, graph table) — e.g. via the core `infra/` config
- The `mind-palace-mcp-remote` Docker image pushed to ECR

## Usage — VPN / private (no auth)

The intended default: internal ALB, reachable only inside your network, no token.

```hcl
module "mind_palace_mcp" {
  source = "github.com/jdwil/mind-palace//infra/modules/remote-mcp"

  name   = "mind-palace-mcp"
  region = "us-east-1"

  vpc_id         = "vpc-0123"
  subnet_ids     = ["subnet-priv-a", "subnet-priv-b"]
  alb_subnet_ids = ["subnet-priv-a", "subnet-priv-b"]
  internal       = true
  ingress_cidrs  = ["10.0.0.0/8"] # your VPC/VPN CIDR

  image = "123456789012.dkr.ecr.us-east-1.amazonaws.com/mind-palace-mcp-remote:latest"

  pages_bucket   = "mind-palace-pages-prod-123456789012"
  vectors_bucket = "mind-palace-vectors-prod-123456789012"
  graph_table    = "mind-palace-graph-prod"

  auth_token = null # auth disabled — network is the boundary
}

output "mcp_url" {
  value = module.mind_palace_mcp.endpoint_url
}
```

## Usage — with bearer auth

For a less-trusted network, set a token (stored in Secrets Manager, injected as
`MP_AUTH_TOKEN`). Clients must send `Authorization: Bearer <token>`.

```hcl
module "mind_palace_mcp" {
  source = "github.com/jdwil/mind-palace//infra/modules/remote-mcp"

  name           = "mind-palace-mcp"
  region         = "us-east-1"
  vpc_id         = "vpc-0123"
  subnet_ids     = ["subnet-priv-a", "subnet-priv-b"]
  alb_subnet_ids = ["subnet-pub-a", "subnet-pub-b"]
  internal       = false
  ingress_cidrs  = ["0.0.0.0/0"]

  image          = "123456789012.dkr.ecr.us-east-1.amazonaws.com/mind-palace-mcp-remote:latest"
  pages_bucket   = "mind-palace-pages-prod-123456789012"
  vectors_bucket = "mind-palace-vectors-prod-123456789012"
  graph_table    = "mind-palace-graph-prod"

  auth_token = var.mcp_auth_token # sensitive; pass via TF_VAR or secret backend
}
```

> If `internal = false`, always set `auth_token`. An internet-facing endpoint
> with auth disabled is unsafe.

## Connecting a client

Use the `endpoint_url` output. Example Kiro agent config:

```json
{
  "mcpServers": {
    "mind-palace": {
      "url": "http://mind-palace-mcp-xxxx.us-east-1.elb.amazonaws.com/mcp",
      "headers": { "Authorization": "Bearer <token>" }
    }
  }
}
```

Omit `headers` when auth is disabled.

## Inputs

See `variables.tf`. Key ones: `name`, `region`, `vpc_id`, `subnet_ids`,
`alb_subnet_ids`, `image`, `pages_bucket`, `vectors_bucket`, `graph_table`,
`auth_token` (optional), `internal` (default true).

## Outputs

| Output | Description |
|--------|-------------|
| `endpoint_url` | MCP URL to give clients |
| `health_url` | Health check URL |
| `alb_dns_name` | Load balancer DNS |
| `cluster_name` / `service_name` | ECS identifiers |
| `auth_enabled` | Whether bearer auth is on |
| `task_role_arn` | Task role (Mind Palace runtime perms) |

## Notes

- **HTTPS:** this template terminates HTTP at the ALB. For a public endpoint,
  add an ACM cert + HTTPS listener (443) and redirect 80→443. Left out here to
  keep the template minimal and because the default (internal/VPN) case doesn't
  require TLS at the ALB.
- **Image builds:** build/push `mind-palace-mcp-remote` from
  `crates/mind-palace-mcp/Dockerfile.remote`.

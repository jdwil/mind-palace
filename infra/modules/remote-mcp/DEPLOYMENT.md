# Deploying the Mind Palace Remote MCP Server — Agent Runbook

This is a complete, self-contained procedure for deploying `mind-palace-mcp-remote`
(the HTTP MCP server) to AWS ECS Fargate for a project. Point an agent at this
file and it should have everything needed.

## Overview

The remote MCP server lets agent harnesses connect to Mind Palace over HTTP
instead of spawning a local process. You deploy it as an ECS Fargate service
behind a load balancer. Two things must happen, in order:

1. **Build & push the Docker image to ECR** (code lifecycle — a script).
2. **Apply Terraform** that references that image (infra lifecycle — a module).

These are deliberately separate: Terraform references an image URI, it does not
build images.

## Prerequisites (gather these first)

Confirm/collect all of the following before starting. If any are unknown, ask
the operator — do not guess.

- **AWS account + region** to deploy into, and an AWS profile with credentials.
- **VPC and subnets** in that account:
  - `vpc_id`
  - `subnet_ids` — where the ECS tasks run (private subnets recommended)
  - `alb_subnet_ids` — where the load balancer lives
- **Mind Palace backend resources** the server will read/write. These must
  already exist (created by the core `infra/` config):
  - pages S3 bucket name
  - vectors S3 bucket name + index name (default `wiki-pages`)
  - graph DynamoDB table name
  - the AWS region those live in, and whether they are in the SAME account you
    are deploying into (see "Cross-account" below)
- **Auth decision**: is this behind a VPN/internal ALB (no auth), or reachable
  from a less-trusted network (requires a bearer token)?
- **Docker** available on the machine running the build step, and the AWS CLI +
  Terraform installed.

## Step 1 — Build and push the image

From the mind-palace repo root, run the AWS build/push script:

```bash
infra/modules/remote-mcp/build-and-push-aws.sh <account-id> <region> [repo-name] [tag] [aws-profile]
```

Example:

```bash
infra/modules/remote-mcp/build-and-push-aws.sh 840854325365 us-east-1 mind-palace-mcp-remote latest command-center
```

This creates the ECR repo if missing, builds `crates/mind-palace-mcp/Dockerfile.remote`,
and pushes. It prints the final **image URI** — capture it for Step 2.

## Step 2 — Write the Terraform

In the project's Terraform, add a module block referencing the pushed image.
Source the module from this repo:

```hcl
module "mind_palace_mcp" {
  source = "github.com/jdwil/mind-palace//infra/modules/remote-mcp"

  name   = "mind-palace-mcp"
  region = "us-east-1"

  # Networking (from prerequisites)
  vpc_id         = "vpc-xxxx"
  subnet_ids     = ["subnet-priv-a", "subnet-priv-b"]
  alb_subnet_ids = ["subnet-priv-a", "subnet-priv-b"]
  internal       = true                # true = VPN/private (default)
  ingress_cidrs  = ["10.0.0.0/8"]      # who can reach the ALB

  # Image from Step 1
  image = "840854325365.dkr.ecr.us-east-1.amazonaws.com/mind-palace-mcp-remote:latest"

  # Mind Palace backend (must already exist)
  pages_bucket   = "mind-palace-pages-prod-840854325365"
  vectors_bucket = "mind-palace-vectors-prod-840854325365"
  vectors_index  = "wiki-pages"
  graph_table    = "mind-palace-graph-prod"

  # Auth: null = disabled (VPN case). Set a token for public/less-trusted.
  auth_token = null
}

output "mcp_url" {
  value = module.mind_palace_mcp.endpoint_url
}
```

**Rules:**
- If `internal = false` (internet-facing), you MUST set `auth_token`.
- Full input/output reference is in `infra/modules/remote-mcp/README.md`.

## Step 3 — Apply

```bash
terraform init
terraform plan     # review: an ECS cluster, service, task def, ALB, IAM, SG, log group
terraform apply
```

The `mcp_url` output is the endpoint. Verify health:

```bash
curl <alb-dns>/health   # expect: ok
```

## Step 4 — Connect a client

Point the agent harness at the `mcp_url`. Example (Kiro):

```json
{
  "mcpServers": {
    "mind-palace": {
      "url": "http://<alb-dns>/mcp",
      "headers": { "Authorization": "Bearer <token>" }
    }
  }
}
```

Omit `headers` if auth is disabled.

## Cross-account note (IMPORTANT)

The module's task IAM role grants access to the buckets/table BY NAME in the
account you deploy into. If the Mind Palace backend lives in a DIFFERENT account
than this deployment (e.g. backend in `command-center`, service deployed in a
product account), the task will be making cross-account calls that the IAM role
alone cannot authorize. In that case you also need:
- The S3 bucket policies (pages + vectors) to allow the task role.
- The DynamoDB table — cross-account access requires the table's resource policy
  (or deploy the service in the same account as the backend).

If backend and service are in the SAME account, no extra policies are needed.
When unsure, deploy the service into the same account as the Mind Palace backend.

## What gets created

ECS cluster, Fargate service + task definition, Application Load Balancer
(internal by default) with a `/health` check, security groups (ALB → service),
task IAM role with Mind Palace runtime permissions, CloudWatch log group, and —
only if `auth_token` is set — a Secrets Manager secret.

## HTTPS

The template terminates HTTP at the ALB. For a public HTTPS endpoint, add an
ACM certificate + a 443 listener and redirect 80→443. Not included to keep the
template minimal; the default internal/VPN case does not require it.

## Teardown

```bash
terraform destroy
```

The ECR repo and any images are NOT managed by Terraform (created by the build
script), so delete them separately if desired.

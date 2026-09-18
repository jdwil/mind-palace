# Deploying the Remote MCP Server with OIDC (Cognito) Auth

End-to-end runbook for deploying `mind-palace-mcp-remote` with per-user identity
via an OIDC provider. Examples use **AWS Cognito federated to Google Workspace**
(one consumer's setup); the server itself is provider-agnostic — it only speaks
OIDC/JWT, and all provider specifics are configuration.

Read `README-remote.md` (same directory as the binary crate) for the full env
var reference. This runbook is the ordered procedure.

## Prerequisites

- Mind Palace backend already provisioned (pages bucket, vectors bucket+index,
  graph table) — from the core `infra/` config.
- `mind-palace-mcp-remote` Docker image built and pushed to ECR
  (`build-and-push-aws.sh`).
- A VPC with subnets, and an **ACM certificate** for the hostname you'll serve
  (HTTPS is mandatory for OIDC — bearer JWTs must not travel plaintext).
- A DNS name you control for the endpoint (e.g. `mind-palace.example.com`).

## Step 1 — Identity provider (Cognito) setup [AWS-side, not in code]

1. **User pool** with your IdP federated (e.g. Google Workspace as a SAML/OIDC
   IdP). This is where humans sign in. Note the **user pool ID** and **region**.
   - Issuer URL = `https://cognito-idp.<region>.amazonaws.com/<userPoolId>`
   - JWKS = `<issuer>/.well-known/jwks.json` (the server derives this by default)
2. **App client for interactive users** (authorization-code grant):
   - Allowed OAuth flows: authorization code grant.
   - Scopes: `openid`, `email`, `profile`, `offline_access`.
   - Callback URLs: include the MCP clients' loopback redirect
     (`http://127.0.0.1:<port>/oauth/callback`) OR enable Dynamic Client
     Registration if your clients use it. (Kiro/Claude Desktop use a loopback.)
   - Note the **app client ID** → this is an allowed audience.
3. **App client(s) for M2M agents** (client-credentials grant):
   - One per autonomous VM agent. Give each its own identity (e.g. its own
     Google Workspace email alias) so tokens are attributable.
   - Note each **client ID** → add to allowed audiences.
4. Ensure the token carries an **email** claim for humans (map the IdP email
   into the token). M2M tokens without email fall back to `sub`.

## Step 2 — Deploy with the Terraform module

Use `infra/modules/remote-mcp` with `auth_mode = "oidc"`:

```hcl
module "mind_palace_mcp" {
  source = "github.com/jdwil/mind-palace//infra/modules/remote-mcp"

  name   = "mind-palace-mcp"
  region = "us-east-1"

  vpc_id         = "vpc-xxxx"
  subnet_ids     = ["subnet-priv-a", "subnet-priv-b"]
  alb_subnet_ids = ["subnet-pub-a", "subnet-pub-b"]  # or private for internal
  internal       = false                              # or true for VPN-only
  ingress_cidrs  = ["0.0.0.0/0"]                       # tighten as appropriate

  image = "<acct>.dkr.ecr.us-east-1.amazonaws.com/mind-palace-mcp-remote:latest"

  pages_bucket   = "mind-palace-pages-prod-<acct>"
  vectors_bucket = "mind-palace-vectors-prod-<acct>"
  graph_table    = "mind-palace-graph-prod"

  # HTTPS is REQUIRED for oidc (the module will fail the plan otherwise)
  acm_certificate_arn = "arn:aws:acm:us-east-1:<acct>:certificate/xxxx"
  public_url          = "https://mind-palace.example.com"

  # OIDC
  auth_mode      = "oidc"
  oidc_issuer    = "https://cognito-idp.us-east-1.amazonaws.com/<userPoolId>"
  oidc_audiences = "<interactive-app-client-id>,<m2m-app-client-id>"
  # oidc_jwks_url left empty → derived from issuer (correct for Cognito)
}

output "mcp_url" { value = module.mind_palace_mcp.endpoint_url }
```

The module fails the plan (via `check` blocks) if: `auth_mode` is token/oidc
without a cert; `token` mode without a token; or `oidc` without an issuer.

Then:
```bash
terraform init && terraform plan && terraform apply
```

## Step 3 — DNS

Point your hostname (matching the ACM cert and `public_url`) at the module's
ALB DNS (`module.mind_palace_mcp.alb_dns_name`) via a CNAME/ALIAS record.

## Step 4 — Verify

```bash
# Health (open, no auth):
curl https://mind-palace.example.com/health         # -> ok

# Discovery (open) — should return protected-resource metadata:
curl https://mind-palace.example.com/.well-known/oauth-protected-resource

# MCP without a token — should be 401:
curl -s -o /dev/null -w '%{http_code}\n' -X POST \
  https://mind-palace.example.com/mcp \
  -H 'Accept: application/json, text/event-stream' \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"c","version":"1"}}}'
# -> 401
```

## Step 5 — Client config

Interactive (Kiro example — the browser sign-in fires on the first 401):

```json
{
  "mcpServers": {
    "mind-palace": {
      "url": "https://mind-palace.example.com/mcp",
      "oauth": {
        "clientId": "<interactive-app-client-id>",
        "oauthScopes": ["openid", "email", "profile", "offline_access"]
      }
    }
  }
}
```

M2M agents: obtain a token via client-credentials from Cognito's token endpoint
and send it as `Authorization: Bearer <jwt>` (their harness/runtime handles the
refresh). Store the client secret in the VM's instance role / a secret store,
never in the image.

## Auth mode quick reference

| Goal | `auth_mode` | Also set | HTTPS |
|------|-------------|----------|-------|
| VPN/private, public pages only | `none` | — | optional |
| Simple shared secret | `token` | `auth_token` | required |
| Per-user identity (recommended) | `oidc` | `oidc_issuer`, `oidc_audiences` | required |

## Notes / gotchas

- **HTTPS is enforced** for token/oidc by a Terraform `check` — supply
  `acm_certificate_arn`.
- `public_url` MUST be the real external HTTPS URL; it is advertised in the
  OIDC discovery metadata that tells clients where to sign in. If omitted it
  defaults to the ALB DNS with the right scheme, but a stable hostname is better.
- The task role gets Mind Palace runtime permissions (S3/DDB/Vectors/Bedrock)
  automatically. For the secrets proxy (Spec 4), also grant
  `secretsmanager:GetSecretValue` on your configured ARN prefix separately.
- No Cognito/Google strings live in the server code — everything above is
  configuration.

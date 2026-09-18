# Mind Palace Remote MCP Server

A remote (HTTP) variant of the Mind Palace MCP server. Serves the exact same
tools as the local stdio binary (`mind-palace-mcp`) over the MCP **Streamable
HTTP** transport, so agent harnesses can connect over the network instead of
spawning a local process.

Binary: `mind-palace-mcp-remote`. Same crate, same tool logic — only the
transport differs.

## When to use which

| Binary | Transport | Use for |
|--------|-----------|---------|
| `mind-palace-mcp` | stdio | Local agents (Kiro, Grok, Claude Desktop) that spawn the process |
| `mind-palace-mcp-remote` | HTTP (Streamable) | Shared deployment on EC2/ECS; many agents connect to one endpoint |

## Configuration

All the `MIND_PALACE_*` variables from the stdio server apply (bucket, table,
region, model, etc.). The remote server adds:

| Variable | Default | Purpose |
|----------|---------|---------|
| `MP_BIND_ADDR` | `0.0.0.0:8080` | Listen address |
| `MP_MCP_PATH` | `/mcp` | HTTP path serving MCP |
| `MP_PUBLIC_URL` | `http://<bind_addr>` | This server's externally reachable URL. **Set this in production** — it is advertised as the resource in the OIDC discovery metadata. |
| `MP_ALLOWED_HOSTS` | *(unset)* | Optional comma-separated `Host` allowlist. Unset = host validation disabled (needed when reached via an ALB/hostname). |
| `MP_AUTH_MODE` | `none` | Auth mode: `none` \| `token` \| `oidc` (see below). |
| `MP_AUTH_TOKEN` | *(unset)* | Shared bearer token. Required when `MP_AUTH_MODE=token`. |

### OIDC variables (only when `MP_AUTH_MODE=oidc`)

| Variable | Required | Default | Purpose |
|----------|----------|---------|---------|
| `MP_OIDC_ISSUER` | **yes** | — | OIDC issuer URL. For Cognito: `https://cognito-idp.<region>.amazonaws.com/<userPoolId>` |
| `MP_OIDC_JWKS_URL` | no | `<issuer>/.well-known/jwks.json` | JWKS endpoint (Cognito follows the default convention). |
| `MP_OIDC_AUDIENCES` | recommended | *(any)* | Comma-separated allowed `aud`/`client_id` values = your app client ID(s). If unset, audience is not checked — set it in production. |
| `MP_OIDC_EMAIL_CLAIM` | no | `email` | JWT claim used as the user identity. |
| `MP_OIDC_FALLBACK_CLAIM` | no | `sub` | Identity claim used when the email claim is absent (e.g. machine-to-machine tokens). |
| `MP_OIDC_AUTH_SERVER_URL` | no | `<issuer>` | Authorization server advertised in the 401 discovery metadata. |

The server fails fast at startup if `MP_AUTH_MODE=oidc` is set without
`MP_OIDC_ISSUER`. No provider name (Cognito/Google/etc.) appears in the code —
all specifics are configuration.

On a server, AWS credentials come from the instance/task role — no SSO. The
SSO cache timezone fix is a no-op there.

## Auth model

`MP_AUTH_MODE` selects one of three modes:

1. **`none` (default) — anonymous / VPN.** No identity. **Anonymous requests can
   see ONLY fully-open (public) pages; anything restricted is invisible.** Run
   behind a VPN/private network. The server logs a warning at startup.

2. **`token` — shared bearer token.** Set `MP_AUTH_TOKEN`. Every request to the
   MCP path must carry `Authorization: Bearer <token>`. One shared identity — no
   per-user distinction. Fine for a small trusted deploy.

3. **`oidc` — per-user identity via JWT (recommended for multi-user).** The
   server validates a JWT on every request (signature via JWKS, issuer,
   audience, expiry) and derives the user's identity from the `email` claim
   (falling back to `sub` for machine-to-machine tokens). On a missing/invalid
   token it returns **401** with discovery metadata at
   `/.well-known/oauth-protected-resource`, which triggers the MCP client's
   browser sign-in flow. Per-user pages, grants, groups, and secret access all
   key off this identity.

`/health` and `/.well-known/oauth-protected-resource` are always open (no auth).

> **HTTPS is required for `token` and `oidc` modes.** Bearer tokens/JWTs must
> not travel in plaintext. Terminate TLS at the ALB (see the Terraform module).
> The `none`/VPN mode may use plain HTTP inside a private network.

### The interactive OIDC sign-in flow (what the user experiences)

1. User opens their AI harness; its base prompt says "call `wiki_instructions`
   before doing anything."
2. That call hits the server unauthenticated → **401 + discovery metadata**.
3. The MCP **client** (Kiro, Claude Desktop, …) opens a browser to the OIDC
   provider; the user signs in (e.g. with Google via Cognito).
4. The client retries with the issued JWT and works until the token expires,
   then silently refreshes (or re-prompts). Lowest-friction: no CLI login.

### Machine-to-machine (headless VM agents)

Autonomous agents use the OAuth **client-credentials** grant against the same
issuer and present a normal `Authorization: Bearer <JWT>`. Give each agent its
own identity (e.g. its own email alias) so it is attributable and its access is
managed independently.

## Run locally

```bash
MIND_PALACE_S3_BUCKET=... MIND_PALACE_DYNAMO_TABLE=... \
MIND_PALACE_VECTORS_BUCKET=... MIND_PALACE_REGION=us-east-1 \
MP_BIND_ADDR=127.0.0.1:8080 \
mind-palace-mcp-remote
```

## Docker

```bash
docker build -f crates/mind-palace-mcp/Dockerfile.remote -t mind-palace-mcp-remote .
docker run -p 8080:8080 \
  -e MIND_PALACE_S3_BUCKET=... \
  -e MIND_PALACE_DYNAMO_TABLE=... \
  -e MIND_PALACE_VECTORS_BUCKET=... \
  -e MIND_PALACE_REGION=us-east-1 \
  -e MP_AUTH_TOKEN=your-shared-secret \
  mind-palace-mcp-remote
```

## Endpoints

- `POST /mcp` — the MCP Streamable HTTP endpoint (subject to auth).
- `GET /health` — returns `ok`, never requires auth (ALB/ECS health checks).
- `GET /.well-known/oauth-protected-resource` — OIDC discovery metadata (open);
  present so MCP clients can find the authorization server in `oidc` mode.

## Connecting a client

### OIDC mode (recommended)
Point the client at the URL with an `oauth` block; the client runs the browser
flow on the first 401. Example (Kiro agent config):

```json
{
  "mcpServers": {
    "mind-palace": {
      "url": "https://mind-palace.example.com/mcp",
      "oauth": {
        "clientId": "<cognito-app-client-id>",
        "oauthScopes": ["openid", "email", "profile", "offline_access"]
      }
    }
  }
}
```

### Token mode
```json
{
  "mcpServers": {
    "mind-palace": {
      "url": "https://mind-palace.example.com/mcp",
      "headers": { "Authorization": "Bearer your-shared-secret" }
    }
  }
}
```

### None / VPN mode
Omit both `oauth` and `headers`; just the `url`.

## Deployment notes

- **ECS/Fargate (recommended):** run the container as a long-lived service (not
  a scheduled task). Point the target group health check at `/health`. Give the
  task role the `mind-palace-runtime` IAM policy.
- **EC2:** run behind nginx/ALB; the instance profile supplies AWS creds.
- **State:** the in-memory knowledge graph is loaded once at startup. If pages
  change out-of-band (e.g. the dreaming process writes new pages), restart the
  service to reload, or rely on the fact that reads/search hit S3/Vectors live.

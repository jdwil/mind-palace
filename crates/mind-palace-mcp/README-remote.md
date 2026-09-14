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
| `MP_AUTH_TOKEN` | *(unset)* | **Optional.** If set, requests must send `Authorization: Bearer <token>`. If unset, **auth is disabled** — only do this behind a VPN/private network. |
| `MP_ALLOWED_HOSTS` | *(unset)* | Optional comma-separated `Host` allowlist. Unset = host validation disabled (needed when reached via an ALB/hostname). |

On a server, AWS credentials come from the instance/task role — no SSO. The
SSO cache timezone fix is a no-op there.

## Auth model

Auth is **optional by design**. Two supported modes:

1. **VPN / private network (no token):** leave `MP_AUTH_TOKEN` unset. The
   server logs a warning at startup. Network isolation is the security
   boundary. This is the intended default for a private deployment.
2. **Bearer token:** set `MP_AUTH_TOKEN`. Every request to the MCP path must
   carry `Authorization: Bearer <token>`. `/health` is always open (for load
   balancer checks).

## Endpoints

- `POST /mcp` — the MCP Streamable HTTP endpoint (subject to auth)
- `GET /health` — returns `ok`, never requires auth (for ALB/ECS health checks)

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

## Connecting a client

Point an MCP client at the URL. Example (Kiro agent config):

```json
{
  "mcpServers": {
    "mind-palace": {
      "url": "https://mind-palace.internal.example.com/mcp",
      "headers": { "Authorization": "Bearer your-shared-secret" }
    }
  }
}
```

If deploying behind a VPN with no token, omit the `headers` block.

## Deployment notes

- **ECS/Fargate (recommended):** run the container as a long-lived service (not
  a scheduled task). Point the target group health check at `/health`. Give the
  task role the `mind-palace-runtime` IAM policy.
- **EC2:** run behind nginx/ALB; the instance profile supplies AWS creds.
- **State:** the in-memory knowledge graph is loaded once at startup. If pages
  change out-of-band (e.g. the dreaming process writes new pages), restart the
  service to reload, or rely on the fact that reads/search hit S3/Vectors live.

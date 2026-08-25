# Mind Palace MCP Server — Setup Guide

## Quick Start

1. Download the binary for your OS from this release
2. Run the setup script (or configure manually below)
3. Restart your AI coding tool

## Download

| OS | File |
|----|------|
| Mac (Apple Silicon) | `mind-palace-mcp-aarch64-apple-darwin` |
| Mac (Intel) | `mind-palace-mcp-x86_64-apple-darwin` |
| Linux | `mind-palace-mcp-x86_64-linux` |
| Windows | `mind-palace-mcp-x86_64-windows.exe` |

## Setup (Mac/Linux)

```bash
# 1. Make it executable and move to your PATH
chmod +x mind-palace-mcp-*
sudo mv mind-palace-mcp-* /usr/local/bin/mind-palace-mcp

# 2. On macOS, allow it past Gatekeeper (first run only)
xattr -d com.apple.quarantine /usr/local/bin/mind-palace-mcp 2>/dev/null || true
```

## Setup (Windows)

Move `mind-palace-mcp-x86_64-windows.exe` to a folder in your PATH (e.g., `C:\Users\<you>\bin\`) and rename to `mind-palace-mcp.exe`.

## Configuration

The MCP server needs these environment variables. Configure them in your AI tool's MCP settings.

### Required

| Variable | Value |
|----------|-------|
| `MIND_PALACE_S3_BUCKET` | `mind-palace-pages-dev-086261225885` |
| `MIND_PALACE_DYNAMO_TABLE` | `mind-palace-graph-dev` |
| `MIND_PALACE_VECTORS_BUCKET` | `mind-palace-vectors-dev-086261225885` |
| `MIND_PALACE_VECTORS_INDEX` | `wiki-pages` |
| `MIND_PALACE_BEDROCK_MODEL` | `amazon.titan-embed-text-v2:0` |
| `MIND_PALACE_REGION` | `us-west-2` |
| `MIND_PALACE_S3_PREFIX` | `v1` |
| `AWS_PROFILE` | `dashlx_dev` |

### Recommended

| Variable | Value |
|----------|-------|
| `MIND_PALACE_USER_ID` | Your email (e.g., `jane@dashlx.com`) |
| `MIND_PALACE_USER_NAME` | Your display name (e.g., `Jane Smith`) |

### AWS Credentials

You must have the `dashlx_dev` AWS profile configured with SSO:

```bash
# One-time: configure the profile (if not already done)
aws configure sso --profile dashlx_dev
# Start URL: https://d-9267b63690.awsapps.com/start/
# Region: us-west-2
# Account: 086261225885
# Role: AWSAdministratorAccess

# Login (do this when credentials expire)
aws sso login --profile dashlx_dev
```

## Tool-Specific Configuration

### Kiro CLI (`.kiro/settings/mcp.json`)

```json
{
  "mcpServers": {
    "mind-palace": {
      "command": "/usr/local/bin/mind-palace-mcp",
      "env": {
        "MIND_PALACE_S3_BUCKET": "mind-palace-pages-dev-086261225885",
        "MIND_PALACE_S3_PREFIX": "v1",
        "MIND_PALACE_DYNAMO_TABLE": "mind-palace-graph-dev",
        "MIND_PALACE_VECTORS_BUCKET": "mind-palace-vectors-dev-086261225885",
        "MIND_PALACE_VECTORS_INDEX": "wiki-pages",
        "MIND_PALACE_BEDROCK_MODEL": "amazon.titan-embed-text-v2:0",
        "MIND_PALACE_REGION": "us-west-2",
        "MIND_PALACE_USER_ID": "YOUR_EMAIL@dashlx.com",
        "MIND_PALACE_USER_NAME": "Your Name",
        "AWS_PROFILE": "dashlx_dev"
      }
    }
  }
}
```

### Grok Build (`.grok/config.toml`)

```toml
[mcp_servers.mind-palace]
command = "/usr/local/bin/mind-palace-mcp"
enabled = true

[mcp_servers.mind-palace.env]
MIND_PALACE_S3_BUCKET = "mind-palace-pages-dev-086261225885"
MIND_PALACE_S3_PREFIX = "v1"
MIND_PALACE_DYNAMO_TABLE = "mind-palace-graph-dev"
MIND_PALACE_VECTORS_BUCKET = "mind-palace-vectors-dev-086261225885"
MIND_PALACE_VECTORS_INDEX = "wiki-pages"
MIND_PALACE_BEDROCK_MODEL = "amazon.titan-embed-text-v2:0"
MIND_PALACE_REGION = "us-west-2"
MIND_PALACE_USER_ID = "YOUR_EMAIL@dashlx.com"
MIND_PALACE_USER_NAME = "Your Name"
AWS_PROFILE = "dashlx_dev"
```

### Claude Desktop (`claude_desktop_config.json`)

```json
{
  "mcpServers": {
    "mind-palace": {
      "command": "/usr/local/bin/mind-palace-mcp",
      "env": {
        "MIND_PALACE_S3_BUCKET": "mind-palace-pages-dev-086261225885",
        "MIND_PALACE_S3_PREFIX": "v1",
        "MIND_PALACE_DYNAMO_TABLE": "mind-palace-graph-dev",
        "MIND_PALACE_VECTORS_BUCKET": "mind-palace-vectors-dev-086261225885",
        "MIND_PALACE_VECTORS_INDEX": "wiki-pages",
        "MIND_PALACE_BEDROCK_MODEL": "amazon.titan-embed-text-v2:0",
        "MIND_PALACE_REGION": "us-west-2",
        "MIND_PALACE_USER_ID": "YOUR_EMAIL@dashlx.com",
        "MIND_PALACE_USER_NAME": "Your Name",
        "AWS_PROFILE": "dashlx_dev"
      }
    }
  }
}
```

## Verify It Works

After configuring, restart your AI tool and ask it to run `wiki_search` with any query. If it returns results, you're good.

## Troubleshooting

- **"dispatch failure" or empty results**: Run `aws sso login --profile dashlx_dev` to refresh credentials
- **"page not found"**: Check that `MIND_PALACE_REGION` is `us-west-2`
- **macOS "cannot be opened"**: Run `xattr -d com.apple.quarantine /usr/local/bin/mind-palace-mcp`
- **Windows Defender blocks it**: Allow the executable in Windows Security settings

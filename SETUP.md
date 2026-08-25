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
| `MIND_PALACE_S3_BUCKET` | Your pages S3 bucket |
| `MIND_PALACE_DYNAMO_TABLE` | Your DynamoDB graph table |
| `MIND_PALACE_VECTORS_BUCKET` | Your S3 Vectors bucket |
| `MIND_PALACE_VECTORS_INDEX` | `wiki-pages` |
| `MIND_PALACE_BEDROCK_MODEL` | `amazon.titan-embed-text-v2:0` |
| `MIND_PALACE_REGION` | Your AWS region (e.g., `us-west-2`) |
| `MIND_PALACE_S3_PREFIX` | `v1` |
| `AWS_PROFILE` | Your AWS CLI profile name |

### Recommended

| Variable | Value |
|----------|-------|
| `MIND_PALACE_USER_ID` | Your email (e.g., `jane@example.com`) |
| `MIND_PALACE_USER_NAME` | Your display name (e.g., `Jane Smith`) |

### AWS Credentials

You must have an AWS profile configured with access to the Mind Palace resources:

```bash
# One-time: configure your profile with SSO (get details from your team lead)
aws configure sso --profile YOUR_PROFILE

# Login (do this when credentials expire)
aws sso login --profile YOUR_PROFILE
```

## Tool-Specific Configuration

### Kiro CLI (`.kiro/settings/mcp.json`)

```json
{
  "mcpServers": {
    "mind-palace": {
      "command": "/usr/local/bin/mind-palace-mcp",
      "env": {
        "MIND_PALACE_S3_BUCKET": "YOUR_PAGES_BUCKET",
        "MIND_PALACE_S3_PREFIX": "v1",
        "MIND_PALACE_DYNAMO_TABLE": "YOUR_DYNAMO_TABLE",
        "MIND_PALACE_VECTORS_BUCKET": "YOUR_VECTORS_BUCKET",
        "MIND_PALACE_VECTORS_INDEX": "wiki-pages",
        "MIND_PALACE_BEDROCK_MODEL": "amazon.titan-embed-text-v2:0",
        "MIND_PALACE_REGION": "us-west-2",
        "MIND_PALACE_USER_ID": "you@example.com",
        "MIND_PALACE_USER_NAME": "Your Name",
        "AWS_PROFILE": "your-profile"
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
MIND_PALACE_S3_BUCKET = "YOUR_PAGES_BUCKET"
MIND_PALACE_S3_PREFIX = "v1"
MIND_PALACE_DYNAMO_TABLE = "YOUR_DYNAMO_TABLE"
MIND_PALACE_VECTORS_BUCKET = "YOUR_VECTORS_BUCKET"
MIND_PALACE_VECTORS_INDEX = "wiki-pages"
MIND_PALACE_BEDROCK_MODEL = "amazon.titan-embed-text-v2:0"
MIND_PALACE_REGION = "us-west-2"
MIND_PALACE_USER_ID = "you@example.com"
MIND_PALACE_USER_NAME = "Your Name"
AWS_PROFILE = "your-profile"
```

### Claude Desktop (`claude_desktop_config.json`)

```json
{
  "mcpServers": {
    "mind-palace": {
      "command": "/usr/local/bin/mind-palace-mcp",
      "env": {
        "MIND_PALACE_S3_BUCKET": "YOUR_PAGES_BUCKET",
        "MIND_PALACE_S3_PREFIX": "v1",
        "MIND_PALACE_DYNAMO_TABLE": "YOUR_DYNAMO_TABLE",
        "MIND_PALACE_VECTORS_BUCKET": "YOUR_VECTORS_BUCKET",
        "MIND_PALACE_VECTORS_INDEX": "wiki-pages",
        "MIND_PALACE_BEDROCK_MODEL": "amazon.titan-embed-text-v2:0",
        "MIND_PALACE_REGION": "us-west-2",
        "MIND_PALACE_USER_ID": "you@example.com",
        "MIND_PALACE_USER_NAME": "Your Name",
        "AWS_PROFILE": "your-profile"
      }
    }
  }
}
```

## Verify It Works

After configuring, restart your AI tool and ask it to run `wiki_search` with any query. If it returns results, you're good.

## Troubleshooting

- **"dispatch failure" or empty results**: Run `aws sso login --profile YOUR_PROFILE` to refresh credentials
- **"page not found"**: Check that `MIND_PALACE_REGION` is `us-west-2`
- **macOS "cannot be opened"**: Run `xattr -d com.apple.quarantine /usr/local/bin/mind-palace-mcp`
- **Windows Defender blocks it**: Allow the executable in Windows Security settings

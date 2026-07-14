# Mind Palace MCP Server — Setup Guide

## Prerequisites

- Rust toolchain (install via [rustup](https://rustup.rs))
- AWS CLI configured with the `dashlx_dev` profile (`aws configure --profile dashlx_dev`)
- Access to the DashLX dev AWS account (086261225885)

## 1. Clone and Build

```bash
git clone git@github.com:jdwil/mind-palace.git
cd mind-palace
cargo build --release -p mind-palace-mcp
```

This takes ~1-2 minutes on first build (downloads + compiles AWS SDK deps).

## 2. Install the Binary

```bash
cp target/release/mind-palace-mcp ~/bin/
# Or wherever your PATH-accessible bin directory is:
# sudo cp target/release/mind-palace-mcp /usr/local/bin/
```

Verify it's accessible:

```bash
which mind-palace-mcp
```

## 3. Configure Your MCP Client

Add the following to your MCP configuration file:

**For Kiro CLI** (`~/.kiro/settings/mcp.json`):

```json
{
  "mcpServers": {
    "mind-palace": {
      "command": "mind-palace-mcp",
      "env": {
        "MIND_PALACE_S3_BUCKET": "mind-palace-pages-dev-086261225885",
        "MIND_PALACE_S3_PREFIX": "v1",
        "MIND_PALACE_DYNAMO_TABLE": "mind-palace-graph-dev",
        "MIND_PALACE_VECTORS_BUCKET": "mind-palace-vectors-dev-086261225885",
        "MIND_PALACE_VECTORS_INDEX": "wiki-pages",
        "MIND_PALACE_BEDROCK_MODEL": "amazon.titan-embed-text-v2:0",
        "MIND_PALACE_REGION": "us-west-2",
        "AWS_PROFILE": "dashlx_dev"
      }
    }
  }
}
```

**For Claude Desktop** (`~/Library/Application Support/Claude/claude_desktop_config.json` on macOS, or `~/.config/claude/claude_desktop_config.json` on Linux):

```json
{
  "mcpServers": {
    "mind-palace": {
      "command": "mind-palace-mcp",
      "env": {
        "MIND_PALACE_S3_BUCKET": "mind-palace-pages-dev-086261225885",
        "MIND_PALACE_S3_PREFIX": "v1",
        "MIND_PALACE_DYNAMO_TABLE": "mind-palace-graph-dev",
        "MIND_PALACE_VECTORS_BUCKET": "mind-palace-vectors-dev-086261225885",
        "MIND_PALACE_VECTORS_INDEX": "wiki-pages",
        "MIND_PALACE_BEDROCK_MODEL": "amazon.titan-embed-text-v2:0",
        "MIND_PALACE_REGION": "us-west-2",
        "AWS_PROFILE": "dashlx_dev"
      }
    }
  }
}
```

## 4. Verify It Works

Restart your MCP client (Kiro, Claude Desktop, etc.) and you should see 6 tools available:

| Tool | Purpose |
|------|---------|
| `wiki_search` | Semantic search — find pages by meaning |
| `wiki_read` | Read a page (summary, section, or full) |
| `wiki_traverse` | Walk the knowledge graph from a page |
| `wiki_create` | Create a new wiki page |
| `wiki_update` | Update an existing page |
| `wiki_list` | List pages by type |

Try asking your agent: *"Search the wiki for any existing pages"* or *"Create a wiki page about our deployment process"*

## 5. Updating

When the repo is updated:

```bash
cd mind-palace
git pull
cargo build --release -p mind-palace-mcp
cp target/release/mind-palace-mcp ~/bin/
```

## Environment Variables Reference

| Variable | Required | Default | Description |
|----------|----------|---------|-------------|
| `MIND_PALACE_REGION` | No | `us-east-1` | AWS region |
| `MIND_PALACE_S3_BUCKET` | No | `mind-palace-pages` | S3 bucket for page content |
| `MIND_PALACE_S3_PREFIX` | No | `v1` | Key prefix in S3 |
| `MIND_PALACE_DYNAMO_TABLE` | No | `mind-palace-graph` | DynamoDB table name |
| `MIND_PALACE_VECTORS_BUCKET` | No | `mind-palace-vectors` | S3 Vectors bucket |
| `MIND_PALACE_VECTORS_INDEX` | No | `wiki` | Vector index name |
| `MIND_PALACE_BEDROCK_MODEL` | No | `amazon.titan-embed-text-v2:0` | Embedding model |
| `AWS_PROFILE` | No | default | AWS credentials profile |

## Troubleshooting

**"credential not found" errors:** Make sure `aws configure --profile dashlx_dev` is set up and you can run `aws s3 ls --profile dashlx_dev` successfully.

**Server doesn't start:** Run it directly in terminal to see errors:
```bash
MIND_PALACE_REGION=us-west-2 \
MIND_PALACE_S3_BUCKET=mind-palace-pages-dev-086261225885 \
MIND_PALACE_DYNAMO_TABLE=mind-palace-graph-dev \
MIND_PALACE_VECTORS_BUCKET=mind-palace-vectors-dev-086261225885 \
MIND_PALACE_VECTORS_INDEX=wiki-pages \
AWS_PROFILE=dashlx_dev \
mind-palace-mcp
```

It should hang (waiting for MCP messages on stdin). If it exits immediately with an error, that's what needs fixing.

**Tools don't appear in client:** Ensure you've restarted the MCP client after adding the config. Most clients only read config on startup.

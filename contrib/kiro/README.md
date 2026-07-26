# Kiro Adapter for Mind Palace

Ships Kiro CLI session logs to S3 so the Mind Palace dreaming process can analyze them.

## Setup

### 1. Configure environment variables

Add to your shell profile or `.env`:

```bash
export MP_LOG_BUCKET="mind-palace-logs-YOUR_ACCOUNT_ID"
export MP_AGENT_ID="kiro-yourname"
export MP_LOG_PREFIX="sessions"        # optional, default: sessions
export MP_MIN_TURNS="3"                 # optional, min user turns to ship
export MP_LOG_REGION="us-west-2"        # optional, defaults to AWS_DEFAULT_REGION
```

### 2. Add the hook to your Kiro agent config

In your agent JSON (e.g., `~/.kiro/agents/my-agent.json`):

```json
{
  "hooks": {
    "stop": [
      {
        "command": "/path/to/mind-palace/contrib/kiro/mp-ship-logs.sh",
        "timeout_ms": 5000
      }
    ]
  }
}
```

### 3. Ensure AWS credentials

The script uses `aws s3 cp`, so your AWS credentials must be configured.
Set `AWS_PROFILE` if you need a specific profile.

## How it works

- Fires on every Kiro assistant turn (the `stop` hook)
- Checks if the session has enough turns to be worth shipping
- Debounces: won't re-upload the same file size twice
- Uploads run in the background so the hook exits immediately
- Session logs land at: `s3://{bucket}/{prefix}/{agent-id}/{date}/{session-id}.jsonl`

## S3 bucket structure

```
s3://mind-palace-logs-{account}/
  sessions/
    kiro-jd/2026-07-26/abc123.jsonl
    kiro-dev2/2026-07-26/def456.jsonl
    grok-jd/2026-07-25/ghi789.jsonl      # other adapters use same format
    dc-agent-foo/2026-07-26/jkl012.jsonl  # autonomous agents too
```

The dreaming process reads and deletes these logs after analysis.

## Requirements

- AWS CLI v2
- Kiro CLI with hooks support
- S3 bucket (see `infra/template.yaml` for CloudFormation)

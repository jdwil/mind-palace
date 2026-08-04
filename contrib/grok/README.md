# Grok Build Adapter for Mind Palace

Ships Grok Build session logs to S3 so the Mind Palace dreaming process can analyze them.

## Setup

### 1. Configure environment variables

Add to your shell profile:

```bash
export MP_LOG_BUCKET="mind-palace-logs-YOUR_ACCOUNT_ID"
export MP_AGENT_ID="grok-yourname"
export MP_LOG_REGION="us-west-2"        # optional, defaults to AWS_DEFAULT_REGION
export MP_MIN_TURNS="3"                 # optional, min user turns to ship
```

### 2. Create the Grok hook config

Create `~/.grok/hooks/mp-ship-logs.json`:

```json
{
  "hooks": {
    "SessionEnd": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "/path/to/mind-palace/contrib/grok/mp-ship-logs.sh",
            "timeout": 5
          }
        ]
      }
    ]
  }
}
```

### 3. Ensure AWS credentials

The script uses `aws s3 cp`, so your AWS credentials must be configured.
Set `AWS_PROFILE` if you need a specific profile.

## How it works

- Fires when a Grok Build session ends (`SessionEnd` event)
- Locates the session's `chat_history.jsonl` via `GROK_SESSION_ID`
- Checks if the session has enough turns to be worth shipping
- Debounces: won't re-upload the same file size twice
- Uploads run in the background so the hook exits immediately
- Session logs land at: `s3://{bucket}/{prefix}/{agent-id}/{date}/{session-id}.jsonl`

## S3 bucket structure

```
s3://mind-palace-logs-{account}/
  sessions/
    grok-jd/2026-08-03/019f3cd4-32be-7ba2-bb88-7d503bafcbdb.jsonl
    kiro-jd/2026-08-03/abc123.jsonl       # Kiro adapter uses same bucket
    dc-agent-foo/2026-08-03/jkl012.jsonl  # autonomous agents too
```

The dreaming process reads and deletes these logs after analysis.

## Requirements

- AWS CLI v2
- Grok Build with hooks support
- S3 bucket (deployed via the mind-palace Terraform module)

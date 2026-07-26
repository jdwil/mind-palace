#!/usr/bin/env bash
# Mind Palace — Kiro session log shipper
#
# Kiro 'stop' hook that ships session logs to S3 for the dreaming process.
# Install: add to your Kiro agent config's hooks.stop array.
#
# Required environment variables:
#   MP_LOG_BUCKET  — S3 bucket for session logs
#   MP_AGENT_ID    — Identifier for this agent instance (e.g., "kiro-jd")
#
# Optional environment variables:
#   MP_LOG_PREFIX  — S3 key prefix (default: "sessions")
#   MP_MIN_TURNS   — Minimum user turns before shipping (default: 3)
#   AWS_PROFILE    — AWS profile to use for S3 upload
#   MP_LOG_REGION  — AWS region for the log bucket (default: uses AWS_DEFAULT_REGION)
#
# The hook reads the stop event from stdin, checks if the session has enough
# substance to be worth shipping, then uploads the session log to S3.
# The upload runs in the background so the hook exits immediately.

set -euo pipefail

# Read and discard the hook event from stdin (required by Kiro hook protocol)
cat > /dev/null

# Validate required env vars
if [ -z "${MP_LOG_BUCKET:-}" ] || [ -z "${MP_AGENT_ID:-}" ]; then
  echo "MP_LOG_BUCKET and MP_AGENT_ID must be set" >&2
  exit 1
fi

# Configuration
BUCKET="$MP_LOG_BUCKET"
AGENT_ID="$MP_AGENT_ID"
PREFIX="${MP_LOG_PREFIX:-sessions}"
MIN_TURNS="${MP_MIN_TURNS:-3}"
REGION_FLAG=""
if [ -n "${MP_LOG_REGION:-}" ]; then
  REGION_FLAG="--region $MP_LOG_REGION"
fi

# Locate the session log
SESSION_ID="${KIRO_SESSION_ID:-}"
if [ -z "$SESSION_ID" ]; then
  exit 0
fi

# Kiro stores session logs here
SESSION_FILE="${HOME}/.kiro/sessions/cli/${SESSION_ID}.jsonl"
if [ ! -f "$SESSION_FILE" ]; then
  exit 0
fi

# Check substance: count user prompts
TURN_COUNT=$(grep -c '"kind":"Prompt"' "$SESSION_FILE" 2>/dev/null || echo "0")
if [ "$TURN_COUNT" -lt "$MIN_TURNS" ]; then
  exit 0
fi

# Debounce: skip if already shipped this version
MARKER_FILE="/tmp/mp-shipped-${SESSION_ID}"
CURRENT_SIZE=$(stat -c%s "$SESSION_FILE" 2>/dev/null || stat -f%z "$SESSION_FILE" 2>/dev/null || echo "0")
if [ -f "$MARKER_FILE" ] && [ "$(cat "$MARKER_FILE")" = "$CURRENT_SIZE" ]; then
  exit 0
fi

# Ship to S3 in background
DATE=$(date -u +%Y-%m-%d)
S3_KEY="${PREFIX}/${AGENT_ID}/${DATE}/${SESSION_ID}.jsonl"

nohup bash -c "
  aws s3 cp '$SESSION_FILE' 's3://${BUCKET}/${S3_KEY}' $REGION_FLAG --quiet 2>/dev/null && \
  echo '$CURRENT_SIZE' > '$MARKER_FILE'
" > /dev/null 2>&1 &

exit 0

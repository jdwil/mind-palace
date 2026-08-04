#!/usr/bin/env bash
# Mind Palace — Grok Build session log shipper
#
# Grok Build 'SessionEnd' hook that ships session logs to S3 for the dreaming process.
# Install: place in ~/.grok/hooks/mp-ship-logs.json (see below).
#
# Required environment variables:
#   MP_LOG_BUCKET  — S3 bucket for session logs
#   MP_AGENT_ID    — Identifier for this agent instance (e.g., "grok-jd")
#
# Optional environment variables:
#   MP_LOG_PREFIX  — S3 key prefix (default: "sessions")
#   MP_MIN_TURNS   — Minimum user turns before shipping (default: 3)
#   AWS_PROFILE    — AWS profile to use for S3 upload
#   MP_LOG_REGION  — AWS region for the log bucket (default: uses AWS_DEFAULT_REGION)
#
# The hook reads the SessionEnd event from stdin, locates the session's
# chat_history.jsonl, and uploads it to S3 in the background.

set -euo pipefail

# Read the hook event from stdin
EVENT=$(cat)

# Validate required env vars
if [ -z "${MP_LOG_BUCKET:-}" ] || [ -z "${MP_AGENT_ID:-}" ]; then
  exit 0  # Silent exit — don't break Grok if unconfigured
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

# Extract session ID from environment (Grok provides this)
SESSION_ID="${GROK_SESSION_ID:-}"
if [ -z "$SESSION_ID" ]; then
  exit 0
fi

# Grok stores sessions in ~/.grok/sessions/{url-encoded-workspace}/{session-id}/
# Find the session directory by searching for the session ID
SESSION_DIR=$(find ~/.grok/sessions -maxdepth 2 -type d -name "$SESSION_ID" 2>/dev/null | head -1)
if [ -z "$SESSION_DIR" ]; then
  exit 0
fi

SESSION_FILE="${SESSION_DIR}/chat_history.jsonl"
if [ ! -f "$SESSION_FILE" ]; then
  exit 0
fi

# Check substance: count user messages
TURN_COUNT=$(grep -c '"role":"user"\|"role": "user"' "$SESSION_FILE" 2>/dev/null || echo "0")
if [ "$TURN_COUNT" -lt "$MIN_TURNS" ]; then
  exit 0
fi

# Debounce: skip if already shipped this version
MARKER_FILE="/tmp/mp-shipped-grok-${SESSION_ID}"
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

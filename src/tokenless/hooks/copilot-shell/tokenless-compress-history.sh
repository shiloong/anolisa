#!/usr/bin/env bash
# tokenless-hook-version: 7
# Token-Less copilot-shell hook — deep history compression.
# Stats are recorded automatically by tokenless compress-history.
# Requires: jq

set -euo pipefail

# --- Dependency checks (fail-open) ---
if ! command -v jq &>/dev/null; then
  echo "[tokenless] WARNING: jq is not installed. History compression hook disabled." >&2
  exit 0
fi

if ! command -v tokenless &>/dev/null; then
  echo "[tokenless] WARNING: tokenless is not installed or not in PATH. History compression hook disabled." >&2
  exit 0
fi

# --- Read input (fail-open) ---
INPUT=$(cat || {
  echo "[tokenless] WARNING: failed to read PreCompact payload. Passing through unchanged." >&2
  exit 0
})

# --- Extract messages from llm_request ---
MESSAGES=$(echo "$INPUT" | jq -c '.llm_request.messages // empty' 2>/dev/null || echo '')

if [ -z "$MESSAGES" ] || [ "$MESSAGES" = "null" ] || [ "$MESSAGES" = "[]" ]; then
  exit 0
fi

# --- Extract caller context ---
SESSION_ID=$(echo "$INPUT" | jq -r '.session_id // empty' 2>/dev/null || echo '')

# --- Compress history ---
SNAPSHOT=$(echo "$MESSAGES" | tokenless compress-history \
  --agent-id copilot-shell \
  ${SESSION_ID:+--session-id "$SESSION_ID"} \
  2>/dev/null) || {
  echo "[tokenless] WARNING: History compression failed. Passing through unchanged." >&2
  exit 0
}

# Validate snapshot is non-empty
if [ -z "$SNAPSHOT" ]; then
  echo "[tokenless] WARNING: History compression returned empty output. Passing through unchanged." >&2
  exit 0
fi

# --- Build copilot-shell response ---
jq -n \
  --arg snapshot "$SNAPSHOT" \
  '{
    "hookSpecificOutput": {
      "hookEventName": "PreCompact",
      "additionalContext": $snapshot
    }
  }' || {
  echo "[tokenless] WARNING: failed to build hook response JSON. Passing through unchanged." >&2
  exit 0
}
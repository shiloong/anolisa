#!/usr/bin/env bash
# tokenless-hook-version: 7
# Token-Less copilot-shell hook — trims redundant history messages.
# Stats are recorded automatically by tokenless trim-history.
# Requires: jq

set -euo pipefail

# --- Dependency checks (fail-open) ---
if ! command -v jq &>/dev/null; then
  echo "[tokenless] WARNING: jq is not installed. History trim hook disabled." >&2
  exit 0
fi

if ! command -v tokenless &>/dev/null; then
  echo "[tokenless] WARNING: tokenless is not installed or not in PATH. History trim hook disabled." >&2
  exit 0
fi

# --- Read input (fail-open) ---
INPUT=$(cat || {
  echo "[tokenless] WARNING: failed to read BeforeModel payload. Passing through unchanged." >&2
  exit 0
})

# --- Extract messages array ---
MESSAGES=$(echo "$INPUT" | jq -c '.llm_request.messages // empty' 2>/dev/null || echo '')

if [ -z "$MESSAGES" ] || [ "$MESSAGES" = "null" ] || [ "$MESSAGES" = "[]" ]; then
  exit 0
fi

# Skip very short conversations (< 4 messages)
MSG_LENGTH=$(echo "$MESSAGES" | jq 'length' 2>/dev/null || echo '0')
if [ "$MSG_LENGTH" -lt 4 ]; then
  exit 0
fi

# --- Extract caller context ---
SESSION_ID=$(echo "$INPUT" | jq -r '.session_id // empty' 2>/dev/null || echo '')

# --- Trim history ---
TRIMMED=$(echo "$MESSAGES" | tokenless trim-history \
  --agent-id copilot-shell \
  ${SESSION_ID:+--session-id "$SESSION_ID"} \
  2>/dev/null) || {
  echo "[tokenless] WARNING: History trim failed. Passing through unchanged." >&2
  exit 0
}

# Validate trimmed output is valid JSON array
if ! echo "$TRIMMED" | jq -e 'type == "array"' &>/dev/null 2>&1; then
  echo "[tokenless] WARNING: History trim returned invalid JSON. Passing through unchanged." >&2
  exit 0
fi

# --- Build copilot-shell response ---
jq -n \
  --argjson messages "$TRIMMED" \
  '{
    "hookSpecificOutput": {
      "hookEventName": "BeforeModel",
      "llm_request": {
        "messages": $messages
      }
    }
  }' || {
  echo "[tokenless] WARNING: failed to build hook response JSON. Passing through unchanged." >&2
  exit 0
}
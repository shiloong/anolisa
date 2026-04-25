#!/usr/bin/env bash
# tokenless-hook-version: 6
# Token-Less copilot-shell hook — compresses tool call responses.
# Stats are recorded automatically by tokenless compress-response.
# Requires: jq

set -euo pipefail

# --- Dependency checks (fail-open) ---

if ! command -v jq &>/dev/null; then
  echo "[tokenless] WARNING: jq is not installed. Response compression hook disabled." >&2
  exit 0
fi

if ! command -v tokenless &>/dev/null; then
  echo "[tokenless] WARNING: tokenless is not installed or not in PATH. Response compression hook disabled." >&2
  exit 0
fi

# --- Read input (fail-open) ---

INPUT=$(cat || {
  echo "[tokenless] WARNING: failed to read PostToolUse payload. Passing through unchanged." >&2
  exit 0
})

# --- Extract tool_response ---

TOOL_RESPONSE=$(echo "$INPUT" | jq -c '.tool_response // empty' 2>/dev/null || echo '')

if [ -z "$TOOL_RESPONSE" ] || [ "$TOOL_RESPONSE" = "null" ] || [ "$TOOL_RESPONSE" = "{}" ]; then
  exit 0
fi

# --- Skip small responses ---

RESPONSE_LEN=${#TOOL_RESPONSE}
if [ "$RESPONSE_LEN" -lt 200 ]; then
  exit 0
fi

# --- Extract caller context from raw payload ---

SESSION_ID=$(echo "$INPUT" | jq -r '.session_id // empty' 2>/dev/null || echo '')
TOOL_USE_ID=$(echo "$INPUT" | jq -r '.tool_use_id // empty' 2>/dev/null || echo '')
TOOL_NAME=$(echo "$INPUT" | jq -r '.tool_name // "unknown"' 2>/dev/null || echo 'unknown')

# --- Compress response (with tool-aware strategy) ---

COMPRESSED=$(echo "$TOOL_RESPONSE" | tokenless compress-response \
  --agent-id copilot-shell \
  ${SESSION_ID:+--session-id "$SESSION_ID"} \
  ${TOOL_USE_ID:+--tool-use-id "$TOOL_USE_ID"} \
  ${TOOL_NAME:+--tool-name "$TOOL_NAME"} \
  2>/dev/null) || {
  echo "[tokenless] WARNING: Response compression failed. Passing through unchanged." >&2
  exit 0
}

# Validate compressed output is non-empty
if [ -z "$COMPRESSED" ]; then
  echo "[tokenless] WARNING: Response compression returned empty output. Passing through unchanged." >&2
  exit 0
fi

# --- Build copilot-shell response ---

jq -n \
  --arg context "$COMPRESSED" \
  --arg tool "$TOOL_NAME" \
  '{
    "suppressOutput": true,
    "hookSpecificOutput": {
      "hookEventName": "PostToolUse",
      "additionalContext": ("[tokenless] compressed response from " + $tool + ":\n" + $context)
    }
  }' || {
  echo "[tokenless] WARNING: failed to build hook response JSON. Passing through unchanged." >&2
  exit 0
}

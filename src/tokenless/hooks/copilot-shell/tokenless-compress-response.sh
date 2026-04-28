#!/usr/bin/env bash
# tokenless-hook-version: 8
# Token-Less copilot-shell hook — compresses tool call responses,
# performs environment attribution analysis on failures,
# then optionally re-encodes to TOON format for additional token savings.
#
# Pipeline: Failure Attribution → Response Compression → TOON Encoding
#   1. If tool_response contains errors, classify as environment vs logic issue
#      and inject "Skip retry" guidance for LLM
#   2. Strip debug fields, nulls, empty values; truncate long strings/arrays
#   3. If the compressed result is still valid JSON, encode to TOON format
#   4. Combine attribution + compressed content in additionalContext
#
# Stats are recorded automatically by tokenless compress-response.
# Requires: tokenless, jq, toon (optional — TOON step disabled if missing)
#
# Hook event: PostToolUse
#
# Design: fail-open — if compression fails or dependencies are missing,
# the original response passes through unchanged.

set -euo pipefail

# --- Dependency checks (fail-open: never block tool responses) ---

if ! command -v jq &>/dev/null; then
  echo "[tokenless] WARNING: jq is not installed. Response compression hook disabled." >&2
  exit 0
fi

if ! command -v tokenless &>/dev/null; then
  echo "[tokenless] WARNING: tokenless is not installed. Response compression hook disabled." >&2
  exit 0
fi

TOON_AVAILABLE=false
if command -v toon &>/dev/null; then
  TOON_AVAILABLE=true
fi

# --- Read input (fail-open) ---

INPUT=$(cat || {
  echo "[tokenless] WARNING: failed to read PostToolUse payload. Passing through unchanged." >&2
  exit 0
})

# --- Skip content-retrieval tools ---
# Tools that return content the agent explicitly requested must not be compressed
# because truncation would make the content incomplete and unusable.
SKIP_TOOLS="Read read_file Glob list_directory NotebookRead read glob notebookread"
TOOL_NAME=$(echo "$INPUT" | jq -r '.tool_name // "unknown"' 2>/dev/null || echo 'unknown')

if [ "$TOOL_NAME" != "unknown" ] && echo "$SKIP_TOOLS" | grep -qw "$TOOL_NAME"; then
  exit 0
fi

# --- Extract tool_response (fail-open) ---

TOOL_RESPONSE_RAW=$(echo "$INPUT" | jq -c '.tool_response // empty' 2>/dev/null || echo '')

if [ -z "$TOOL_RESPONSE_RAW" ] || [ "$TOOL_RESPONSE_RAW" = "null" ] || [ "$TOOL_RESPONSE_RAW" = "{}" ]; then
  exit 0
fi

# --- Skip small responses ---

RESPONSE_LEN=${#TOOL_RESPONSE_RAW}
if [ "$RESPONSE_LEN" -lt 200 ]; then
  exit 0
fi

# --- Skip skill files (YAML frontmatter markdown) ---
# Skill files (.md with YAML frontmatter) must not be compressed because
# truncation would break the skill metadata and make agent skills unusable.
# Detection failure is intentionally non-fatal (fail-open): if detection
# fails, we continue to compression rather than blocking the response.
SKILL_CHECK_RAW=$(echo "$INPUT" | jq -r '.tool_response // empty' 2>/dev/null || echo '')
if [ -n "$SKILL_CHECK_RAW" ]; then
  case "$SKILL_CHECK_RAW" in
    ---*)
      if echo "$SKILL_CHECK_RAW" | head -n 20 | grep -qE '^(name|description):'; then
        exit 0
      fi
      ;;
  esac
fi

# --- Unwrap string-wrapped JSON ---
# tool_response may be a JSON string (escaped shell output) or a raw JSON object.
# If it's a string whose inner content is JSON, parse it into an object for compression.
FIRST_CHAR=$(echo "$TOOL_RESPONSE_RAW" | head -c 1)
if [ "$FIRST_CHAR" = '"' ]; then
  # It's a JSON string — extract inner content
  INNER=$(echo "$TOOL_RESPONSE_RAW" | jq -r '.' 2>/dev/null || echo '')
  # Check if the inner content is valid JSON
  if echo "$INNER" | jq -e '.' &>/dev/null 2>&1; then
    TOOL_RESPONSE=$(echo "$INNER" | jq -c '.' 2>/dev/null || echo '')
  else
    # Not JSON inner content — keep the original JSON string for TOON
    TOOL_RESPONSE="$TOOL_RESPONSE_RAW"
  fi
else
  TOOL_RESPONSE="$TOOL_RESPONSE_RAW"
fi

if [ -z "$TOOL_RESPONSE" ] || [ "$TOOL_RESPONSE" = "null" ] || [ "$TOOL_RESPONSE" = "{}" ]; then
  exit 0
fi

# Recalculate length after unwrapping
RESPONSE_LEN=${#TOOL_RESPONSE}
if [ "$RESPONSE_LEN" -lt 200 ]; then
  exit 0
fi

# --- Environment attribution analysis ---
# Classify tool execution failures as environment issues vs logic errors.
# Injects "Skip retry" guidance when the failure is environment-related.

ENV_ATTRIBUTION=""
ATTR_CATEGORY=""
ATTR_FIX_HINT=""

# Use unwrapped TOOL_RESPONSE for error parsing
ATTR_INPUT="$TOOL_RESPONSE"

# Unwrap if still a JSON string
FIRST_ATTR_CHAR=$(echo "$ATTR_INPUT" | head -c 1)
if [ "$FIRST_ATTR_CHAR" = '"' ]; then
  ATTR_INPUT=$(echo "$ATTR_INPUT" | jq -r '.' 2>/dev/null || echo "$ATTR_INPUT")
fi

# Check for error indicators in the parsed response
EXIT_CODE=$(echo "$ATTR_INPUT" | jq -r '.exit_code // empty' 2>/dev/null || echo '')
STDERR_TEXT=$(echo "$ATTR_INPUT" | jq -r '.stderr // empty' 2>/dev/null || echo '')
ERROR_FIELD=$(echo "$ATTR_INPUT" | jq -r '.error // empty' 2>/dev/null || echo '')

# Combine all error text for pattern matching
ERROR_TEXT="${STDERR_TEXT}${ERROR_FIELD}"

if [ -n "$ERROR_TEXT" ] || [ "$EXIT_CODE" = "1" ] || [ "$EXIT_CODE" = "2" ]; then
  if [ -n "$ERROR_TEXT" ]; then
    case "$ERROR_TEXT" in
      *"command not found"*|*"not installed"*|*"which: no"*|*"No command '*'"*)
        ATTR_CATEGORY="ENV_DEPENDENCY_MISSING"
        MISSING_CMD=$(echo "$ERROR_TEXT" | grep -oP '(command not found: |which: no )\S+' | head -1 | sed 's/command not found: //;s/which: no //' || echo "unknown")
        ATTR_FIX_HINT="Install missing dependency: ${MISSING_CMD}"
        ;;
      *"Permission denied"*|*"permission denied"*|*"Access denied"*)
        ATTR_CATEGORY="ENV_PERMISSION"
        ATTR_FIX_HINT="Check file/dir permissions or run with appropriate access"
        ;;
      *"No such file or directory"*|*"cannot find"*|*"does not exist"*|*"ENOENT"*)
        ATTR_CATEGORY="ENV_FILE_MISSING"
        ATTR_FIX_HINT="Create or locate the required file/directory"
        ;;
      *"Connection refused"*|*"ECONNREFUSED"*|*"Connection timed out"*|*"ETIMEDOUT"*|*"curl: (7)"*|*"curl: (6)"*|*"network is unreachable"*)
        ATTR_CATEGORY="ENV_NETWORK"
        ATTR_FIX_HINT="Check network connectivity and DNS resolution"
        ;;
      *"ModuleNotFoundError"*|*"cannot find module"*|*"ImportError"*|*"npm ERR! 404"*)
        ATTR_CATEGORY="ENV_PACKAGE_MISSING"
        ATTR_FIX_HINT="Install the required module/package"
        ;;
    esac
  fi

  if [ -n "$ATTR_CATEGORY" ]; then
    ENV_ATTRIBUTION="[tokenless env-attribution] ${TOOL_NAME} tool failed: ${ATTR_CATEGORY} (${ATTR_FIX_HINT}). Skip retry — this is an environment issue, not a logic error."
  fi
fi

# --- Step 1: Response Compression ---
# tokenless compress-response expects a JSON object/array. If TOOL_RESPONSE
# is a JSON string (plain text), compression will fail — we skip to TOON.

COMPRESSED=""
USED_RESP_COMPRESSION=false
if echo "$TOOL_RESPONSE" | jq -e 'type == "object" or type == "array"' &>/dev/null 2>&1; then
  COMPRESSED=$(echo "$TOOL_RESPONSE" | tokenless compress-response 2>/dev/null) || true
  if [ -n "$COMPRESSED" ]; then
    USED_RESP_COMPRESSION=true
  fi
fi

# If response compression was skipped or failed, use original for TOON
if [ -z "$COMPRESSED" ]; then
  COMPRESSED="$TOOL_RESPONSE"
fi

# Calculate savings after response compression
AFTER_RESP_CHARS=${#COMPRESSED}

# --- Step 2: TOON Encoding (if compressed result is valid JSON) ---

TOON_OUTPUT=""
if [ "$TOON_AVAILABLE" = true ]; then
  IS_JSON=$(echo "$COMPRESSED" | jq -e '.' &>/dev/null && echo "yes" || echo "no")
  if [ "$IS_JSON" = "yes" ]; then
    TOON_OUTPUT=$(echo "$COMPRESSED" | toon -e 2>/dev/null) || true
    if [ -n "$TOON_OUTPUT" ]; then
      AFTER_TOON_CHARS=${#TOON_OUTPUT}
      if [ "$USED_RESP_COMPRESSION" = true ]; then
        SAVINGS_LABEL="response compressed + TOON encoded"
      else
        SAVINGS_LABEL="TOON encoded"
      fi
    fi
  fi
fi

# Set label if TOON didn't produce output
if [ -z "${SAVINGS_LABEL:-}" ]; then
  if [ "$USED_RESP_COMPRESSION" = true ]; then
    SAVINGS_LABEL="response compressed"
  else
    SAVINGS_LABEL="passed through"
  fi
fi

# Determine final output and metrics
if [ -n "$TOON_OUTPUT" ]; then
  FINAL_OUTPUT="$TOON_OUTPUT"
  AFTER_CHARS=$AFTER_TOON_CHARS
else
  FINAL_OUTPUT="$COMPRESSED"
  AFTER_CHARS=$AFTER_RESP_CHARS
fi

# --- Calculate combined savings ---

BEFORE_CHARS=$RESPONSE_LEN
BEFORE_TOKENS=$(( (BEFORE_CHARS + 3) / 4 ))
AFTER_TOKENS=$(( (AFTER_CHARS + 3) / 4 ))

SAVINGS_PCT=0
if [ "$BEFORE_CHARS" -gt 0 ]; then
  SAVINGS_PCT=$(( (BEFORE_CHARS - AFTER_CHARS) * 100 / BEFORE_CHARS ))
fi

# --- Record statistics ---

SESSION_ID=$(echo "$INPUT" | jq -r '.session_id // empty' 2>/dev/null || echo '')
TOOL_USE_ID=$(echo "$INPUT" | jq -r '.tool_use_id // empty' 2>/dev/null || echo '')
AGENT_ID="copilot-shell"
AGENT_PID=$$

if command -v tokenless &>/dev/null; then
  RECORD_CMD=(
    tokenless stats record
    --operation compress-response
    --agent-id "$AGENT_ID"
    --before-chars "$BEFORE_CHARS"
    --before-tokens "$BEFORE_TOKENS"
    --after-chars "$AFTER_CHARS"
    --after-tokens "$AFTER_TOKENS"
    --pid "$AGENT_PID"
    --before-text "$TOOL_RESPONSE"
    --after-text "$FINAL_OUTPUT"
  )

  [ -n "$SESSION_ID" ] && RECORD_CMD+=(--session-id "$SESSION_ID")
  [ -n "$TOOL_USE_ID" ] && RECORD_CMD+=(--tool-use-id "$TOOL_USE_ID")

  "${RECORD_CMD[@]}" 2>/dev/null || true
fi

# --- Build final additionalContext ---
# Combine attribution (if present) with compression/TOON content

FINAL_CONTEXT=""
if [ -n "$ENV_ATTRIBUTION" ]; then
  FINAL_CONTEXT="${ENV_ATTRIBUTION}\n\n"
fi
FINAL_CONTEXT="${FINAL_CONTEXT}[tokenless] Tool response from ${TOOL_NAME} ${SAVINGS_LABEL} (${SAVINGS_PCT}% token savings)."
if [ "$SAVINGS_LABEL" = "response compressed + TOON encoded" ]; then
  FINAL_CONTEXT="${FINAL_CONTEXT}\nTOON is a compact notation for structured data. Parse it as key-value pairs and tabular data.\n"
fi
FINAL_CONTEXT="${FINAL_CONTEXT}\n${FINAL_OUTPUT}"

jq -n \
  --arg context "$FINAL_CONTEXT" \
  --arg tool "$TOOL_NAME" \
  --arg savings "$SAVINGS_PCT" \
  --arg label "$SAVINGS_LABEL" \
  '{
    "suppressOutput": true,
    "hookSpecificOutput": {
      "hookEventName": "PostToolUse",
      "additionalContext": $context
    }
  }' || {
  echo "[tokenless] WARNING: failed to build hook response JSON. Passing through unchanged." >&2
  exit 0
}
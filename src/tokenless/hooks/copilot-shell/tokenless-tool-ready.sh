#!/usr/bin/env bash
# tokenless-hook-version: 8
# Token-Less copilot-shell hook — Tool Ready environment scanner.
# Scans tool dependencies (object-format or string-format) before execution,
# attempts auto-fix for missing items via config-driven install engine,
# and injects diagnostic info into additionalContext so LLM can avoid retries.
#
# Hook event: PreToolUse (matcher: "" — matches all tools)
# Requires: jq
#
# Design: fail-open. If jq is missing or scanning fails, exit 0 silently.

set -euo pipefail

# --- Dependency checks (fail-open) ---
if ! command -v jq &>/dev/null; then exit 0; fi

# --- Resolve paths (search shared core location first, then local) ---
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

SPEC_FILE=""
for candidate in \
    "${TOKENLESS_TOOL_READY_SPEC:-}" \
    "/usr/share/tokenless/core/env-check/tool-ready-spec.json" \
    "${SCRIPT_DIR}/tool-ready-spec.json" \
    "$HOME/.tokenless/tool-ready-spec.json"; do
    if [ -n "$candidate" ] && [ -f "$candidate" ]; then
        SPEC_FILE="$candidate"
        break
    fi
done

FIX_SCRIPT=""
for candidate in \
    "${TOKENLESS_ENV_FIX_SCRIPT:-}" \
    "/usr/share/tokenless/core/env-check/tokenless-env-fix.sh" \
    "${SCRIPT_DIR}/tokenless-env-fix.sh"; do
    if [ -n "$candidate" ] && [ -x "$candidate" ]; then
        FIX_SCRIPT="$candidate"
        break
    fi
done

# --- Read input (fail-open) ---
INPUT=$(cat || { exit 0; })

# --- Extract tool_name ---
TOOL_NAME=$(echo "$INPUT" | jq -r '.tool_name // empty' 2>/dev/null || echo '')
if [ -z "$TOOL_NAME" ]; then exit 0; fi

# --- Load dependency spec for this tool ---
if [ ! -f "$SPEC_FILE" ]; then exit 0; fi
TOOL_SPEC=$(jq -c ".\"$TOOL_NAME\"" "$SPEC_FILE" 2>/dev/null || echo '')
if [ -z "$TOOL_SPEC" ] || [ "$TOOL_SPEC" = "null" ]; then exit 0; fi

# --- Normalize deps to object format ---
# Supports both string ("jq") and object ({binary:"jq",...}) formats.
# String defaults: manager="apt", package=binary name.
# Handles version constraints: "rtk>=0.35" → {binary:"rtk", version:">=0.35", ...}

normalize_deps() {
  local array="$1"
  echo "$array" | jq -c '[.[] | if type == "string" then
    (if (test(">=") or test("[^<]<[^=]") or test("=")) then
      {binary: (capture("^(?<b>[^>=<]+)") | .b), version: (match("[>=<]+[0-9.]+").string), package: (capture("^(?<b>[^>=<]+)") | .b), manager: "apt"}
    else
      {binary: ., package: ., manager: "apt"}
    end)
  else . end]' 2>/dev/null || echo '[]'
}

REQUIRED=$(normalize_deps "$(echo "$TOOL_SPEC" | jq -c '.required // []')")
RECOMMENDED=$(normalize_deps "$(echo "$TOOL_SPEC" | jq -c '.recommended // []')")
CONFIG_FILES=$(echo "$TOOL_SPEC" | jq -r '.config_files[] // empty' 2>/dev/null || echo '')
PERMISSIONS=$(echo "$TOOL_SPEC" | jq -r '.permissions[] // empty' 2>/dev/null || echo '')
NETWORK=$(echo "$TOOL_SPEC" | jq -r '.network[] // empty' 2>/dev/null || echo '')

# --- Version comparison helper ---
version_ge() {
  local installed="$1" required="$2"
  local i_major i_minor i_patch r_major r_minor r_patch
  IFS='.' read -r i_major i_minor i_patch <<< "$installed"
  IFS='.' read -r r_major r_minor r_patch <<< "$required"
  i_major=${i_major:-0}; i_minor=${i_minor:-0}; i_patch=${i_patch:-0}
  r_major=${r_major:-0}; r_minor=${r_minor:-0}; r_patch=${r_patch:-0}
  [ "$i_major" -gt "$r_major" ] && return 0
  [ "$i_major" -lt "$r_major" ] && return 1
  [ "$i_minor" -gt "$r_minor" ] && return 0
  [ "$i_minor" -lt "$r_minor" ] && return 1
  [ "$i_patch" -gt "$r_patch" ] && return 0
  [ "$i_patch" -lt "$r_patch" ] && return 1
  return 0
}

# --- Check a single dep (normalized object) ---
# Input: JSON object {binary, version, package, manager, ...}
# Output: "available", "missing", "version_low:<installed>:<required>"

check_dep() {
  local dep_json="$1"
  local binary version
  binary=$(echo "$dep_json" | jq -r '.binary')
  version=$(echo "$dep_json" | jq -r '.version // empty')

  if ! command -v "$binary" &>/dev/null; then
    echo "missing"
    return
  fi

  if [ -z "$version" ]; then
    echo "available"
    return
  fi

  # Parse version constraint
  local constraint_ver
  constraint_ver=$(echo "$version" | sed 's/[>=<]//g')
  local installed_version
  installed_version=$("$binary" --version 2>/dev/null | grep -oE '[0-9]+\.[0-9]+\.[0-9]+' | head -1 || echo "0.0.0")
  [ -z "$installed_version" ] && installed_version="0.0.0"

  if version_ge "$installed_version" "$constraint_ver"; then
    echo "available"
  else
    echo "version_low:${installed_version}:${constraint_ver}"
  fi
}

# --- Scan all dependencies ---
MISSING_REQUIRED=""
MISSING_RECOMMENDED=""
VERSION_LOW=""
CONFIG_MISSING=""
PERM_MISSING=""
NET_MISSING=""
AVAILABLE_LIST=""
MISSING_DEP_JSONS="[]"

# Check required deps
req_count=$(echo "$REQUIRED" | jq 'length')
for i in $(seq 0 $((req_count - 1))); do
  dep_json=$(echo "$REQUIRED" | jq -c ".[$i]")
  binary=$(echo "$dep_json" | jq -r '.binary')
  status=$(check_dep "$dep_json")
  case "$status" in
    available)   AVAILABLE_LIST="${AVAILABLE_LIST} ${binary}✓" ;;
    missing)
      MISSING_REQUIRED="${MISSING_REQUIRED} ${binary}"
      MISSING_DEP_JSONS=$(echo "$MISSING_DEP_JSONS" | jq -c ". + [$dep_json]")
      ;;
    version_low:*)
      low_info="${status#version_low:}"
      VERSION_LOW="${VERSION_LOW} ${binary}(${low_info})"
      ;;
  esac
done

# Check recommended deps
rec_count=$(echo "$RECOMMENDED" | jq 'length')
for i in $(seq 0 $((rec_count - 1))); do
  dep_json=$(echo "$RECOMMENDED" | jq -c ".[$i]")
  binary=$(echo "$dep_json" | jq -r '.binary')
  status=$(check_dep "$dep_json")
  case "$status" in
    available)   AVAILABLE_LIST="${AVAILABLE_LIST} ${binary}✓" ;;
    missing)
      MISSING_RECOMMENDED="${MISSING_RECOMMENDED} ${binary}"
      MISSING_DEP_JSONS=$(echo "$MISSING_DEP_JSONS" | jq -c ". + [$dep_json]")
      ;;
    version_low:*)
      low_info="${status#version_low:}"
      VERSION_LOW="${VERSION_LOW} ${binary}(${low_info})"
      ;;
  esac
done

# Check config files
for cfg in $CONFIG_FILES; do
  expanded=$(echo "$cfg" | sed "s|^~|${HOME}|")
  [ ! -f "$expanded" ] && CONFIG_MISSING="${CONFIG_MISSING} ${cfg}"
done

# Check permissions
for perm in $PERMISSIONS; do
  case "$perm" in
    file_read)   [ ! -r / ] && PERM_MISSING="${PERM_MISSING} file_read" ;;
    file_write)  ! touch "${HOME}/.tokenless-ready-test" 2>/dev/null && rm -f "${HOME}/.tokenless-ready-test" 2>/dev/null && PERM_MISSING="${PERM_MISSING} file_write" ;;
    exec_shell)  ! command -v bash &>/dev/null && PERM_MISSING="${PERM_MISSING} exec_shell" ;;
  esac
done

# Check network
for net in $NETWORK; do
  case "$net" in
    https_outbound) ! curl -s --max-time 2 https://example.com >/dev/null 2>&1 && NET_MISSING="${NET_MISSING} https_outbound" ;;
  esac
done

# --- Determine pre-fix status ---
PRE_FIX_STATUS="READY"
[ -n "$MISSING_REQUIRED" ] || [ -n "$VERSION_LOW" ] || [ -n "$PERM_MISSING" ] && PRE_FIX_STATUS="NOT_READY"
[ "$PRE_FIX_STATUS" = "READY" ] && { [ -n "$MISSING_RECOMMENDED" ] || [ -n "$CONFIG_MISSING" ] || [ -n "$NET_MISSING" ]; } && PRE_FIX_STATUS="PARTIAL"

# --- Attempt auto-fix for missing items ---
POST_FIX_STATUS="$PRE_FIX_STATUS"
FIX_ATTEMPTED=""
FIX_RESULTS=""

missing_count=$(echo "$MISSING_DEP_JSONS" | jq 'length' 2>/dev/null || echo 0)

if [ "$missing_count" -gt 0 ] && [ -x "$FIX_SCRIPT" ]; then
  FIX_OUTPUT=$(bash "$FIX_SCRIPT" fix-all "$(echo "$MISSING_DEP_JSONS" | jq -c '.')" 2>/dev/null || true)
  FIX_ATTEMPTED=$(echo "$MISSING_DEP_JSONS" | jq -r '.[].binary' | tr '\n' ' ')
  FIX_RESULTS="$FIX_OUTPUT"

  # Re-scan to determine post-fix status
  POST_FIX_STATUS="READY"
  STILL_MISSING=""
  for i in $(seq 0 $((missing_count - 1))); do
    binary=$(echo "$MISSING_DEP_JSONS" | jq -r ".[$i].binary")
    if ! command -v "$binary" &>/dev/null; then
      STILL_MISSING="${STILL_MISSING} ${binary}"
      POST_FIX_STATUS="NOT_READY"
    fi
  done
  if [ -z "$STILL_MISSING" ] && [ -z "$VERSION_LOW" ] && [ -z "$PERM_MISSING" ]; then
    POST_FIX_STATUS="READY"
  elif [ -n "$STILL_MISSING" ]; then
    POST_FIX_STATUS="NOT_READY"
  else
    POST_FIX_STATUS="PARTIAL"
  fi
fi

# --- If everything is ready, exit silently ---
[ "$POST_FIX_STATUS" = "READY" ] && exit 0

# --- Build diagnostic message ---
DIAG_PARTS=""
[ -n "$MISSING_REQUIRED" ]  && DIAG_PARTS="${DIAG_PARTS} required missing: ${MISSING_REQUIRED};"
[ -n "$MISSING_RECOMMENDED" ] && DIAG_PARTS="${DIAG_PARTS} recommended missing: ${MISSING_RECOMMENDED};"
[ -n "$VERSION_LOW" ]       && DIAG_PARTS="${DIAG_PARTS} version too low: ${VERSION_LOW};"
[ -n "$CONFIG_MISSING" ]    && DIAG_PARTS="${DIAG_PARTS} config missing: ${CONFIG_MISSING};"
[ -n "$PERM_MISSING" ]      && DIAG_PARTS="${DIAG_PARTS} permission missing: ${PERM_MISSING};"
[ -n "$NET_MISSING" ]       && DIAG_PARTS="${DIAG_PARTS} network missing: ${NET_MISSING};"
[ -n "$AVAILABLE_LIST" ]    && DIAG_PARTS="${DIAG_PARTS} available: ${AVAILABLE_LIST};"

DIAG_MSG="[tokenless env-check] ${TOOL_NAME} tool: ${POST_FIX_STATUS}"
[ -n "$DIAG_PARTS" ] && DIAG_MSG="${DIAG_MSG} (${DIAG_PARTS})"

if [ -n "$FIX_ATTEMPTED" ]; then
  FIX_SUMMARY=$(echo "$FIX_RESULTS" | grep -oE '\[tokenless-env-fix\] [^:]+' | tr '\n' ',' | sed 's/,$//;s/\[tokenless-env-fix\] //g' || true)
  if [ -n "$FIX_SUMMARY" ]; then
    DIAG_MSG="${DIAG_MSG}. Auto-fix attempted: ${FIX_SUMMARY}."
  fi
fi

[ "$POST_FIX_STATUS" = "NOT_READY" ] && DIAG_MSG="${DIAG_MSG}. Skip retry — environment issue, not logic error."

# --- Build hook response JSON ---
jq -n --arg context "$DIAG_MSG" '{
  "hookSpecificOutput": {
    "hookEventName": "PreToolUse",
    "additionalContext": $context
  }
}' || exit 0
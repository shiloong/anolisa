#!/usr/bin/bash
# tokenless-hook-version: 10
# Tool Ready environment pre-check — thin wrapper over tokenless env-check.
#
# Hook event: PreToolUse (matcher: "" — matches all tools)
# Design: fail-open. If tokenless binary is missing, exit 0 silently.

set -euo pipefail

VERBOSE="${TOKENLESS_VERBOSE:-}"
log_v() { [ -n "$VERBOSE" ] && echo "[tokenless tool-ready] $1" >&2 || true; }

# Fail-open: tokenless binary must exist
if ! command -v tokenless &>/dev/null; then
    log_v "tokenless binary not found, skipping"
    exit 0
fi

# Read stdin and extract tool_name via python3
TOOL_NAME=$(python3 -c "
import sys, json
try:
    data = json.load(sys.stdin)
except Exception:
    sys.exit(0)
print(data.get('tool_name', ''))
" 2>/dev/null || echo '')

if [ -z "$TOOL_NAME" ]; then exit 0; fi
log_v "tool_name=$TOOL_NAME"

# --- Phase 1: Check tool readiness ---
RESULT=$(tokenless env-check --tool "$TOOL_NAME" --json 2>/dev/null || echo '{}')
STATUS=$(python3 -c "import sys,json;d=json.loads(sys.argv[1]);print(d.get('status','UNKNOWN'))" "$RESULT" 2>/dev/null || echo 'UNKNOWN')
log_v "status=$STATUS"

case "$STATUS" in READY|UNKNOWN) exit 0 ;; esac

# --- Phase 2: Auto-fix and re-check ---
FIX_RESULT=$(tokenless env-check --tool "$TOOL_NAME" --fix --json 2>/dev/null || echo '{}')
POST_STATUS=$(python3 -c "import sys,json;d=json.loads(sys.argv[1]);print(d.get('status','NOT_READY'))" "$FIX_RESULT" 2>/dev/null || echo 'NOT_READY')
log_v "post-fix status=$POST_STATUS"

[ "$POST_STATUS" = "READY" ] && exit 0

# --- Phase 3: Build response via single python3 invocation ---
# Pass POST_STATUS, TOOL_NAME and FIX_RESULT as args to avoid shell escaping issues
python3 -c "
import sys, json

post_status = sys.argv[1]
tool_name = sys.argv[2]
fix_result = json.loads(sys.argv[3])
diagnostic = fix_result.get('diagnostic',
    f'[tokenless tool-ready] {tool_name}: {post_status} -- environment issue')

if post_status == 'NOT_READY':
    print(json.dumps({
        'decision': 'block',
        'reason': diagnostic,
        'hookSpecificOutput': {
            'hookEventName': 'PreToolUse',
            'additionalContext': diagnostic
        }
    }))
else:
    print(json.dumps({
        'systemMessage': diagnostic,
        'hookSpecificOutput': {
            'hookEventName': 'PreToolUse',
            'additionalContext': diagnostic
        }
    }))
" "$POST_STATUS" "$TOOL_NAME" "$FIX_RESULT"
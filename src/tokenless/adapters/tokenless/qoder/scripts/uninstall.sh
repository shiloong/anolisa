#!/usr/bin/env bash
# uninstall-qoder.sh — Remove tokenless plugin from Qoder CLI.
set -euo pipefail

AGENT="${ANOLISA_TARGET:-qoder}"
COMPONENT="${ANOLISA_COMPONENT:-tokenless}"
SETTINGS_PATH="$HOME/.qoder/settings.json"

# Find qodercli binary
QODERCLI=""
for candidate in "$HOME/.qoder/bin/qodercli/qodercli-${ANOLISA_QODER_VERSION:-*}" \
                 "$HOME/.qoder/bin/qodercli/qodercli" \
                 "qodercli"; do
    for resolved in $candidate; do
        if [ -x "$resolved" ] || command -v "$resolved" &>/dev/null; then
            QODERCLI="$resolved"
            break 2
        fi
    done
done

echo "[${COMPONENT}] Removing ${AGENT} plugin..."

# Unregister plugin via qodercli standard command
if [ -n "$QODERCLI" ]; then
    "$QODERCLI" plugins uninstall tokenless
else
    echo "[${COMPONENT}] WARNING: qodercli not found, cannot unregister plugin"
fi

# Remove hooks from settings.json
if [ -f "$SETTINGS_PATH" ] && command -v python3 &>/dev/null; then
    python3 -c "
import json
cfg = json.load(open('${SETTINGS_PATH}'))
hooks = cfg.get('hooks', {})
removed = False
for event in list(hooks.keys()):
    keep = []
    for entry in hooks[event]:
        cmd = ''
        for h in (entry.get('hooks') or []):
            if h.get('command'):
                cmd = h['command']
                break
        if 'tokenless' not in cmd:
            keep.append(entry)
        else:
            removed = True
    if keep:
        hooks[event] = keep
    else:
        del hooks[event]
        removed = True
if removed:
    if hooks:
        cfg['hooks'] = hooks
    else:
        cfg.pop('hooks', None)
    json.dump(cfg, open('${SETTINGS_PATH}', 'w'), indent=2)
    print(f'[${COMPONENT}] Removed tokenless hooks from settings.json')
" 2>/dev/null || true
fi

echo "[${COMPONENT}] ${AGENT} plugin removed."
#!/usr/bin/env bash
# install-qoder.sh — Install tokenless plugin for Qoder CLI.
set -euo pipefail

AGENT="${ANOLISA_TARGET:-qoder}"
COMPONENT="${ANOLISA_COMPONENT:-tokenless}"
VERSION="${ANOLISA_VERSION:-0.3.2}"
ADAPTER_DIR="${ANOLISA_ADAPTER_DIR:-$(cd "$(dirname "$0")/../.." && pwd)}"

PLUGIN_DIR="$ADAPTER_DIR/qoder"
SETTINGS_PATH="$HOME/.qoder/settings.json"

# Find qodercli binary
QODERCLI=""
for candidate in "$HOME/.qoder/bin/qodercli/qodercli-${ANOLISA_QODER_VERSION:-*}" \
                 "$HOME/.qoder/bin/qodercli/qodercli" \
                 "qodercli"; do
    # Resolve glob expansion
    for resolved in $candidate; do
        if [ -x "$resolved" ] || command -v "$resolved" &>/dev/null; then
            QODERCLI="$resolved"
            break 2
        fi
    done
done

if [ -z "$QODERCLI" ]; then
    echo "[${COMPONENT}] WARNING: qodercli not found, skipping plugin registration"
    echo "    Install Qoder CLI first: https://qoder.com/cli"
    exit 1
fi

echo "[${COMPONENT}] Installing ${AGENT} plugin v${VERSION}..."

# Register plugin via qodercli standard command.
# qodercli derives plugin name from the directory name, so we use a
# symlink named "tokenless" to match plugin.json's name field.
echo "[${COMPONENT}] Registering plugin with qodercli..."
TEMP_SYMLINK="/tmp/tokenless"
rm -rf "$TEMP_SYMLINK"
ln -sfn "$PLUGIN_DIR" "$TEMP_SYMLINK"
"$QODERCLI" plugins install "$TEMP_SYMLINK"
rm -f "$TEMP_SYMLINK"

# Merge hooks into ~/.qoder/settings.json
# Hooks reference the plugin dir directly (RPM-managed, not ~/.local/)
HOOKS_DIR="$PLUGIN_DIR/hooks"
HOOKS_CONFIG=$(cat "$PLUGIN_DIR/hooks.json")

if command -v python3 &>/dev/null; then
    python3 -c "
import json, os

hooks_dir = '${HOOKS_DIR}'
hooks_template = json.loads('''${HOOKS_CONFIG}''')
hooks_str = json.dumps(hooks_template)
hooks_str = hooks_str.replace('\${QODER_TOKENLESS_HOOKS}', hooks_dir)
resolved = json.loads(hooks_str)

settings_path = '${SETTINGS_PATH}'
cfg = {}
if os.path.exists(settings_path):
    try:
        cfg = json.load(open(settings_path))
    except Exception:
        pass
if not isinstance(cfg, dict):
    cfg = {}

existing_hooks = cfg.get('hooks', {})
if not isinstance(existing_hooks, dict):
    existing_hooks = {}

for event, entries in resolved['hooks'].items():
    existing = existing_hooks.get(event, [])
    existing_cmds = set()
    for e in existing:
        for h in (e.get('hooks') or []):
            if h.get('command'):
                existing_cmds.add(h['command'])
    for entry in entries:
        entry_cmd = None
        for h in (entry.get('hooks') or []):
            if h.get('command'):
                entry_cmd = h['command']
                break
        if entry_cmd and entry_cmd not in existing_cmds:
            existing.append(entry)
    existing_hooks[event] = existing

cfg['hooks'] = existing_hooks
os.makedirs(os.path.dirname(settings_path), exist_ok=True)
json.dump(cfg, open(settings_path, 'w'), indent=2)
print(f'[${COMPONENT}] Updated {settings_path}')
"
fi

echo "[${COMPONENT}] ${AGENT} plugin v${VERSION} installed and activated."
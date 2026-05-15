#!/usr/bin/env bash
# install.sh — Install tokenless plugin into Hermes Agent via symlink + enable.
set -euo pipefail

AGENT="${ANOLISA_TARGET:-hermes}"
COMPONENT="${ANOLISA_COMPONENT:-tokenless}"
ADAPTER_DIR="${ANOLISA_ADAPTER_DIR:-$(cd "$(dirname "$0")/../.." && pwd)}"

PLUGIN_SRC="$ADAPTER_DIR/hermes"
PLUGIN_DST="$HOME/.hermes/plugins/tokenless"

echo "[${COMPONENT}] Installing ${AGENT} plugin..."

if [ ! -d "$PLUGIN_SRC" ]; then
    echo "[${COMPONENT}] Plugin source not found: $PLUGIN_SRC"
    exit 1
fi

if [ ! -f "$PLUGIN_SRC/plugin.yaml" ] || [ ! -f "$PLUGIN_SRC/__init__.py" ]; then
    echo "[${COMPONENT}] Missing plugin.yaml or __init__.py in $PLUGIN_SRC"
    exit 1
fi

mkdir -p "$PLUGIN_DST"

# Use symlinks so plugin stays synced with system install
ln -sfn "$PLUGIN_SRC/__init__.py" "$PLUGIN_DST/__init__.py"
ln -sfn "$PLUGIN_SRC/plugin.yaml" "$PLUGIN_DST/plugin.yaml"

echo "[${COMPONENT}] ${AGENT} plugin linked to $PLUGIN_DST (from $PLUGIN_SRC)."

# Enable via hermes CLI if available (adds to plugins.enabled in config.yaml)
if command -v hermes &>/dev/null; then
    echo "[${COMPONENT}] Enabling ${AGENT} plugin..."
    hermes plugins enable tokenless || {
        echo "[${COMPONENT}] Warning: hermes plugins enable failed — enable manually via config.yaml."
    }
else
    echo "[${COMPONENT}] hermes CLI not found — add 'tokenless' to plugins.enabled in ~/.hermes/config.yaml."
fi

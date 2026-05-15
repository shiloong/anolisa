#!/usr/bin/env bash
# uninstall.sh — Disable and remove tokenless plugin from Hermes Agent.
set -euo pipefail

AGENT="${ANOLISA_TARGET:-hermes}"
COMPONENT="${ANOLISA_COMPONENT:-tokenless}"

PLUGIN_DST="$HOME/.hermes/plugins/tokenless"

echo "[${COMPONENT}] Uninstalling ${AGENT} plugin..."

# Disable via hermes CLI if available (removes from plugins.enabled in config.yaml)
if command -v hermes &>/dev/null; then
    hermes plugins disable tokenless || true
    hermes plugins remove tokenless || true
else
    # Manually remove symlinks/directory when hermes CLI is unavailable
    if [ -d "$PLUGIN_DST" ]; then
        rm -f "$PLUGIN_DST/__init__.py" "$PLUGIN_DST/plugin.yaml" 2>/dev/null || true
        rmdir "$PLUGIN_DST" 2>/dev/null || true
    fi
fi

echo "[${COMPONENT}] ${AGENT} plugin uninstalled."

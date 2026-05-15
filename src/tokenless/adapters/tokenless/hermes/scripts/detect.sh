#!/usr/bin/env bash
# detect.sh — Check if Hermes Agent is installed and compatible.
# Exit 0 = ready to install, non-0 = not available.
set -euo pipefail

AGENT="${ANOLISA_TARGET:-hermes}"
COMPONENT="${ANOLISA_COMPONENT:-tokenless}"

if [ -d "$HOME/.hermes/plugins" ]; then
    echo "[${COMPONENT}] ${AGENT}: detected ~/.hermes/plugins directory"
    exit 0
fi

if command -v hermes &>/dev/null; then
    echo "[${COMPONENT}] ${AGENT}: detected hermes binary"
    exit 0
fi

echo "[${COMPONENT}] ${AGENT}: not detected (neither ~/.hermes/plugins nor hermes binary found)" >&2
exit 1
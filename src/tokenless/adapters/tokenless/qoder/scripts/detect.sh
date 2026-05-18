#!/usr/bin/env bash
# detect-qoder.sh — Check if Qoder CLI is installed and compatible.
# Exit 0 = ready to install, non-0 = not available.
set -euo pipefail

AGENT="${ANOLISA_TARGET:-qoder}"
COMPONENT="${ANOLISA_COMPONENT:-tokenless}"

if [ -d "$HOME/.qoder" ]; then
    echo "[${COMPONENT}] ${AGENT}: detected ~/.qoder config directory"
    exit 0
fi

if command -v qoder &>/dev/null; then
    echo "[${COMPONENT}] ${AGENT}: detected qoder binary"
    exit 0
fi

echo "[${COMPONENT}] ${AGENT}: not detected (neither ~/.qoder nor qoder binary found)" >&2
exit 1
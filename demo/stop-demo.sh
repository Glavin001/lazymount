#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

echo "Stopping lazymount client daemon..."
lazymount daemon stop 2>/dev/null || true

echo "Stopping Docker container..."
docker compose -f "$SCRIPT_DIR/docker-compose.yml" down

echo "Done."

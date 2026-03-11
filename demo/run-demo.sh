#!/usr/bin/env bash
# =============================================================================
# demo/run-demo.sh — Interactive demo: Mac (share) + Docker container (mount)
#
# This sets up the full lazymount pipeline and leaves it running so you can
# explore the mounted filesystem interactively.  Hit Ctrl+C to stop.
#
# See demo/e2e-test.sh for the automated, non-interactive version.
#
# Architecture:
#   Docker container  = "remote server"  → lazymount server start (chisel + rclone mount)
#   Host (Mac/Linux)  = "local machine"  → lazymount daemon + connect + share
#
# Usage:
#   ./demo/run-demo.sh                            # shares ~/Desktop by default
#   SHARE_PATH=~/code/my-project ./demo/run-demo.sh
#   LAZYMOUNT_BIN=/path/to/lazymount ./demo/run-demo.sh
# =============================================================================

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

# ── Locate lazymount binary ───────────────────────────────────────────────────
if [[ -z "${LAZYMOUNT_BIN:-}" ]]; then
  if [[ -x "$REPO_ROOT/target/release/lazymount" ]]; then
    LAZYMOUNT_BIN="$REPO_ROOT/target/release/lazymount"
  elif command -v lazymount &>/dev/null; then
    LAZYMOUNT_BIN="$(command -v lazymount)"
  else
    echo "ERROR: lazymount binary not found."
    echo "  Build it with: cargo build --release -p lazymount-cli"
    echo "  Or set LAZYMOUNT_BIN=/path/to/lazymount"
    exit 1
  fi
fi
echo "Using lazymount: $LAZYMOUNT_BIN"

# ── Check prerequisites ───────────────────────────────────────────────────────
for bin in chisel rclone docker; do
  if ! command -v "$bin" &>/dev/null; then
    echo "ERROR: '$bin' not found on PATH."
    exit 1
  fi
done

REMOTE_NAME="docker-demo"
REMOTE_AUTH="demo:secret"
SHARE_NAME="myfiles"
SHARE_PATH="${SHARE_PATH:-$HOME/Desktop}"
COMPOSE_FILE="$SCRIPT_DIR/docker-compose.yml"

# ── Cleanup on exit ───────────────────────────────────────────────────────────
cleanup() {
  echo ""
  echo "Stopping daemon and container..."
  "$LAZYMOUNT_BIN" daemon stop 2>/dev/null || true
  docker compose -f "$COMPOSE_FILE" down --remove-orphans 2>/dev/null || true
}
trap cleanup EXIT

# ── Step 1: Build & start the Docker server container ────────────────────────
echo ""
echo "=== Step 1: Starting Docker container (remote server) ==="
docker compose -f "$COMPOSE_FILE" up -d --build
echo "Waiting for chisel server to bind..."
sleep 3

# ── Step 2: Start the host daemon ────────────────────────────────────────────
echo ""
echo "=== Step 2: Starting lazymount client daemon ==="
"$LAZYMOUNT_BIN" daemon stop 2>/dev/null || true
sleep 1
nohup "$LAZYMOUNT_BIN" daemon start > /tmp/lazymount-daemon.log 2>&1 &
sleep 2

# ── Step 3–5: Register, connect, share ───────────────────────────────────────
echo ""
echo "=== Step 3: Registering remote '$REMOTE_NAME' (localhost:8090) ==="
"$LAZYMOUNT_BIN" remote add "$REMOTE_NAME" "localhost:8090" --auth "$REMOTE_AUTH" || true

echo ""
echo "=== Step 4: Connecting ==="
"$LAZYMOUNT_BIN" connect "$REMOTE_NAME"
sleep 2

echo ""
echo "=== Step 5: Sharing '$SHARE_PATH' as '$SHARE_NAME' ==="
"$LAZYMOUNT_BIN" share add "$SHARE_NAME" "$SHARE_PATH" --remote "$REMOTE_NAME"

echo ""
echo "=== Done! Waiting for mount to appear in container (~5–10s)... ==="
echo ""
echo "Helpful commands:"
echo "  # See files appearing in the container:"
echo "  docker compose -f $COMPOSE_FILE exec lazymount-server ls /root/LazyMount/$SHARE_NAME/"
echo ""
echo "  # Check mount status:"
echo "  docker compose -f $COMPOSE_FILE exec lazymount-server lazymount mounts"
echo ""
echo "  # Host daemon status:"
echo "  $LAZYMOUNT_BIN status"
echo ""
echo "  # Host daemon logs:"
echo "  tail -f /tmp/lazymount-daemon.log"
echo ""
echo "Press Ctrl+C to stop everything."

# Keep running until Ctrl+C
wait

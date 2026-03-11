#!/usr/bin/env bash
# =============================================================================
# demo/e2e-test.sh — End-to-end tests for LazyMount
#
# Tests the full round-trip with a real Docker container as the "remote server":
#   - Host (Mac or Linux CI runner) = local machine, runs lazymount daemon + share
#   - Docker container (Linux)      = remote server, runs lazymount server start
#
# Usage:
#   ./demo/e2e-test.sh                        # uses binary from target/release/ or PATH
#   LAZYMOUNT_BIN=/path/to/lazymount ./demo/e2e-test.sh
#
# Requirements on host:  lazymount, chisel, rclone, docker
# =============================================================================

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
COMPOSE_FILE="$SCRIPT_DIR/docker-compose.yml"

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

echo "Using lazymount: $LAZYMOUNT_BIN  ($(uname -sm))"

# ── Check host prerequisites ──────────────────────────────────────────────────
for bin in chisel rclone docker; do
  if ! command -v "$bin" &>/dev/null; then
    echo "ERROR: '$bin' not found on PATH."
    exit 1
  fi
done

# ── Config ────────────────────────────────────────────────────────────────────
REMOTE_NAME="e2e-demo"
REMOTE_AUTH="demo:secret"          # must match docker-compose.yml command args
SHARE_NAME="testshare"
MOUNT_TIMEOUT=60                   # seconds to wait for mount to appear
NEW_FILE_TIMEOUT=45                # seconds to wait for a new file to propagate
                                   # (rclone --dir-cache-time 30s + buffer)
PASS=0
FAIL=0

# ── Cleanup trap ─────────────────────────────────────────────────────────────
TEST_DIR=""
cleanup() {
  echo ""
  echo "── Cleanup ──────────────────────────────────────────────────────────"
  "$LAZYMOUNT_BIN" daemon stop 2>/dev/null || true
  docker compose -f "$COMPOSE_FILE" down --volumes --remove-orphans 2>/dev/null || true
  if [[ -n "$TEST_DIR" && -d "$TEST_DIR" ]]; then
    rm -rf "$TEST_DIR"
  fi
}
trap cleanup EXIT

# ── Test helpers ──────────────────────────────────────────────────────────────
pass() { echo "[PASS] $1"; (( PASS++ )) || true; }
fail() { echo "[FAIL] $1"; (( FAIL++ )) || true; }

# Run a command inside the container (non-interactive)
container_exec() {
  docker compose -f "$COMPOSE_FILE" exec -T lazymount-server "$@"
}

# Poll until a command succeeds or timeout (in seconds)
wait_for() {
  local desc="$1" timeout="$2"; shift 2
  local elapsed=0
  while ! "$@" &>/dev/null; do
    if (( elapsed >= timeout )); then
      return 1
    fi
    sleep 2
    (( elapsed += 2 ))
  done
  return 0
}

# ── Step 1: Create test data ──────────────────────────────────────────────────
echo ""
echo "── Setup: creating test data ────────────────────────────────────────"
TEST_DIR="$(mktemp -d)"
echo "Test dir: $TEST_DIR"

echo "hello lazymount" > "$TEST_DIR/hello.txt"
printf "line1\nline2\nline3\n" > "$TEST_DIR/multiline.txt"
mkdir -p "$TEST_DIR/subdir"
echo "nested file content" > "$TEST_DIR/subdir/nested.txt"

# ── Step 2: Start Docker server container ────────────────────────────────────
echo ""
echo "── Step 2: Building and starting Docker server container ────────────"
docker compose -f "$COMPOSE_FILE" up -d --build
echo "Container started. Waiting for chisel server to bind port 8090..."
sleep 3

# ── Step 3: Start host daemon ─────────────────────────────────────────────────
echo ""
echo "── Step 3: Starting lazymount daemon on host ────────────────────────"
"$LAZYMOUNT_BIN" daemon stop 2>/dev/null || true
sleep 1
nohup "$LAZYMOUNT_BIN" daemon start > /tmp/lazymount-e2e-daemon.log 2>&1 &
sleep 2

# ── Step 4: Register remote, connect, share ───────────────────────────────────
echo ""
echo "── Step 4: Registering remote '$REMOTE_NAME' and sharing test dir ──"
"$LAZYMOUNT_BIN" remote add "$REMOTE_NAME" "localhost:8090" --auth "$REMOTE_AUTH" || true
"$LAZYMOUNT_BIN" connect "$REMOTE_NAME"
"$LAZYMOUNT_BIN" share add "$SHARE_NAME" "$TEST_DIR" --remote "$REMOTE_NAME"
echo "Share '$SHARE_NAME' → $TEST_DIR"

# ── Step 5: Wait for mount to appear ──────────────────────────────────────────
echo ""
echo "── Step 5: Waiting for mount to appear in container (up to ${MOUNT_TIMEOUT}s) ──"
MOUNT_APPEARED=false
elapsed=0
while (( elapsed < MOUNT_TIMEOUT )); do
  if container_exec test -d "/root/LazyMount/$SHARE_NAME" 2>/dev/null; then
    echo "Mount appeared after ${elapsed}s"
    MOUNT_APPEARED=true
    break
  fi
  # Show dots for progress
  printf "."
  sleep 2
  (( elapsed += 2 ))
done
echo ""

if ! $MOUNT_APPEARED; then
  echo ""
  echo "[FAIL] Mount did not appear within ${MOUNT_TIMEOUT}s"
  echo ""
  echo "── Daemon log (last 30 lines) ───"
  tail -n 30 /tmp/lazymount-e2e-daemon.log || true
  echo ""
  echo "── Container logs ───"
  docker compose -f "$COMPOSE_FILE" logs lazymount-server || true
  exit 1
fi

# Give rclone mount a moment to fully initialize the VFS
sleep 2

# ── Test suite ────────────────────────────────────────────────────────────────
echo ""
echo "── Running tests ────────────────────────────────────────────────────"

# T1: hello.txt exists
if container_exec test -f "/root/LazyMount/$SHARE_NAME/hello.txt"; then
  pass "T1: hello.txt exists in container mount"
else
  fail "T1: hello.txt not found in container mount"
fi

# T2: hello.txt content matches
ACTUAL="$(container_exec cat "/root/LazyMount/$SHARE_NAME/hello.txt" 2>/dev/null || true)"
if [[ "$ACTUAL" == "hello lazymount" ]]; then
  pass "T2: hello.txt content matches"
else
  fail "T2: hello.txt content mismatch (got: '$ACTUAL')"
fi

# T3: multiline.txt content matches
ACTUAL="$(container_exec cat "/root/LazyMount/$SHARE_NAME/multiline.txt" 2>/dev/null || true)"
EXPECTED=$'line1\nline2\nline3'
if [[ "$ACTUAL" == "$EXPECTED" ]]; then
  pass "T3: multiline.txt content matches"
else
  fail "T3: multiline.txt content mismatch"
fi

# T4: subdirectory visible
if container_exec test -d "/root/LazyMount/$SHARE_NAME/subdir"; then
  pass "T4: subdir/ directory visible in mount"
else
  fail "T4: subdir/ not visible in mount"
fi

# T5: nested file accessible
ACTUAL="$(container_exec cat "/root/LazyMount/$SHARE_NAME/subdir/nested.txt" 2>/dev/null || true)"
if [[ "$ACTUAL" == "nested file content" ]]; then
  pass "T5: subdir/nested.txt accessible with correct content"
else
  fail "T5: subdir/nested.txt not accessible (got: '$ACTUAL')"
fi

# T6: lazymount mounts reports share as active
MOUNTS_OUT="$(container_exec lazymount mounts 2>/dev/null || true)"
if echo "$MOUNTS_OUT" | grep -q "$SHARE_NAME"; then
  pass "T6: 'lazymount mounts' lists $SHARE_NAME"
else
  fail "T6: $SHARE_NAME not in 'lazymount mounts' output"
  echo "    output: $MOUNTS_OUT"
fi

# T7: lazymount status shows server running
STATUS_OUT="$(container_exec lazymount server status 2>/dev/null || true)"
if echo "$STATUS_OUT" | grep -qi "running"; then
  pass "T7: 'lazymount server status' reports running"
else
  fail "T7: server not reported as running"
  echo "    output: $STATUS_OUT"
fi

# T8: New file propagates through the mount
# (rclone --dir-cache-time 30s means new files appear within ~30s)
echo ""
echo "── T8: Testing new file propagation (waiting up to ${NEW_FILE_TIMEOUT}s) ──"
NEW_FILE="$TEST_DIR/new-$(date +%s).txt"
NEW_CONTENT="dynamic file $(date)"
echo "$NEW_CONTENT" > "$NEW_FILE"
NEW_FNAME="$(basename "$NEW_FILE")"

elapsed=0
PROPAGATED=false
while (( elapsed < NEW_FILE_TIMEOUT )); do
  if container_exec test -f "/root/LazyMount/$SHARE_NAME/$NEW_FNAME" 2>/dev/null; then
    PROPAGATED=true
    break
  fi
  printf "."
  sleep 2
  (( elapsed += 2 ))
done
echo ""

if $PROPAGATED; then
  ACTUAL="$(container_exec cat "/root/LazyMount/$SHARE_NAME/$NEW_FNAME" 2>/dev/null || true)"
  if [[ "$ACTUAL" == "$NEW_CONTENT" ]]; then
    pass "T8: new file propagated within ${elapsed}s with correct content"
  else
    fail "T8: new file appeared but content mismatch (got: '$ACTUAL')"
  fi
else
  fail "T8: new file did not propagate within ${NEW_FILE_TIMEOUT}s"
fi

# ── Summary ───────────────────────────────────────────────────────────────────
echo ""
echo "══════════════════════════════════════════════════════════"
TOTAL=$(( PASS + FAIL ))
if (( FAIL == 0 )); then
  echo "  E2E PASS: $PASS/$TOTAL tests passed"
else
  echo "  E2E FAIL: $PASS passed, $FAIL failed out of $TOTAL tests"
fi
echo "══════════════════════════════════════════════════════════"
echo ""

if (( FAIL > 0 )); then
  echo "── Daemon log ───────────────────────────────────────────"
  tail -n 40 /tmp/lazymount-e2e-daemon.log || true
  echo ""
  echo "── Container logs ───────────────────────────────────────"
  docker compose -f "$COMPOSE_FILE" logs lazymount-server || true
  exit 1
fi

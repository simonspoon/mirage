#!/bin/bash
# Mirage Wizard Flow — Khora QA Script
# Verifies the full wizard: start blank -> import spec -> select endpoints -> serve API
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
SPEC_FILE="$SCRIPT_DIR/fixtures/petstore.yaml"
PORT=0
SID=""
MIRAGE_PID=""
PASS=0
FAIL=0
SCREENSHOTS_DIR="/tmp/mirage-qa-$(date +%s)"

mkdir -p "$SCREENSHOTS_DIR"

cleanup() {
    [ -n "$SID" ] && khora kill "$SID" 2>/dev/null || true
    [ -n "$MIRAGE_PID" ] && kill "$MIRAGE_PID" 2>/dev/null && wait "$MIRAGE_PID" 2>/dev/null || true
    echo ""
    echo "Screenshots: $SCREENSHOTS_DIR/"
    echo "Results: $PASS passed, $FAIL failed"
    [ "$FAIL" -eq 0 ] && echo "PASS" || echo "FAIL"
}
trap cleanup EXIT

check() {
    local name="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        echo "  [PASS] $name"
        PASS=$((PASS + 1))
    else
        echo "  [FAIL] $name"
        FAIL=$((FAIL + 1))
    fi
}

# Build
echo "Building mirage..."
cd "$PROJECT_DIR"
cargo build --quiet 2>&1

# Find a random port
PORT=$(python3 -c "import socket; s=socket.socket(); s.bind(('',0)); print(s.getsockname()[1]); s.close()")

# Start mirage without spec
echo "Starting mirage on port $PORT (no spec)..."
"$PROJECT_DIR/target/debug/mirage" --port "$PORT" &
MIRAGE_PID=$!
sleep 2

BASE="http://127.0.0.1:$PORT"

echo ""
echo "Step 1: Verify blank startup"
check "Admin API responds" curl -sf "$BASE/_api/admin/spec"
check "No spec loaded" bash -c "curl -sf '$BASE/_api/admin/spec' | grep -q 'No spec loaded'"
check "Mock API returns 404" bash -c "[ \$(curl -sf -o /dev/null -w '%{http_code}' '$BASE/pet') = '404' ]"

# Launch khora
echo ""
echo "Step 2: Launch browser and navigate to admin"
# Clean stale Chrome locks
find /private/var/folders -name "SingletonLock" -path "*/chromiumoxide-runner/*" -delete 2>/dev/null || true
SID=$(khora launch 2>&1 | grep "^Session:" | awk '{print $2}')
khora navigate "$SID" "$BASE/_admin/" 2>&1
sleep 2
khora screenshot "$SID" -o "$SCREENSHOTS_DIR/01-idle.png" 2>&1
check "Textarea exists" khora find "$SID" "#spec-input"
check "Import button exists" khora find "$SID" "#import-btn"

# Paste spec and import
echo ""
echo "Step 3: Import Swagger spec"
SPEC_JSON=$(python3 -c "import sys,json; print(json.dumps(open('$SPEC_FILE').read()))")
khora eval "$SID" "document.getElementById('spec-input').value = ${SPEC_JSON}; document.getElementById('spec-input').dispatchEvent(new Event('input', {bubbles: true}));" 2>&1
khora click "$SID" "#import-btn" 2>&1
sleep 2
khora screenshot "$SID" -o "$SCREENSHOTS_DIR/02-selecting.png" 2>&1
check "Endpoint list appears" khora find "$SID" "#endpoint-list"
check "Start button appears" khora find "$SID" "#start-btn"
check "Seed count input exists" khora find "$SID" "#seed-count"

# Click start
echo ""
echo "Step 4: Configure and start mock server"
khora click "$SID" "#start-btn" 2>&1
sleep 2
khora screenshot "$SID" -o "$SCREENSHOTS_DIR/03-running.png" 2>&1
check "Dashboard shows active endpoints" khora find "$SID" "table"
check "Reset button exists" khora find "$SID" "#reset-btn"

# Verify API
echo ""
echo "Step 5: Verify mock API serves data"
check "GET /pet returns 200" curl -sf "$BASE/pet"
check "GET /pet returns array" bash -c "curl -sf '$BASE/pet' | python3 -c 'import sys,json; assert json.load(sys.stdin).__class__.__name__==\"list\"'"
check "GET /pet returns 10 rows" bash -c "curl -sf '$BASE/pet' | python3 -c 'import sys,json; assert len(json.load(sys.stdin))==10'"
check "GET /pet/1 returns 200" curl -sf "$BASE/pet/1"
check "POST /pet returns 201" bash -c "[ \$(curl -sf -o /dev/null -w '%{http_code}' -X POST '$BASE/pet' -H 'Content-Type: application/json' -d '{\"name\":\"QADog\",\"status\":\"available\"}') = '201' ]"
check "DELETE /pet/1 returns 204" bash -c "[ \$(curl -sf -o /dev/null -w '%{http_code}' -X DELETE '$BASE/pet/1') = '204' ]"
check "GET /pet/1 returns 404 after delete" bash -c "[ \$(curl -sf -o /dev/null -w '%{http_code}' '$BASE/pet/1') = '404' ]"

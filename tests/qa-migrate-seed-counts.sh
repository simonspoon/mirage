#!/bin/bash
# Mirage Pre-Change Recipe Migration — Khora QA Script (task gtjj)
#
# Verifies that opening a recipe saved before the per-table seed_counts
# feature fans out the old scalar seed_count onto every response def's
# per-table rows input, and that saving persists the materialized map.
#
# Flow:
#   1. Build and start mirage in isolated workdir.
#   2. POST /_api/admin/recipes with seed_count=25 and no seed_counts to
#      simulate a pre-change recipe. Verify DB stores seed_counts="{}".
#   3. Launch visible khora at 1080p.
#   4. Navigate Recipes → Edit the test recipe → Next (Select → Config).
#   5. Read every table-header-seed-input DOM value — assert all show 25.
#   6. Next → Save. Wait for PUT to complete.
#   7. GET /_api/admin/recipes/{id} — assert seed_counts JSON has every
#      response def mapped to 25 (no longer empty).
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
SPEC_FILE="$SCRIPT_DIR/fixtures/mega.yaml"
PORT=0
SID=""
MIRAGE_PID=""
WORKDIR=""
PASS=0
FAIL=0
SCREENSHOTS_DIR="/tmp/mirage-qa-migrate-$(date +%s)"

mkdir -p "$SCREENSHOTS_DIR"

cleanup() {
    [ -n "$SID" ] && khora kill "$SID" 2>/dev/null || true
    [ -n "$MIRAGE_PID" ] && kill "$MIRAGE_PID" 2>/dev/null && wait "$MIRAGE_PID" 2>/dev/null || true
    [ -n "$WORKDIR" ] && rm -rf "$WORKDIR" 2>/dev/null || true
    echo ""
    echo "Screenshots: $SCREENSHOTS_DIR/"
    echo "Results: $PASS passed, $FAIL failed"
    if [ "$FAIL" -eq 0 ] && [ "$PASS" -gt 0 ]; then
        echo "PASS"
    else
        echo "FAIL"
    fi
}
trap cleanup EXIT

expect_eq() {
    local name="$1" expected="$2" actual="$3"
    if [ "$expected" = "$actual" ]; then
        echo "  [PASS] $name ($actual)"
        PASS=$((PASS + 1))
    else
        echo "  [FAIL] $name (expected=$expected actual=$actual)"
        FAIL=$((FAIL + 1))
    fi
}

keval() {
    local sid="$1" script="$2"
    khora eval "$sid" "(() => { $script; return 'ok'; })()" 2>&1
}

kread() {
    local sid="$1" expr="$2"
    khora eval "$sid" "(() => { const v = ($expr); return v === undefined || v === null ? '' : String(v); })()" 2>&1 | tail -1
}

# Build
echo "Building mirage..."
cd "$PROJECT_DIR"
cargo build --quiet 2>&1

# Random port + isolated workdir
PORT=$(python3 -c "import socket; s=socket.socket(); s.bind(('',0)); print(s.getsockname()[1]); s.close()")
BASE="http://127.0.0.1:$PORT"
WORKDIR="/tmp/mirage-qa-migrate-wd-$PORT"
mkdir -p "$WORKDIR"

echo "Starting mirage on port $PORT (workdir=$WORKDIR)..."
(cd "$WORKDIR" && "$PROJECT_DIR/target/debug/mirage" --port "$PORT") &
MIRAGE_PID=$!
sleep 2

# Stage pre-change recipe via API: seed_count=25, NO seed_counts field.
# Server defaults seed_counts column to "{}" — exact shape produced by
# recipes saved before the per-table feature landed.
echo ""
echo "Step 1: Create pre-change recipe via API (seed_count=25, no seed_counts)"
RECIPE_PAYLOAD=$(python3 <<PY
import json
spec = open("$SPEC_FILE").read()
body = {
    "name": "gtjj-pre-change",
    "spec_source": spec,
    "endpoints": [
        {"method": "get", "path": "/owners"},
        {"method": "get", "path": "/widgets"},
        {"method": "get", "path": "/gadgets"},
    ],
    "seed_count": 25,
}
print(json.dumps(body))
PY
)
RECIPE_ID=$(curl -sf -X POST -H "Content-Type: application/json" \
    -d "$RECIPE_PAYLOAD" "$BASE/_api/admin/recipes" \
    | python3 -c "import sys,json; print(json.load(sys.stdin)['id'])")
echo "  Created recipe id=$RECIPE_ID"

# Confirm pre-change shape in DB.
RAW_SEED_COUNTS=$(curl -sf "$BASE/_api/admin/recipes/$RECIPE_ID" \
    | python3 -c "import sys,json; print(json.load(sys.stdin)['seed_counts'])")
expect_eq "DB stores seed_counts as empty map before UI visit" "{}" "$RAW_SEED_COUNTS"

# Launch visible khora at 1080p.
echo ""
echo "Step 2: Launch visible browser at 1920x1080"
find /private/var/folders -name "SingletonLock" -path "*/chromiumoxide-runner/*" -delete 2>/dev/null || true
SID=$(khora launch --visible --window-size 1920x1080 2>&1 | grep "^Session:" | awk '{print $2}')
if [ -z "$SID" ]; then
    echo "ERROR: khora launch failed"
    exit 1
fi
khora navigate "$SID" "$BASE/_admin/" 2>&1
sleep 2
khora screenshot "$SID" -o "$SCREENSHOTS_DIR/01-admin.png" 2>&1 >/dev/null

# Navigate to Recipes page.
echo ""
echo "Step 3: Navigate to Recipes page"
keval "$SID" "
  const nav = [...document.querySelectorAll('nav button, nav a, nav [role=button]')].find(el => el.textContent.trim() === 'Recipes');
  if (!nav) throw new Error('Recipes nav item not found');
  nav.click();
" >/dev/null
sleep 1
khora screenshot "$SID" -o "$SCREENSHOTS_DIR/02-recipes.png" 2>&1 >/dev/null

# Click Edit on the test recipe.
echo ""
echo "Step 4: Click Edit on pre-change recipe"
keval "$SID" "
  const cards = [...document.querySelectorAll('div')].filter(d => d.textContent && d.textContent.includes('gtjj-pre-change'));
  const btn = [...document.querySelectorAll('button')].filter(b => b.textContent.trim() === 'Edit')[0];
  if (!btn) throw new Error('Edit button not found');
  btn.click();
" >/dev/null
sleep 1
khora screenshot "$SID" -o "$SCREENSHOTS_DIR/03-edit-select.png" 2>&1 >/dev/null

# Edit mode lands on Select step. Click Next → Config (triggers
# handleFetchGraph → handleGoToConfig → seed_counts fan-out).
echo ""
echo "Step 5: Next (Select → Config) — triggers migration fan-out"
keval "$SID" "
  const btn = [...document.querySelectorAll('button')].find(b => b.textContent.trim() === 'Next');
  if (!btn) throw new Error('Next button not found at select step');
  btn.click();
" >/dev/null
# Graph computation plus ref resolve takes a beat.
sleep 3
khora screenshot "$SID" -o "$SCREENSHOTS_DIR/04-config.png" 2>&1 >/dev/null

# Read every rendered table-header-seed-input. Every one must show 25.
echo ""
echo "Step 6: Verify every per-table rows input shows 25 (fan-out)"
INPUT_COUNT=$(kread "$SID" "document.querySelectorAll('[data-testid=table-header-seed-input]').length" || echo "0")
if [ -z "$INPUT_COUNT" ] || ! [ "$INPUT_COUNT" -gt 0 ] 2>/dev/null; then
    echo "  [FAIL] Per-table seed inputs present (INPUT_COUNT=$INPUT_COUNT)"
    FAIL=$((FAIL + 1))
else
    echo "  [PASS] Per-table seed inputs present ($INPUT_COUNT)"
    PASS=$((PASS + 1))
fi

# Check every input's value === "25". Return 'OK' if all match,
# else the first mismatched def:value pair.
MISMATCH=$(kread "$SID" "(() => { const els = [...document.querySelectorAll('[data-testid=table-header-seed-input]')]; const bad = els.find(el => el.value !== '25'); return bad ? (bad.getAttribute('data-def') + '=' + bad.value) : 'OK'; })()" || echo "ERR")
expect_eq "Every per-table input equals 25" "OK" "$MISMATCH"

# Advance to name step, save.
echo ""
echo "Step 7: Next → name → Save"
keval "$SID" "
  const btn = [...document.querySelectorAll('button')].find(b => b.textContent.trim() === 'Next');
  if (!btn) throw new Error('Next button not found at config step');
  btn.click();
" >/dev/null
sleep 1

keval "$SID" "
  const btn = [...document.querySelectorAll('button')].find(b => b.textContent.trim() === 'Save');
  if (!btn) throw new Error('Save button not found at name step');
  btn.click();
" >/dev/null
# Edit-mode PUT is quick but let UI settle.
sleep 3
khora screenshot "$SID" -o "$SCREENSHOTS_DIR/05-saved.png" 2>&1 >/dev/null

# GET the recipe and assert seed_counts persisted.
echo ""
echo "Step 8: Verify persisted seed_counts map"
PERSISTED=$(curl -sf "$BASE/_api/admin/recipes/$RECIPE_ID" \
    | python3 -c "import sys,json; print(json.load(sys.stdin)['seed_counts'])")
echo "  seed_counts = $PERSISTED"

# Non-empty map check.
MAP_SIZE=$(python3 -c "import json; m = json.loads('''$PERSISTED'''); print(len(m))")
expect_eq "Persisted seed_counts non-empty" "yes" "$([ "$MAP_SIZE" -gt 0 ] && echo yes || echo no)"

# Every value must be 25.
ALL_25=$(python3 -c "
import json
m = json.loads('''$PERSISTED''')
print('yes' if m and all(v == 25 for v in m.values()) else 'no')
")
expect_eq "Every persisted def's seed count == 25" "yes" "$ALL_25"

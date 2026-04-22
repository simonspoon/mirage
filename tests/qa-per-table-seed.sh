#!/bin/bash
# Mirage Per-Table Seed Counts — Khora QA Script (task mqvu)
#
# Verifies the Configure step of the recipe wizard renders one rows input per
# expanded table block bound to a per-def signal, so editing one table's count
# leaves every other table untouched both in the DOM and in the persisted
# activation behavior.
#
# Flow:
#   1. Build and start mirage.
#   2. Launch a visible Chrome via khora (1080p per user pref).
#   3. Drive the recipe wizard: Create Recipe → paste mega.yaml → Next →
#      select endpoints → Next → Configure → expand 2 table groups → set
#      distinct row counts → read each input back via DOM to confirm
#      independence → Next → name → Save & Activate.
#   4. curl the two collection endpoints; assert per-table row counts match
#      the values entered.
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
SCREENSHOTS_DIR="/tmp/mirage-qa-seed-$(date +%s)"

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

# Run JS via khora eval. Must wrap in IIFE returning a non-undefined value —
# khora eval rejects `undefined` with "JavaScript error: No value found".
keval() {
    local sid="$1" script="$2"
    khora eval "$sid" "(() => { $script; return 'ok'; })()" 2>&1
}

# Read a value from the page via khora eval.
kread() {
    local sid="$1" expr="$2"
    khora eval "$sid" "(() => { const v = ($expr); return v === undefined || v === null ? '' : String(v); })()" 2>&1 | tail -1
}

# Build
echo "Building mirage..."
cd "$PROJECT_DIR"
cargo build --quiet 2>&1

# Random port
PORT=$(python3 -c "import socket; s=socket.socket(); s.bind(('',0)); print(s.getsockname()[1]); s.close()")
BASE="http://127.0.0.1:$PORT"

# Isolated workdir so mirage.db does not collide with the project one.
WORKDIR="/tmp/mirage-qa-seed-wd-$PORT"
mkdir -p "$WORKDIR"

echo "Starting mirage on port $PORT (no spec, workdir=$WORKDIR)..."
(cd "$WORKDIR" && "$PROJECT_DIR/target/debug/mirage" --port "$PORT") &
MIRAGE_PID=$!
sleep 2

# Launch khora (visible, 1920x1080 per user pref).
echo ""
echo "Step 1: Launch visible browser at 1920x1080"
find /private/var/folders -name "SingletonLock" -path "*/chromiumoxide-runner/*" -delete 2>/dev/null || true
SID=$(khora launch --visible --window-size 1920x1080 2>&1 | grep "^Session:" | awk '{print $2}')
if [ -z "$SID" ]; then
    echo "ERROR: khora launch failed"
    exit 1
fi
khora navigate "$SID" "$BASE/_admin/" 2>&1
sleep 2
khora screenshot "$SID" -o "$SCREENSHOTS_DIR/01-admin.png" 2>&1 >/dev/null

# Click Recipes nav by text match (nav items have no data-testid).
echo ""
echo "Step 2: Navigate to Recipes page"
keval "$SID" "
  const nav = [...document.querySelectorAll('nav button, nav a, nav [role=button]')].find(el => el.textContent.trim() === 'Recipes');
  if (!nav) throw new Error('Recipes nav item not found');
  nav.click();
" >/dev/null
sleep 1
khora screenshot "$SID" -o "$SCREENSHOTS_DIR/02-recipes.png" 2>&1 >/dev/null

# Click "Create Recipe".
echo ""
echo "Step 3: Open Create Recipe"
keval "$SID" "
  const btn = [...document.querySelectorAll('button')].find(b => b.textContent.trim() === 'Create Recipe');
  if (!btn) throw new Error('Create Recipe button not found');
  btn.click();
" >/dev/null
sleep 1

# Paste mega.yaml into the textarea.
echo ""
echo "Step 4: Paste mega.yaml spec"
SPEC_JSON=$(python3 -c "import sys,json; print(json.dumps(open('$SPEC_FILE').read()))")
keval "$SID" "
  const ta = document.querySelector('textarea');
  if (!ta) throw new Error('spec textarea not found');
  ta.value = ${SPEC_JSON};
  ta.dispatchEvent(new Event('input', {bubbles: true}));
" >/dev/null
sleep 1

# Click Next → select.
keval "$SID" "
  const btn = [...document.querySelectorAll('button')].find(b => b.textContent.trim() === 'Next');
  if (!btn) throw new Error('Next button not found at paste step');
  btn.click();
" >/dev/null
sleep 1
khora screenshot "$SID" -o "$SCREENSHOTS_DIR/03-select.png" 2>&1 >/dev/null

# At select step, endpoints default to all selected. Click Next → config.
echo ""
echo "Step 5: Advance select → config"
keval "$SID" "
  const btn = [...document.querySelectorAll('button')].find(b => b.textContent.trim() === 'Next');
  if (!btn) throw new Error('Next button not found at select step');
  btn.click();
" >/dev/null
# Graph computation plus ref resolve takes a moment.
sleep 3
khora screenshot "$SID" -o "$SCREENSHOTS_DIR/04-config.png" 2>&1 >/dev/null

# Expand Widget and Gadget table blocks.
echo ""
echo "Step 6: Expand Widget + Gadget table blocks"
keval "$SID" "
  const targets = ['Widget', 'Gadget'];
  const headers = [...document.querySelectorAll('[data-testid=table-group-header]')];
  for (const name of targets) {
    const h = headers.find(h => h.querySelector('.font-medium')?.textContent?.trim() === name);
    if (!h) throw new Error('Table header not found: ' + name);
    h.click();
  }
" >/dev/null
sleep 1

check "Widget seed input present" bash -c "khora find '$SID' '[data-testid=table-header-seed-input][data-def=Widget]'"
check "Gadget seed input present" bash -c "khora find '$SID' '[data-testid=table-header-seed-input][data-def=Gadget]'"

# Set Widget = 7. Gadget MUST remain at its default (10).
echo ""
echo "Step 7: Set Widget rows = 7, verify Gadget untouched"
keval "$SID" "
  const el = document.querySelector('[data-testid=table-header-seed-input][data-def=Widget]');
  if (!el) throw new Error('Widget input not found');
  el.focus();
  el.value = '7';
  el.dispatchEvent(new Event('input', {bubbles: true}));
  el.blur();
" >/dev/null
sleep 1

WIDGET_VAL=$(kread "$SID" "document.querySelector('[data-testid=table-header-seed-input][data-def=Widget]').value")
GADGET_VAL=$(kread "$SID" "document.querySelector('[data-testid=table-header-seed-input][data-def=Gadget]').value")
expect_eq "Widget input reflects 7 after edit+blur" "7" "$WIDGET_VAL"
expect_eq "Gadget input untouched after Widget edit" "10" "$GADGET_VAL"

# Set Gadget = 3. Widget must stay at 7.
echo ""
echo "Step 8: Set Gadget rows = 3, verify Widget unchanged"
keval "$SID" "
  const el = document.querySelector('[data-testid=table-header-seed-input][data-def=Gadget]');
  if (!el) throw new Error('Gadget input not found');
  el.focus();
  el.value = '3';
  el.dispatchEvent(new Event('input', {bubbles: true}));
  el.blur();
" >/dev/null
sleep 1

WIDGET_VAL=$(kread "$SID" "document.querySelector('[data-testid=table-header-seed-input][data-def=Widget]').value")
GADGET_VAL=$(kread "$SID" "document.querySelector('[data-testid=table-header-seed-input][data-def=Gadget]').value")
expect_eq "Widget input still 7 after Gadget edit" "7" "$WIDGET_VAL"
expect_eq "Gadget input reflects 3" "3" "$GADGET_VAL"

# Advance to name step, save & activate.
echo ""
echo "Step 9: Next → name → Save & Activate"
keval "$SID" "
  const btn = [...document.querySelectorAll('button')].find(b => b.textContent.trim() === 'Next');
  btn.click();
" >/dev/null
sleep 1

keval "$SID" "
  const input = document.querySelector('input[placeholder*=Petstore]');
  if (!input) throw new Error('Recipe name input not found');
  input.focus();
  input.value = 'mqvu-qa';
  input.dispatchEvent(new Event('input', {bubbles: true}));
" >/dev/null
sleep 1

keval "$SID" "
  const btn = [...document.querySelectorAll('button')].find(b => b.textContent.trim().startsWith('Save & Activate'));
  if (!btn) throw new Error('Save & Activate button not found');
  btn.click();
" >/dev/null
# Activation re-seeds every table — takes a beat on mega.yaml.
sleep 5
khora screenshot "$SID" -o "$SCREENSHOTS_DIR/05-activated.png" 2>&1 >/dev/null

# Verify per-endpoint row counts match what was entered.
echo ""
echo "Step 10: Verify activated mock returns per-table row counts"
WIDGET_COUNT=$(curl -sf "$BASE/widgets" | python3 -c "import sys,json; print(len(json.load(sys.stdin)))")
GADGET_COUNT=$(curl -sf "$BASE/gadgets" | python3 -c "import sys,json; print(len(json.load(sys.stdin)))")
expect_eq "GET /widgets returns 7 rows" "7" "$WIDGET_COUNT"
expect_eq "GET /gadgets returns 3 rows" "3" "$GADGET_COUNT"

# Defs the user never edited fall back to scalar seed_count. /owners + /things
# should therefore use the wizard default of 10 (server default also 10).
OWNERS_COUNT=$(curl -sf "$BASE/owners" | python3 -c "import sys,json; print(len(json.load(sys.stdin)))")
expect_eq "GET /owners falls back to default 10" "10" "$OWNERS_COUNT"

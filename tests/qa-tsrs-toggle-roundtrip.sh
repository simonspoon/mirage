#!/bin/bash
# Mirage Endpoint-Layer Toggle Round-trip QA — Khora
# Task tsrs. Verifies Schemas > Endpoints toggle ON/OFF round-trip
# preserves orthogonal signals (expandedDefs, selectedEntities,
# graphFocused, graphExpanded) and schema-node name-set + positions
# restore after OFF. Plus Dashboard round-trip state preservation.
# Uses mega.yaml fixture. DOM-only; no runtime hooks.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
SPEC_FILE="$SCRIPT_DIR/fixtures/mega.yaml"
PORT=0
SID=""
MIRAGE_PID=""
PASS=0
FAIL=0
ABORTED=0
SCREENSHOTS_DIR="$SCRIPT_DIR/screenshots/tsrs-$(date +%s)"

mkdir -p "$SCREENSHOTS_DIR"

# ANSI colors for readability
RED=$'\033[0;31m'
GREEN=$'\033[0;32m'
YELLOW=$'\033[0;33m'
NC=$'\033[0m'

cleanup() {
    set +e
    if [ -n "$SID" ]; then
        khora kill "$SID" >/dev/null 2>&1 || true
    fi
    if [ -n "$MIRAGE_PID" ]; then
        kill "$MIRAGE_PID" >/dev/null 2>&1 || true
        wait "$MIRAGE_PID" 2>/dev/null || true
    fi
    # Backstop: any stray chromiumoxide-runner or mirage release from this run
    pkill -f chromiumoxide-runner >/dev/null 2>&1 || true
    pkill -f 'target/release/mirage' >/dev/null 2>&1 || true
    echo ""
    echo "Screenshots: $SCREENSHOTS_DIR/"
    echo "Results: $PASS passed, $FAIL failed"
    if [ "$FAIL" -eq 0 ] && [ "$ABORTED" -eq 0 ] && [ "$PASS" -gt 0 ]; then
        echo "${GREEN}PASS${NC}"
    else
        echo "${RED}FAIL${NC}"
    fi
}
trap cleanup EXIT INT TERM

check() {
    local name="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        echo "  ${GREEN}[PASS]${NC} $name"
        PASS=$((PASS + 1))
    else
        echo "  ${RED}[FAIL]${NC} $name"
        FAIL=$((FAIL + 1))
    fi
}

# ------------------------------------------------------------
# Pre-flight: kill stale mirage/chromiumoxide processes
# ------------------------------------------------------------
pkill -f 'target/release/mirage' >/dev/null 2>&1 || true
pkill -f chromiumoxide-runner >/dev/null 2>&1 || true

# ------------------------------------------------------------
# Build (UI first, then touch server.rs to defeat rust_embed mtime
# coarseness, then release build)
# ------------------------------------------------------------
cd "$PROJECT_DIR"
echo "Building UI..."
(cd ui && pnpm install --frozen-lockfile >/dev/null 2>&1 && pnpm build >/dev/null 2>&1)
echo "Touching src/server.rs to refresh rust_embed..."
touch src/server.rs
echo "Building mirage (release)..."
cargo build --release --quiet 2>&1
if [ ! -x "$PROJECT_DIR/target/release/mirage" ]; then
    ABORTED=1
    echo "FATAL: target/release/mirage missing after build"
    exit 1
fi

# ------------------------------------------------------------
# Pick ephemeral port
# ------------------------------------------------------------
PORT=$(python3 -c "import socket; s=socket.socket(); s.bind(('',0)); p=s.getsockname()[1]; s.close(); print(p)")
BASE="http://127.0.0.1:$PORT"
echo "Using port $PORT"

# ------------------------------------------------------------
# Spawn mirage (no spec — UI-driven import)
# ------------------------------------------------------------
echo "Starting mirage on $BASE ..."
cd "$PROJECT_DIR"
"$PROJECT_DIR/target/release/mirage" --port "$PORT" >/dev/null 2>&1 &
MIRAGE_PID=$!

# Readiness: curl /_api/admin/spec until 200 (20x 0.25s)
ready=0
for _ in $(seq 1 20); do
    if curl -sf "$BASE/_api/admin/spec" >/dev/null 2>&1; then
        ready=1
        break
    fi
    sleep 0.25
done
if [ "$ready" -ne 1 ]; then
    ABORTED=1
    echo "FATAL: mirage did not become ready at $BASE"
    exit 1
fi
echo "Mirage ready."

# ------------------------------------------------------------
# Launch khora and navigate to admin UI
# ------------------------------------------------------------
echo ""
echo "Step 1: Launch browser and navigate to admin"
find /private/var/folders -name "SingletonLock" -path "*/chromiumoxide-runner/*" -delete 2>/dev/null || true
SID=$(khora launch --visible 2>&1 | grep "^Session:" | awk '{print $2}')
if [ -z "$SID" ]; then
    ABORTED=1
    echo "FATAL: khora launch did not return a Session id"
    exit 1
fi
khora navigate "$SID" "$BASE/_admin/" >/dev/null 2>&1

# Poll for readiness: document complete + #spec-input exists
ready=0
for _ in $(seq 1 40); do
    out=$(khora eval "$SID" "(() => document.readyState === 'complete' && !!document.getElementById('spec-input'))()" 2>/dev/null || true)
    if echo "$out" | grep -q 'true'; then
        ready=1
        break
    fi
    sleep 0.25
done
if [ "$ready" -ne 1 ]; then
    ABORTED=1
    echo "FATAL: admin UI did not finish loading"
    exit 1
fi
khora screenshot "$SID" -o "$SCREENSHOTS_DIR/01-admin.png" >/dev/null 2>&1 || true

# ------------------------------------------------------------
# Import mega.yaml fixture
# ------------------------------------------------------------
echo ""
echo "Step 2: Import mega.yaml spec"
SPEC_JSON=$(python3 -c "import sys,json; print(json.dumps(open('$SPEC_FILE').read()))")
khora eval "$SID" "(() => { const el = document.getElementById('spec-input'); el.value = ${SPEC_JSON}; el.dispatchEvent(new Event('input', {bubbles: true})); return true; })()" >/dev/null 2>&1
khora click "$SID" "#import-btn" >/dev/null 2>&1

# Wait for endpoint list
ready=0
for _ in $(seq 1 40); do
    out=$(khora eval "$SID" "(() => !!document.getElementById('endpoint-list'))()" 2>/dev/null || true)
    if echo "$out" | grep -q 'true'; then ready=1; break; fi
    sleep 0.25
done
if [ "$ready" -ne 1 ]; then
    ABORTED=1
    echo "FATAL: endpoint list never appeared"
    exit 1
fi

# Start mock server
khora click "$SID" "#start-btn" >/dev/null 2>&1

# Wait for running dashboard
ready=0
for _ in $(seq 1 80); do
    out=$(khora eval "$SID" "(() => !!Array.from(document.querySelectorAll('h2')).find(e => e.textContent.trim() === 'Active Endpoints'))()" 2>/dev/null || true)
    if echo "$out" | grep -q 'true'; then ready=1; break; fi
    sleep 0.25
done
if [ "$ready" -ne 1 ]; then
    ABORTED=1
    echo "FATAL: dashboard did not enter running state"
    exit 1
fi
khora screenshot "$SID" -o "$SCREENSHOTS_DIR/02-running.png" >/dev/null 2>&1 || true

# ------------------------------------------------------------
# Navigate to Schemas tab
# ------------------------------------------------------------
echo ""
echo "Step 3: Navigate to Schemas tab"
khora eval "$SID" "(() => { const btn = Array.from(document.querySelectorAll('nav button')).find(b => b.textContent.trim() === 'Schemas'); if (btn) { btn.click(); return true; } return false; })()" >/dev/null 2>&1

# Poll for Schemas page: h2 'Schemas' AND toggle present
ready=0
for _ in $(seq 1 40); do
    out=$(khora eval "$SID" "(() => { const h = Array.from(document.querySelectorAll('h2')).find(e => e.textContent.trim() === 'Schemas'); const toggle = Array.from(document.querySelectorAll('.flex.gap-1.flex-wrap button')).find(b => b.textContent.trim() === 'Endpoints'); return !!h && !!toggle; })()" 2>/dev/null || true)
    if echo "$out" | grep -q 'true'; then ready=1; break; fi
    sleep 0.25
done
if [ "$ready" -ne 1 ]; then
    ABORTED=1
    echo "FATAL: Schemas page (with Endpoints toggle) did not render"
    exit 1
fi
khora screenshot "$SID" -o "$SCREENSHOTS_DIR/03-schemas-initial.png" >/dev/null 2>&1 || true

# ------------------------------------------------------------
# READ_STATE JavaScript helper — atomic JSON snapshot.
# Returns JSON string:
#   {schemaNodes:[{name,x,y}], expandedDefs:[...],
#    selectedEntities:[...], graphFocused:bool,
#    graphExpanded:[...]}
# Name resolution per entity-box:
#   - prefer [data-neighbor-stub] [data-entity-name] (stubs)
#   - else child text[data-entity-box-header] textContent
# Position via transform attr translate(x, y) with Math.round.
# ------------------------------------------------------------
IFS='' read -r -d '' READ_STATE <<'JS' || true
(() => {
    const boxes = Array.from(document.querySelectorAll('g[data-entity-box]'));
    const schemaNodes = boxes.map(g => {
        let name = g.getAttribute('data-entity-name');
        if (!name) {
            const header = g.querySelector('text[data-entity-box-header]');
            name = header ? (header.textContent || '').trim() : '';
        }
        const t = g.getAttribute('transform') || '';
        const m = t.match(/translate\(([^,\s]+)[,\s]+([^)\s]+)\)/);
        const x = m ? Math.round(parseFloat(m[1])) : 0;
        const y = m ? Math.round(parseFloat(m[2])) : 0;
        return { name, x, y };
    }).filter(n => n.name.length > 0);
    const expandedDefs = Array.from(document.querySelectorAll('[data-entity]'))
        .filter(e => !!e.querySelector('svg.rotate-90'))
        .map(e => e.getAttribute('data-entity'))
        .filter(Boolean)
        .sort();
    const selectedEntities = Array.from(document.querySelectorAll('[data-entity]'))
        .filter(e => {
            const btn = e.querySelector('button');
            return btn && btn.className && btn.className.indexOf('bg-blue-600/15') >= 0;
        })
        .map(e => e.getAttribute('data-entity'))
        .filter(Boolean)
        .sort();
    const graphFocused = !!Array.from(document.querySelectorAll('button'))
        .find(b => (b.textContent || '').trim() === 'Clear' && b.className && b.className.indexOf('bg-gray-800/80') >= 0);
    const graphExpanded = boxes
        .filter(g => !!g.querySelector('[data-entity-scroll]'))
        .map(g => {
            let name = g.getAttribute('data-entity-name');
            if (!name) {
                const header = g.querySelector('text[data-entity-box-header]');
                name = header ? (header.textContent || '').trim() : '';
            }
            return name;
        })
        .filter(Boolean)
        .sort();
    return JSON.stringify({ schemaNodes, expandedDefs, selectedEntities, graphFocused, graphExpanded });
})()
JS

# Helpers
# read_state(): invoke the READ_STATE JS; it returns a JSON string via
# JSON.stringify(...). In khora eval text mode the result is printed
# surrounded by double quotes with internal quotes backslash-escaped —
# unwrap via python json.loads so downstream consumers see raw JSON text.
read_state() {
    local raw
    raw=$(khora eval "$SID" "$READ_STATE" 2>/dev/null || true)
    # If raw starts and ends with double quotes, it is a JSON-encoded string —
    # unescape by letting python decode it. Otherwise return raw as-is.
    python3 -c "
import sys, json
raw = sys.stdin.read().strip()
if raw.startswith('\"') and raw.endswith('\"'):
    try:
        print(json.loads(raw))
        sys.exit(0)
    except Exception:
        pass
print(raw)
" <<< "$raw"
}

# Stability-gate snapshot: two consecutive READ_STATE reads 250ms apart
# must have identical schemaNodes JSON. Up to 20 attempts.
stable_snapshot() {
    local last=""
    local cur=""
    local attempts=20
    local i=0
    last=$(read_state)
    while [ $i -lt $attempts ]; do
        sleep 0.25
        cur=$(read_state)
        # Compare schemaNodes array in JSON via python
        if A="$last" B="$cur" python3 - <<'PY' 2>/dev/null
import json, os, sys
a = json.loads(os.environ["A"])
b = json.loads(os.environ["B"])
sys.exit(0 if a.get("schemaNodes") == b.get("schemaNodes") else 1)
PY
        then
            echo "$cur"
            return 0
        fi
        last="$cur"
        i=$((i + 1))
    done
    # Return last sample even if not converged
    echo "$cur"
    return 0
}

# Poll generic: eval a JS expression, succeed when numeric result matches
# comparator. Args: comparator (ge|eq), threshold, max_attempts, sleep, expr
poll_num() {
    local cmp="$1" thr="$2" max="$3" slp="$4" expr="$5"
    local i=0
    while [ $i -lt $max ]; do
        local out n
        out=$(khora eval "$SID" "$expr" 2>/dev/null || true)
        n=$(echo "$out" | tr -d '\r\n ' | grep -oE '[0-9]+' | head -n1 || true)
        if [ -n "$n" ]; then
            if [ "$cmp" = "ge" ] && [ "$n" -ge "$thr" ]; then return 0; fi
            if [ "$cmp" = "eq" ] && [ "$n" -eq "$thr" ]; then return 0; fi
        fi
        sleep "$slp"
        i=$((i + 1))
    done
    return 1
}

# Poll toggle class: return 0 when toggle class state matches desired (on|off).
poll_toggle_class() {
    local desired="$1" max="$2" slp="$3"
    local i=0
    while [ $i -lt $max ]; do
        local out
        out=$(khora eval "$SID" "(() => { const btn = Array.from(document.querySelectorAll('.flex.gap-1.flex-wrap button')).find(b => b.textContent.trim() === 'Endpoints'); return btn ? btn.className.includes('bg-blue-600/20') : null; })()" 2>/dev/null || true)
        if [ "$desired" = "on" ] && echo "$out" | grep -q 'true'; then return 0; fi
        if [ "$desired" = "off" ] && echo "$out" | grep -q 'false'; then return 0; fi
        sleep "$slp"
        i=$((i + 1))
    done
    return 1
}

click_endpoints_toggle() {
    khora eval "$SID" "(() => { const btn = Array.from(document.querySelectorAll('.flex.gap-1.flex-wrap button')).find(b => b.textContent.trim() === 'Endpoints'); if (btn) { btn.click(); return true; } return false; })()" >/dev/null 2>&1
}

# JSON field-equality via python. Usage: json_eq <field> <a> <b>
json_eq() {
    FIELD="$1" A="$2" B="$3" python3 - <<'PY'
import json, os, sys
field = os.environ["FIELD"]
a = json.loads(os.environ["A"])
b = json.loads(os.environ["B"])
sys.exit(0 if a.get(field) == b.get(field) else 1)
PY
}

# Position-equality comparison (exact round to int). If fails, optionally
# retry with epsilon=2 — currently strict.
positions_eq_exact() {
    A="$1" B="$2" python3 - <<'PY'
import json, os, sys
a = json.loads(os.environ["A"])["schemaNodes"]
b = json.loads(os.environ["B"])["schemaNodes"]
am = {n["name"]: (n["x"], n["y"]) for n in a}
bm = {n["name"]: (n["x"], n["y"]) for n in b}
if set(am.keys()) != set(bm.keys()):
    sys.exit(1)
for k in am:
    if am[k] != bm[k]:
        sys.exit(1)
sys.exit(0)
PY
}

positions_eq_epsilon() {
    A="$1" B="$2" python3 - <<'PY'
import json, os, sys
a = json.loads(os.environ["A"])["schemaNodes"]
b = json.loads(os.environ["B"])["schemaNodes"]
am = {n["name"]: (n["x"], n["y"]) for n in a}
bm = {n["name"]: (n["x"], n["y"]) for n in b}
if set(am.keys()) != set(bm.keys()):
    sys.exit(1)
eps = 2
for k in am:
    ax, ay = am[k]
    bx, by = bm[k]
    if abs(ax - bx) > eps or abs(ay - by) > eps:
        sys.exit(1)
sys.exit(0)
PY
}

names_eq() {
    A="$1" B="$2" python3 - <<'PY'
import json, os, sys
a = sorted(n["name"] for n in json.loads(os.environ["A"])["schemaNodes"])
b = sorted(n["name"] for n in json.loads(os.environ["B"])["schemaNodes"])
sys.exit(0 if a == b else 1)
PY
}

# Sanity that READ_STATE returns parseable JSON with expected keys
probe=$(read_state)
if ! PROBE="$probe" python3 - <<'PY' 2>/dev/null
import json, os, sys
d = json.loads(os.environ["PROBE"])
for k in ("schemaNodes", "expandedDefs", "selectedEntities", "graphFocused", "graphExpanded"):
    assert k in d, k
PY
then
    ABORTED=1
    echo "FATAL: READ_STATE did not return expected JSON shape. Got:"
    echo "$probe" | head -c 400
    exit 1
fi

# ------------------------------------------------------------
# Check 1: Endpoints toggle button present
# Check 2: Toggle default OFF on first Schemas load
# ------------------------------------------------------------
echo ""
echo "Phase A: Toggle presence + default OFF"
check "Endpoints toggle button present" bash -c "
    out=\$(khora eval '$SID' \"(() => { const btn = Array.from(document.querySelectorAll('.flex.gap-1.flex-wrap button')).find(b => b.textContent.trim() === 'Endpoints'); return !!btn; })()\")
    echo \"\$out\" | grep -q 'true'
"
check "toggle default OFF on first Schemas load" bash -c "
    out=\$(khora eval '$SID' \"(() => { const btn = Array.from(document.querySelectorAll('.flex.gap-1.flex-wrap button')).find(b => b.textContent.trim() === 'Endpoints'); return btn ? !btn.className.includes('bg-blue-600/20') : false; })()\")
    echo \"\$out\" | grep -q 'true'
"

# ------------------------------------------------------------
# PRE_ON snapshot
# ------------------------------------------------------------
echo ""
echo "Phase B: PRE_ON snapshot"
PRE_ON=$(stable_snapshot)
khora screenshot "$SID" -o "$SCREENSHOTS_DIR/04-pre-on.png" >/dev/null 2>&1 || true

check "PRE_ON schemaNodes.length >= 5" env PRE_ON="$PRE_ON" python3 -c '
import json, os
d = json.loads(os.environ["PRE_ON"])
assert len(d["schemaNodes"]) >= 5, len(d["schemaNodes"])
'

check "PRE_ON selectedEntities empty" env PRE_ON="$PRE_ON" python3 -c '
import json, os
d = json.loads(os.environ["PRE_ON"])
assert d["selectedEntities"] == [], d["selectedEntities"]
'

check "PRE_ON graphFocused === false" env PRE_ON="$PRE_ON" python3 -c '
import json, os
d = json.loads(os.environ["PRE_ON"])
assert d["graphFocused"] is False, d["graphFocused"]
'

# ------------------------------------------------------------
# Toggle ON
# ------------------------------------------------------------
echo ""
echo "Phase C: Toggle ON"
click_endpoints_toggle

if poll_toggle_class on 20 0.1; then on_class=1; else on_class=0; fi
check "toggle class reads ON after click" bash -c "[ '$on_class' = '1' ]"

if poll_num ge 1 40 0.1 "(() => document.querySelectorAll('[data-endpoint-key]').length)()"; then ep_on=1; else ep_on=0; fi
check "[data-endpoint-key] count >= 1 after ON" bash -c "[ '$ep_on' = '1' ]"

POST_ON=$(stable_snapshot)
khora screenshot "$SID" -o "$SCREENSHOTS_DIR/05-post-on.png" >/dev/null 2>&1 || true

check "expandedDefs equal PRE_ON vs POST_ON" json_eq expandedDefs "$PRE_ON" "$POST_ON"
check "selectedEntities equal PRE_ON vs POST_ON" json_eq selectedEntities "$PRE_ON" "$POST_ON"
check "graphFocused equal PRE_ON vs POST_ON" json_eq graphFocused "$PRE_ON" "$POST_ON"
check "graphExpanded equal PRE_ON vs POST_ON" json_eq graphExpanded "$PRE_ON" "$POST_ON"

# ------------------------------------------------------------
# Toggle OFF
# ------------------------------------------------------------
echo ""
echo "Phase D: Toggle OFF"
click_endpoints_toggle

poll_toggle_class off 20 0.1 || true
# Toggle OFF class flip is informational only; if the class fails to flip
# we will catch via endpoint-node count and positions checks below.

if poll_num eq 0 40 0.1 "(() => document.querySelectorAll('[data-endpoint-node]').length)()"; then ep_off=1; else ep_off=0; fi
check "[data-endpoint-node] count == 0 after OFF" bash -c "[ '$ep_off' = '1' ]"

POST_OFF=$(stable_snapshot)
khora screenshot "$SID" -o "$SCREENSHOTS_DIR/06-post-off.png" >/dev/null 2>&1 || true

check "schemaNodes name-set equal PRE_ON vs POST_OFF" names_eq "$PRE_ON" "$POST_OFF"

# Positions: prefer exact; if strict fails, the check is recorded as FAIL.
# dagLayout.ts:585-603 guarantees byte-identical OFF path — strict should hold.
check "schemaNodes positions equal PRE_ON vs POST_OFF" positions_eq_exact "$PRE_ON" "$POST_OFF"

# ------------------------------------------------------------
# Dashboard round-trip with toggle OFF
# ------------------------------------------------------------
echo ""
echo "Phase E: Dashboard round-trip — state OFF preserved"
# Click Dashboard
khora eval "$SID" "(() => { const btn = Array.from(document.querySelectorAll('nav button')).find(b => b.textContent.trim() === 'Dashboard'); if (btn) { btn.click(); return true; } return false; })()" >/dev/null 2>&1
# Poll for dashboard heading
ready=0
for _ in $(seq 1 40); do
    out=$(khora eval "$SID" "(() => !!Array.from(document.querySelectorAll('h2')).find(e => e.textContent.trim() === 'Active Endpoints'))()" 2>/dev/null || true)
    if echo "$out" | grep -q 'true'; then ready=1; break; fi
    sleep 0.1
done
khora screenshot "$SID" -o "$SCREENSHOTS_DIR/07-dashboard.png" >/dev/null 2>&1 || true

# Click Schemas
khora eval "$SID" "(() => { const btn = Array.from(document.querySelectorAll('nav button')).find(b => b.textContent.trim() === 'Schemas'); if (btn) { btn.click(); return true; } return false; })()" >/dev/null 2>&1
# Poll for Schemas heading + toggle
ready=0
for _ in $(seq 1 40); do
    out=$(khora eval "$SID" "(() => { const h = Array.from(document.querySelectorAll('h2')).find(e => e.textContent.trim() === 'Schemas'); const toggle = Array.from(document.querySelectorAll('.flex.gap-1.flex-wrap button')).find(b => b.textContent.trim() === 'Endpoints'); return !!h && !!toggle; })()" 2>/dev/null || true)
    if echo "$out" | grep -q 'true'; then ready=1; break; fi
    sleep 0.1
done

check "toggle state preserved as OFF after Dashboard round-trip" bash -c "
    out=\$(khora eval '$SID' \"(() => { const btn = Array.from(document.querySelectorAll('.flex.gap-1.flex-wrap button')).find(b => b.textContent.trim() === 'Endpoints'); return btn ? !btn.className.includes('bg-blue-600/20') : false; })()\")
    echo \"\$out\" | grep -q 'true'
"

# ------------------------------------------------------------
# Toggle ON, Dashboard round-trip, assert ON preserved
# ------------------------------------------------------------
echo ""
echo "Phase F: Dashboard round-trip — state ON preserved"
click_endpoints_toggle
if ! poll_toggle_class on 20 0.1; then echo "WARN: toggle class did not flip ON"; fi
poll_num ge 1 40 0.1 "(() => document.querySelectorAll('[data-endpoint-key]').length)()" || true

# Click Dashboard
khora eval "$SID" "(() => { const btn = Array.from(document.querySelectorAll('nav button')).find(b => b.textContent.trim() === 'Dashboard'); if (btn) { btn.click(); return true; } return false; })()" >/dev/null 2>&1
ready=0
for _ in $(seq 1 40); do
    out=$(khora eval "$SID" "(() => !!Array.from(document.querySelectorAll('h2')).find(e => e.textContent.trim() === 'Active Endpoints'))()" 2>/dev/null || true)
    if echo "$out" | grep -q 'true'; then ready=1; break; fi
    sleep 0.1
done

# Click Schemas
khora eval "$SID" "(() => { const btn = Array.from(document.querySelectorAll('nav button')).find(b => b.textContent.trim() === 'Schemas'); if (btn) { btn.click(); return true; } return false; })()" >/dev/null 2>&1
ready=0
for _ in $(seq 1 40); do
    out=$(khora eval "$SID" "(() => { const h = Array.from(document.querySelectorAll('h2')).find(e => e.textContent.trim() === 'Schemas'); const toggle = Array.from(document.querySelectorAll('.flex.gap-1.flex-wrap button')).find(b => b.textContent.trim() === 'Endpoints'); return !!h && !!toggle; })()" 2>/dev/null || true)
    if echo "$out" | grep -q 'true'; then ready=1; break; fi
    sleep 0.1
done
khora screenshot "$SID" -o "$SCREENSHOTS_DIR/08-roundtrip-on.png" >/dev/null 2>&1 || true

check "toggle state preserved as ON after Dashboard round-trip" bash -c "
    out=\$(khora eval '$SID' \"(() => { const btn = Array.from(document.querySelectorAll('.flex.gap-1.flex-wrap button')).find(b => b.textContent.trim() === 'Endpoints'); return btn ? btn.className.includes('bg-blue-600/20') : false; })()\")
    echo \"\$out\" | grep -q 'true'
"

if poll_num ge 1 40 0.1 "(() => document.querySelectorAll('[data-endpoint-key]').length)()"; then ep_rt=1; else ep_rt=0; fi
check "[data-endpoint-key] >= 1 after round-trip with state ON" bash -c "[ '$ep_rt' = '1' ]"

# ------------------------------------------------------------
# ON -> OFF -> ON: schemaNode name-set stable
# Current state is ON (after the round-trip). Flip OFF, poll, flip ON, poll,
# snapshot, compare name-set to POST_ON.
# ------------------------------------------------------------
echo ""
echo "Phase G: ON-OFF-ON cycle stability"
# Capture fresh ON snapshot (after round-trip) as baseline for this cycle
CYCLE_ON_BASE=$(stable_snapshot)

click_endpoints_toggle
poll_toggle_class off 20 0.1 || true
poll_num eq 0 40 0.1 "(() => document.querySelectorAll('[data-endpoint-node]').length)()" || true

click_endpoints_toggle
poll_toggle_class on 20 0.1 || true
poll_num ge 1 40 0.1 "(() => document.querySelectorAll('[data-endpoint-key]').length)()" || true

CYCLE_ON_AFTER=$(stable_snapshot)

check "ON-OFF-ON leaves schemaNode name-set stable" names_eq "$CYCLE_ON_BASE" "$CYCLE_ON_AFTER"

echo ""
echo "All acceptance checks complete. Cleanup trap runs on EXIT."
[ "$FAIL" -eq 0 ] && exit 0 || exit 1

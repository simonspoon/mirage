#!/bin/bash
# Mirage Endpoint-Layer + hide-with-no-refs Integration — Khora QA Script
# Task hhnn. Verifies Schemas > Endpoints toggle ON/OFF and the
# "Endpoints without schemas" virtualRoots section using mega.yaml fixture.
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
SCREENSHOTS_DIR="/tmp/mirage-qa-endpoint-layer-$(date +%s)"

mkdir -p "$SCREENSHOTS_DIR"

cleanup() {
    set +e
    if [ -n "$SID" ]; then
        khora kill "$SID" >/dev/null 2>&1 || true
    fi
    if [ -n "$MIRAGE_PID" ]; then
        kill "$MIRAGE_PID" >/dev/null 2>&1 || true
        wait "$MIRAGE_PID" 2>/dev/null || true
    fi
    # Broad safety: stray chromiumoxide-runner from this session
    pkill -f chromiumoxide-runner >/dev/null 2>&1 || true
    echo ""
    echo "Screenshots: $SCREENSHOTS_DIR/"
    echo "Results: $PASS passed, $FAIL failed"
    if [ "$FAIL" -eq 0 ] && [ "$ABORTED" -eq 0 ] && [ "$PASS" -gt 0 ]; then
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

# Wait for running dashboard — "Active Endpoints" heading appears in running state.
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
# Navigate to Schemas tab (sidebar button, text match via IIFE)
# ------------------------------------------------------------
echo ""
echo "Step 3: Navigate to Schemas tab"
khora eval "$SID" "(() => { const btn = Array.from(document.querySelectorAll('nav button')).find(b => b.textContent.trim() === 'Schemas'); if (btn) { btn.click(); return true; } return false; })()" >/dev/null 2>&1

# Poll for Schemas page: heading h2 textContent 'Schemas' AND toggle container present
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
khora screenshot "$SID" -o "$SCREENSHOTS_DIR/03-schemas.png" >/dev/null 2>&1 || true

# ------------------------------------------------------------
# AC2: Endpoints toggle present and starts OFF
# ------------------------------------------------------------
echo ""
echo "AC2: Endpoints toggle present + starts OFF"
check "Endpoints toggle exists in Schemas panel" bash -c "
    out=\$(khora eval '$SID' \"(() => { const btn = Array.from(document.querySelectorAll('.flex.gap-1.flex-wrap button')).find(b => b.textContent.trim() === 'Endpoints'); return !!btn; })()\")
    echo \"\$out\" | grep -q 'true'
"
check "Endpoints toggle starts OFF (no bg-blue-600/20 class)" bash -c "
    out=\$(khora eval '$SID' \"(() => { const btn = Array.from(document.querySelectorAll('.flex.gap-1.flex-wrap button')).find(b => b.textContent.trim() === 'Endpoints'); return btn ? !btn.className.includes('bg-blue-600/20') : false; })()\")
    echo \"\$out\" | grep -q 'true'
"

# ------------------------------------------------------------
# AC3: Toggle ON -> [data-endpoint-node] count >= 1 and each node has
# HTTP method + path fragment in textContent.
# ------------------------------------------------------------
echo ""
echo "AC3: Toggle ON produces endpoint nodes"
khora eval "$SID" "(() => { const btn = Array.from(document.querySelectorAll('.flex.gap-1.flex-wrap button')).find(b => b.textContent.trim() === 'Endpoints'); if (btn) { btn.click(); return true; } return false; })()" >/dev/null 2>&1

# Poll until endpoint nodes appear (4s @ 100ms)
got=0
for _ in $(seq 1 40); do
    out=$(khora eval "$SID" "(() => document.querySelectorAll('[data-endpoint-node]').length)()" 2>/dev/null || true)
    # khora returns a value; capture numeric
    n=$(echo "$out" | tr -d '\r\n ' | grep -oE '[0-9]+' | head -n1 || true)
    if [ -n "$n" ] && [ "$n" -ge 1 ]; then got=1; break; fi
    sleep 0.1
done
check "At least 1 [data-endpoint-node] after toggle ON" bash -c "[ '$got' = '1' ]"
khora screenshot "$SID" -o "$SCREENSHOTS_DIR/04-endpoints-on.png" >/dev/null 2>&1 || true

check "Every endpoint node textContent contains HTTP method + path fragment" bash -c "
    out=\$(khora eval '$SID' \"(() => { const methods = ['GET','POST','PUT','PATCH','DELETE','HEAD','OPTIONS']; const nodes = Array.from(document.querySelectorAll('[data-endpoint-node]')); if (nodes.length === 0) return false; return nodes.every(n => { const t = n.textContent || ''; const hasMethod = methods.some(m => t.includes(m)); const hasPath = t.includes('/'); return hasMethod && hasPath; }); })()\")
    echo \"\$out\" | grep -q 'true'
"

# ------------------------------------------------------------
# AC4: path[data-edge-direction="input"] >= 1 AND
# path[data-edge-direction="output"] >= 1.
# Directed endpoint edges only render inside the *detail* Graph tab
# (conditional on selectedEntities size > 0). Pick the first schema
# in the sidebar left-panel, click it, switch to Graph tab, then poll.
# ------------------------------------------------------------
echo ""
echo "AC4: Directed endpoint edges present (in detail Graph tab)"

# Click "Widget" entity in sidebar — has both input (POST/PUT body ref) and
# output (GET 200 ref) edges in mega.yaml, so data-edge-direction "input" and
# "output" are both guaranteed non-zero.
khora eval "$SID" "(() => { const el = document.querySelector('[data-entity=\"Widget\"]'); if (!el) { const fb = document.querySelector('[data-entity]'); if (!fb) return false; const btn = fb.querySelector('button'); if (btn) { btn.click(); return true; } return false; } const btn = el.querySelector('button'); if (btn) { btn.click(); return true; } return false; })()" >/dev/null 2>&1

# Poll for Details/Graph tab bar appearance then switch to Graph.
ready=0
for _ in $(seq 1 40); do
    out=$(khora eval "$SID" "(() => !!Array.from(document.querySelectorAll('button')).find(b => b.textContent.trim() === 'Graph'))()" 2>/dev/null || true)
    if echo "$out" | grep -q 'true'; then ready=1; break; fi
    sleep 0.1
done
khora eval "$SID" "(() => { const btn = Array.from(document.querySelectorAll('button')).find(b => b.textContent.trim() === 'Graph'); if (btn) { btn.click(); return true; } return false; })()" >/dev/null 2>&1

# Poll for directed edges (up to 8s @ 100ms — dagre layout + toggle ON both).
inputN=0
outputN=0
for _ in $(seq 1 80); do
    i=$(khora eval "$SID" "(() => document.querySelectorAll('path[data-edge-direction=input]').length)()" 2>/dev/null | tr -d '\r\n ' | grep -oE '[0-9]+' | head -n1 || true)
    o=$(khora eval "$SID" "(() => document.querySelectorAll('path[data-edge-direction=output]').length)()" 2>/dev/null | tr -d '\r\n ' | grep -oE '[0-9]+' | head -n1 || true)
    inputN=${i:-0}
    outputN=${o:-0}
    if [ "$inputN" -ge 1 ] && [ "$outputN" -ge 1 ]; then break; fi
    sleep 0.1
done
khora screenshot "$SID" -o "$SCREENSHOTS_DIR/04b-graph-tab.png" >/dev/null 2>&1 || true
check "path[data-edge-direction=input] count >= 1" bash -c "[ '$inputN' -ge 1 ]"
check "path[data-edge-direction=output] count >= 1" bash -c "[ '$outputN' -ge 1 ]"

# ------------------------------------------------------------
# AC5: Toggle OFF -> [data-endpoint-node] == 0 AND
# [data-edge-direction] == 0 (poll until zero).
# ------------------------------------------------------------
echo ""
echo "AC5: Toggle OFF restores baseline"
khora eval "$SID" "(() => { const btn = Array.from(document.querySelectorAll('.flex.gap-1.flex-wrap button')).find(b => b.textContent.trim() === 'Endpoints'); if (btn) { btn.click(); return true; } return false; })()" >/dev/null 2>&1

off=0
for _ in $(seq 1 40); do
    out=$(khora eval "$SID" "(() => document.querySelectorAll('[data-endpoint-node]').length)()" 2>/dev/null || true)
    n=$(echo "$out" | tr -d '\r\n ' | grep -oE '[0-9]+' | head -n1 || true)
    if [ -n "$n" ] && [ "$n" -eq 0 ]; then off=1; break; fi
    sleep 0.1
done
check "[data-endpoint-node] count == 0 after toggle OFF" bash -c "[ '$off' = '1' ]"
check "[data-edge-direction] count == 0 after toggle OFF" bash -c "
    out=\$(khora eval '$SID' \"(() => document.querySelectorAll('[data-edge-direction]').length)()\")
    n=\$(echo \"\$out\" | tr -d '\r\n ' | grep -oE '[0-9]+' | head -n1)
    [ -n \"\$n\" ] && [ \"\$n\" -eq 0 ]
"
khora screenshot "$SID" -o "$SCREENSHOTS_DIR/05-endpoints-off.png" >/dev/null 2>&1 || true

# ------------------------------------------------------------
# AC6: 'Endpoints without schemas' section expands and lists >=1
# primitive/empty-response endpoint from mega.yaml fixture.
# mega.yaml has /labels (primitive array) and /ping (204 empty) as virtualRoots.
# ------------------------------------------------------------
echo ""
echo "AC6: 'Endpoints without schemas' section expands and lists rows"

# Locate the section button and click to expand (initially collapsed).
khora eval "$SID" "(() => { const btn = Array.from(document.querySelectorAll('button')).find(b => b.textContent.includes('Endpoints without schemas')); if (btn) { btn.click(); return true; } return false; })()" >/dev/null 2>&1

# Poll for at least 1 row matching HTTP method + path pattern.
rowgot=0
for _ in $(seq 1 40); do
    out=$(khora eval "$SID" "(() => { const section = Array.from(document.querySelectorAll('button')).find(b => b.textContent.includes('Endpoints without schemas')); if (!section) return 0; const container = section.parentElement; if (!container) return 0; const rows = Array.from(container.querySelectorAll('span.font-mono.text-xs')); return rows.filter(r => /^(GET|POST|PUT|PATCH|DELETE|HEAD|OPTIONS)\s+\//.test(r.textContent.trim())).length; })()" 2>/dev/null || true)
    n=$(echo "$out" | tr -d '\r\n ' | grep -oE '[0-9]+' | head -n1 || true)
    if [ -n "$n" ] && [ "$n" -ge 1 ]; then rowgot=1; break; fi
    sleep 0.1
done
check "'Endpoints without schemas' section present" bash -c "
    out=\$(khora eval '$SID' \"(() => { const btn = Array.from(document.querySelectorAll('button')).find(b => b.textContent.includes('Endpoints without schemas')); return !!btn; })()\")
    echo \"\$out\" | grep -q 'true'
"
check "'Endpoints without schemas' lists >=1 row" bash -c "[ '$rowgot' = '1' ]"
check "First virtualRoot row matches /^(GET|POST|PUT|PATCH|DELETE)\\s+\\// regex" bash -c "
    out=\$(khora eval '$SID' \"(() => { const section = Array.from(document.querySelectorAll('button')).find(b => b.textContent.includes('Endpoints without schemas')); if (!section) return false; const container = section.parentElement; if (!container) return false; const rows = Array.from(container.querySelectorAll('span.font-mono.text-xs')); if (rows.length === 0) return false; return /^(GET|POST|PUT|PATCH|DELETE|HEAD|OPTIONS)\\\\s+\\\\//.test(rows[0].textContent.trim()); })()\")
    echo \"\$out\" | grep -q 'true'
"
khora screenshot "$SID" -o "$SCREENSHOTS_DIR/06-virtualroots.png" >/dev/null 2>&1 || true

# ------------------------------------------------------------
# AC9: Orphan-free cleanup after this run. Cleanup trap handles
# the kill steps. The checks below run in the trap-driven exit
# path; assert inline now for $MIRAGE_PID alive (sanity), then
# rely on trap + external verify for full orphan-freeness.
# ------------------------------------------------------------
echo ""
echo "AC9: Sanity — mirage PID $MIRAGE_PID running; cleanup trap will tear down"
check "mirage process still alive mid-run" bash -c "kill -0 $MIRAGE_PID 2>/dev/null"

echo ""
echo "All acceptance checks complete. Cleanup trap runs on EXIT."
exit 0

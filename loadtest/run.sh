#!/usr/bin/env bash
set -euo pipefail

API_URL="${API_URL:-https://rustyclawrouter-gateway.fly.dev}"
RESULTS_DIR="/tmp/loadtest-results"
GATEWAY_APP="rustyclawrouter-gateway"
mkdir -p "$RESULTS_DIR"

# --- Helper functions ---

warmup() {
    echo "  [warmup] 10s at 5 RPS..."
    solvela --api-url "$API_URL" loadtest \
        --rps 5 --duration 10s --concurrency 10 \
        --mode dev-bypass \
        --slo-p99-ms 30000 --slo-error-rate 1.0 \
        || true
}

cooldown() {
    echo "  [cooldown] 30s pause..."
    sleep 30
}

check_memory() {
    echo "  [memory] Checking gateway RSS..."
    fly machine status -a "$GATEWAY_APP" 2>/dev/null | grep -i memory || echo "  (could not read memory)"
}

# --- Phase 1: Baseline (shared-cpu-1x) ---

echo "=== Phase 1: Baseline (shared-cpu-1x) ==="
for rps in 10 50 100 200; do
    warmup
    echo "--- Phase 1: ${rps} RPS ---"
    solvela --api-url "$API_URL" loadtest \
        --rps "$rps" \
        --duration 60s \
        --concurrency "$((rps * 2))" \
        --mode dev-bypass \
        --slo-p99-ms 10000 \
        --slo-error-rate 0.50 \
        --report-json "$RESULTS_DIR/phase1-${rps}rps.json" \
        || true
    check_memory
    cooldown
done

# --- Phase 2: Break-point ---
# REVIEW FIX #2: set +e so SLO failure exit code doesn't abort the loop

echo "=== Phase 2: Break-point ==="
rps=200
set +e
while true; do
    rps=$((rps + 50))
    warmup
    echo "--- Phase 2: ${rps} RPS (break-point ramp) ---"
    solvela --api-url "$API_URL" loadtest \
        --rps "$rps" \
        --duration 60s \
        --concurrency "$((rps * 2))" \
        --mode dev-bypass \
        --slo-p99-ms 10000 \
        --slo-error-rate 0.10 \
        --report-json "$RESULTS_DIR/phase2-${rps}rps.json"
    exit_code=$?

    check_memory

    if [ "$exit_code" -ne 0 ]; then
        echo "Break-point reached at ${rps} RPS (exit code: $exit_code)"
        break
    fi

    if [ "$rps" -ge 2000 ]; then
        echo "Reached 2000 RPS cap without breaking"
        break
    fi
    cooldown
done
set -e

echo ""
echo "=== Results ==="
ls -la "$RESULTS_DIR/"
echo ""
echo "Copy results with: fly ssh sftp get /tmp/loadtest-results/*.json"

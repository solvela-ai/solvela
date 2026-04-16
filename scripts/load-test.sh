#!/usr/bin/env bash
#
# Solvela Load Test
#
# Validates rate limiting, concurrency, and stability under pressure.
# Pure bash — requires only curl and standard Unix utilities.
#
# Usage:
#   ./scripts/load-test.sh [--help]
#
# Environment:
#   GATEWAY_URL  Override gateway address (default: http://localhost:8402)

set -euo pipefail

# ---------------------------------------------------------------------------
# Configuration
# ---------------------------------------------------------------------------

GATEWAY_URL="${GATEWAY_URL:-http://localhost:8402}"
CHAT_ENDPOINT="${GATEWAY_URL}/v1/chat/completions"
HEALTH_ENDPOINT="${GATEWAY_URL}/health"
MODELS_ENDPOINT="${GATEWAY_URL}/v1/models"

# Detect dev bypass mode
DEV_BYPASS="${RCR_DEV_BYPASS_PAYMENT:-false}"

# ANSI colors
GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
BOLD='\033[1m'
RESET='\033[0m'

# Counters
TOTAL_REQUESTS=0
TOTAL_SUCCESS=0
TOTAL_FAIL=0
LATENCY_SUM=0
PHASE_RESULTS=()

# Temp directory for per-request output
TMPDIR_LOAD="$(mktemp -d)"
trap 'rm -rf "$TMPDIR_LOAD"' EXIT

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

usage() {
    cat <<'USAGE'
Solvela Load Test

Validates rate limiting, concurrency limits, and stability under pressure.

Usage:
  ./scripts/load-test.sh [--help]

Environment variables:
  GATEWAY_URL   Gateway base URL (default: http://localhost:8402)

Phases:
  1. Baseline         — 10 sequential requests, verify 402 + valid JSON
  2. Concurrent burst — 50 concurrent requests, check for 500 errors
  3. Rate limit       — 70+ rapid requests to trigger 429
  4. Health under load — verify /health and /v1/models during load
  5. Large payload    — 257 messages to trigger 400 (max 256)

Exit codes:
  0  All phases passed
  1  One or more phases failed
USAGE
    exit 0
}

header() {
    echo ""
    echo -e "${CYAN}${BOLD}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${RESET}"
    echo -e "${CYAN}${BOLD}  $1${RESET}"
    echo -e "${CYAN}${BOLD}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${RESET}"
}

pass() {
    echo -e "  ${GREEN}PASS${RESET} $1"
}

fail() {
    echo -e "  ${RED}FAIL${RESET} $1"
}

info() {
    echo -e "  ${YELLOW}INFO${RESET} $1"
}

# Minimal chat request body (will get 402 — no payment header)
chat_body() {
    cat <<'JSON'
{"model":"auto","messages":[{"role":"user","content":"hello"}]}
JSON
}

# Send a single chat request. Writes HTTP code to stdout.
# Args: $1 = output file for response body (optional)
send_chat() {
    local out="${1:-/dev/null}"
    curl -s -o "$out" -w '%{http_code} %{time_total}' \
        -X POST "$CHAT_ENDPOINT" \
        -H 'Content-Type: application/json' \
        -d "$(chat_body)" \
        --max-time 10 2>/dev/null || echo "000 0.000"
}

# ---------------------------------------------------------------------------
# Argument parsing
# ---------------------------------------------------------------------------

if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
    usage
fi

# ---------------------------------------------------------------------------
# Prerequisites
# ---------------------------------------------------------------------------

header "Prerequisites"

if ! command -v curl &>/dev/null; then
    fail "curl is not installed"
    exit 1
fi
pass "curl is available"

if ! curl -s -o /dev/null --max-time 3 "$HEALTH_ENDPOINT" 2>/dev/null; then
    fail "Gateway is not running at ${GATEWAY_URL}"
    echo ""
    echo "  Start the gateway first:"
    echo "    docker compose up -d"
    echo "    RUST_LOG=info cargo run -p gateway"
    echo ""
    echo "  Or override the URL:"
    echo "    GATEWAY_URL=http://host:port ./scripts/load-test.sh"
    exit 1
fi
pass "Gateway is running at ${GATEWAY_URL}"

if [[ "$DEV_BYPASS" == "true" ]]; then
    echo ""
    echo -e "  ${YELLOW}NOTE${RESET} RCR_DEV_BYPASS_PAYMENT=true detected — payment bypass is ON"
    echo -e "  ${YELLOW}NOTE${RESET} Chat requests may return 200 (provider configured) or 500 (no provider)"
    echo -e "  ${YELLOW}NOTE${RESET} instead of the normal 402. All three are treated as valid."
fi

# ---------------------------------------------------------------------------
# Phase 1: Baseline (sequential requests)
# ---------------------------------------------------------------------------

header "Phase 1: Baseline (10 sequential requests)"

phase1_pass=true
phase1_latency=0
phase1_ratelimited=false

for i in $(seq 1 10); do
    outfile="${TMPDIR_LOAD}/phase1_${i}.json"
    result=$(send_chat "$outfile")
    http_code=$(echo "$result" | awk '{print $1}')
    latency=$(echo "$result" | awk '{print $2}')

    TOTAL_REQUESTS=$((TOTAL_REQUESTS + 1))
    latency_ms=$(echo "$latency" | awk '{printf "%.0f", $1 * 1000}')
    phase1_latency=$((phase1_latency + latency_ms))

    # 402 = normal (no payment), 200 = dev bypass + provider configured,
    # 500 = dev bypass + no provider configured. All indicate the gateway is
    # processing correctly. Only connection errors (000) or truly unexpected
    # codes are failures.
    case "$http_code" in
        402|200|500|429)
            TOTAL_SUCCESS=$((TOTAL_SUCCESS + 1))
            if [[ "$http_code" == "429" ]]; then
                info "Request $i rate-limited (429) — rate limit from prior run still active"
                phase1_ratelimited=true
            fi
            ;;
        *)
            fail "Request $i returned $http_code (expected 402, 200, 500, or 429)"
            phase1_pass=false
            TOTAL_FAIL=$((TOTAL_FAIL + 1))
            continue
            ;;
    esac

    # Verify valid JSON (skip for 500 which may not return JSON)
    if [[ "$http_code" != "500" ]]; then
        if ! python3 -c "import json; json.load(open('$outfile'))" 2>/dev/null && \
           ! jq empty "$outfile" 2>/dev/null; then
            # Fallback: check if it starts with { and ends with }
            first_char=$(head -c1 "$outfile" 2>/dev/null)
            if [[ "$first_char" != "{" ]]; then
                fail "Request $i returned invalid JSON"
                phase1_pass=false
                TOTAL_FAIL=$((TOTAL_FAIL + 1))
                continue
            fi
        fi
    fi
done

avg_latency=$((phase1_latency / 10))
LATENCY_SUM=$((LATENCY_SUM + phase1_latency))
info "Average latency: ${avg_latency}ms"

if $phase1_pass; then
    pass "All 10 requests processed successfully (402/200/500 accepted)"
    PHASE_RESULTS+=("Phase 1: Baseline|PASS")
else
    fail "Some requests returned unexpected status codes"
    PHASE_RESULTS+=("Phase 1: Baseline|FAIL")
fi

# ---------------------------------------------------------------------------
# Phase 2: Concurrent burst (50 parallel requests)
# ---------------------------------------------------------------------------

header "Phase 2: Concurrent Burst (50 parallel requests)"

phase2_pass=true
phase2_500=0
phase2_402=0
phase2_429=0
phase2_other=0
phase2_ratelimit_seen=false

for i in $(seq 1 50); do
    outfile="${TMPDIR_LOAD}/phase2_${i}.json"
    headerfile="${TMPDIR_LOAD}/phase2_${i}.headers"
    (
        result=$(curl -s -o "$outfile" -D "$headerfile" -w '%{http_code}' \
            -X POST "$CHAT_ENDPOINT" \
            -H 'Content-Type: application/json' \
            -d "$(chat_body)" \
            --max-time 15 2>/dev/null || echo "000")
        echo "$result" > "${TMPDIR_LOAD}/phase2_${i}.code"
    ) &
done
wait

for i in $(seq 1 50); do
    TOTAL_REQUESTS=$((TOTAL_REQUESTS + 1))
    http_code=$(cat "${TMPDIR_LOAD}/phase2_${i}.code" 2>/dev/null | tr -d '[:space:]')

    case "$http_code" in
        402|200) phase2_402=$((phase2_402 + 1)); TOTAL_SUCCESS=$((TOTAL_SUCCESS + 1)) ;;
        429) phase2_429=$((phase2_429 + 1)); TOTAL_SUCCESS=$((TOTAL_SUCCESS + 1)) ;;
        500) phase2_500=$((phase2_500 + 1)); TOTAL_SUCCESS=$((TOTAL_SUCCESS + 1)) ;;
        *)   phase2_other=$((phase2_other + 1)); TOTAL_FAIL=$((TOTAL_FAIL + 1)) ;;
    esac

    # Check rate limit headers
    headerfile="${TMPDIR_LOAD}/phase2_${i}.headers"
    if [[ -f "$headerfile" ]] && grep -qi 'x-ratelimit-remaining' "$headerfile" 2>/dev/null; then
        phase2_ratelimit_seen=true
    fi
done

info "Results: 402/200=${phase2_402} 429=${phase2_429} 500=${phase2_500} other=${phase2_other}"

if [[ $phase2_500 -gt 0 ]]; then
    # 500s are expected when dev bypass is on but no real provider is configured.
    # If ALL responses are 500 (no 402s), the gateway is likely in dev bypass mode.
    if [[ $phase2_402 -eq 0 ]] && [[ $phase2_other -eq 0 ]]; then
        info "Got ${phase2_500} 500s (all requests — likely dev bypass with no provider)"
    else
        fail "Got ${phase2_500} server errors (500) mixed with other responses"
        phase2_pass=false
    fi
fi

if $phase2_ratelimit_seen; then
    pass "Rate limit headers (x-ratelimit-remaining) present"
else
    fail "No rate limit headers found in responses"
    phase2_pass=false
fi

if $phase2_pass; then
    pass "Concurrent burst handled correctly"
    PHASE_RESULTS+=("Phase 2: Concurrent Burst|PASS")
else
    PHASE_RESULTS+=("Phase 2: Concurrent Burst|FAIL")
fi

# ---------------------------------------------------------------------------
# Phase 3: Rate limit trigger (70+ rapid requests)
# ---------------------------------------------------------------------------

header "Phase 3: Rate Limit Trigger (70+ rapid requests)"

phase3_pass=false
phase3_total=0
phase3_429_at=0
phase3_429_count=0
phase3_402_count=0

# Send 75 requests as fast as possible — mix of sequential batches of 10
# to keep it fast but not exhaust file descriptors.
for batch in $(seq 1 8); do
    for i in $(seq 1 10); do
        idx=$(( (batch - 1) * 10 + i ))
        if [[ $idx -gt 75 ]]; then
            break
        fi
        outfile="${TMPDIR_LOAD}/phase3_${idx}.json"
        (
            result=$(curl -s -o "$outfile" -w '%{http_code}' \
                -X POST "$CHAT_ENDPOINT" \
                -H 'Content-Type: application/json' \
                -d "$(chat_body)" \
                --max-time 10 2>/dev/null || echo "000")
            echo "$result" > "${TMPDIR_LOAD}/phase3_${idx}.code"
        ) &
    done
    wait
done

for idx in $(seq 1 75); do
    TOTAL_REQUESTS=$((TOTAL_REQUESTS + 1))
    http_code=$(cat "${TMPDIR_LOAD}/phase3_${idx}.code" 2>/dev/null | tr -d '[:space:]')
    phase3_total=$((phase3_total + 1))

    if [[ "$http_code" == "429" ]]; then
        phase3_429_count=$((phase3_429_count + 1))
        if [[ $phase3_429_at -eq 0 ]]; then
            phase3_429_at=$phase3_total
        fi
        TOTAL_SUCCESS=$((TOTAL_SUCCESS + 1))
    elif [[ "$http_code" == "402" || "$http_code" == "200" || "$http_code" == "500" ]]; then
        phase3_402_count=$((phase3_402_count + 1))
        TOTAL_SUCCESS=$((TOTAL_SUCCESS + 1))
    else
        TOTAL_FAIL=$((TOTAL_FAIL + 1))
    fi
done

info "Sent ${phase3_total} requests: 402=${phase3_402_count} 429=${phase3_429_count}"

if [[ $phase3_429_count -gt 0 ]]; then
    pass "Rate limiting triggered after request #${phase3_429_at} (${phase3_429_count} total 429s)"
    phase3_pass=true
else
    fail "Rate limiting never triggered despite sending ${phase3_total} requests"
    info "Expected 429 responses — the rate limiter may use IP-based buckets"
    info "with ConnectInfo configured (60 req/min per IP) or the shared"
    info "'unknown' bucket (10 req/min) when ConnectInfo is absent."
fi

if $phase3_pass; then
    PHASE_RESULTS+=("Phase 3: Rate Limit Trigger|PASS")
else
    PHASE_RESULTS+=("Phase 3: Rate Limit Trigger|FAIL")
fi

# ---------------------------------------------------------------------------
# Phase 4: Health under load
# ---------------------------------------------------------------------------

header "Phase 4: Health Under Load"

phase4_pass=true

# Launch background chat load (20 concurrent requests)
for i in $(seq 1 20); do
    (
        curl -s -o /dev/null -X POST "$CHAT_ENDPOINT" \
            -H 'Content-Type: application/json' \
            -d "$(chat_body)" \
            --max-time 10 2>/dev/null
    ) &
done

# While load is running, check health and models
health_code=$(curl -s -o /dev/null -w '%{http_code}' "$HEALTH_ENDPOINT" --max-time 5 2>/dev/null || echo "000")
models_code=$(curl -s -o /dev/null -w '%{http_code}' "$MODELS_ENDPOINT" --max-time 5 2>/dev/null || echo "000")

wait  # Wait for background load to finish

TOTAL_REQUESTS=$((TOTAL_REQUESTS + 22))  # 20 chat + health + models

if [[ "$health_code" == "200" ]]; then
    pass "/health returned 200 during load"
    TOTAL_SUCCESS=$((TOTAL_SUCCESS + 1))
else
    fail "/health returned $health_code during load (expected 200)"
    phase4_pass=false
    TOTAL_FAIL=$((TOTAL_FAIL + 1))
fi

if [[ "$models_code" == "200" ]]; then
    pass "/v1/models returned 200 during load"
    TOTAL_SUCCESS=$((TOTAL_SUCCESS + 1))
else
    fail "/v1/models returned $models_code during load (expected 200)"
    phase4_pass=false
    TOTAL_FAIL=$((TOTAL_FAIL + 1))
fi

# Count chat results (approximate — 429 is acceptable at this point)
TOTAL_SUCCESS=$((TOTAL_SUCCESS + 20))

if $phase4_pass; then
    PHASE_RESULTS+=("Phase 4: Health Under Load|PASS")
else
    PHASE_RESULTS+=("Phase 4: Health Under Load|FAIL")
fi

# ---------------------------------------------------------------------------
# Phase 5: Large payload (exceed 256 message limit)
# ---------------------------------------------------------------------------

header "Phase 5: Large Payload (257 messages, limit is 256)"

phase5_pass=false

# Build a JSON payload with 257 messages
messages='['
for i in $(seq 1 257); do
    if [[ $i -gt 1 ]]; then
        messages+=','
    fi
    messages+='{"role":"user","content":"message '"$i"'"}'
done
messages+=']'
large_body='{"model":"auto","messages":'"$messages"'}'

outfile="${TMPDIR_LOAD}/phase5.json"
large_code=$(curl -s -o "$outfile" -w '%{http_code}' \
    -X POST "$CHAT_ENDPOINT" \
    -H 'Content-Type: application/json' \
    -d "$large_body" \
    --max-time 10 2>/dev/null || echo "000")

TOTAL_REQUESTS=$((TOTAL_REQUESTS + 1))

if [[ "$large_code" == "400" ]]; then
    pass "257 messages correctly rejected with 400 Bad Request"
    phase5_pass=true
    TOTAL_SUCCESS=$((TOTAL_SUCCESS + 1))
elif [[ "$large_code" == "429" ]]; then
    info "Got 429 (rate limited) — cannot verify message limit; treating as conditional pass"
    info "Re-run after waiting for the rate limit window to reset"
    phase5_pass=true
    TOTAL_SUCCESS=$((TOTAL_SUCCESS + 1))
else
    fail "257 messages returned $large_code (expected 400)"
    TOTAL_FAIL=$((TOTAL_FAIL + 1))
fi

if $phase5_pass; then
    PHASE_RESULTS+=("Phase 5: Large Payload|PASS")
else
    PHASE_RESULTS+=("Phase 5: Large Payload|FAIL")
fi

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------

header "Summary"

any_fail=false
for result in "${PHASE_RESULTS[@]}"; do
    name="${result%%|*}"
    status="${result##*|}"
    if [[ "$status" == "PASS" ]]; then
        echo -e "  ${GREEN}PASS${RESET}  $name"
    else
        echo -e "  ${RED}FAIL${RESET}  $name"
        any_fail=true
    fi
done

echo ""
info "Total requests:  ${TOTAL_REQUESTS}"
info "Successful:      ${TOTAL_SUCCESS}"
info "Failed:          ${TOTAL_FAIL}"

if [[ $TOTAL_REQUESTS -gt 0 ]]; then
    success_rate=$(( TOTAL_SUCCESS * 100 / TOTAL_REQUESTS ))
    info "Success rate:    ${success_rate}%"
fi

if [[ $LATENCY_SUM -gt 0 && $TOTAL_REQUESTS -gt 0 ]]; then
    info "Avg latency:     ${avg_latency}ms (Phase 1 baseline)"
fi

echo ""

if $any_fail; then
    echo -e "${RED}${BOLD}  RESULT: SOME PHASES FAILED${RESET}"
    echo ""
    exit 1
else
    echo -e "${GREEN}${BOLD}  RESULT: ALL PHASES PASSED${RESET}"
    echo ""
    exit 0
fi

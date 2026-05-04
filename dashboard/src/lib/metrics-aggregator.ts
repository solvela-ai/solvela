// Public-metrics aggregator.
//
// This module turns gateway responses into a strictly public-safe summary
// that can be served at /metrics (and /metrics/data.json) without any auth.
//
// Design rules:
//   1. Pure function. No fetching, no env reads. Inputs in, summary out.
//      Keeps the page testable and keeps data leakage decisions explicit.
//   2. Aggregates only. We never expose per-wallet numbers, top-N wallets,
//      individual day-by-day rows, or anything that ties activity to a
//      specific identity. Grant evaluators and acquirers want totals.
//   3. Graceful empty state. If a source is missing we return null for that
//      field, not zero — zero is a fact ("we routed $0 today"), null is a
//      "we don't know yet" signal the UI can render distinctly.
//   4. No mutation. Inputs are read-only; outputs are fresh objects.
//
// If a new public number is added here it should also be:
//   - Documented in the table at the top of /metrics page.tsx
//   - Linked from docs/grants/README.md (acquirer-facing evidence)
//   - Exposed verbatim through /metrics/data.json so external pages can embed.

import type {
  AdminStatsResponse,
  HealthResponse,
  PricingResponse,
  EscrowConfig,
} from '@/types'

// ─── Inputs ────────────────────────────────────────────────────────────────

export interface MetricsInputs {
  /** Public health endpoint result, or null if unreachable. */
  health: HealthResponse | null
  /** Public pricing endpoint result, or null if unreachable. */
  pricing: PricingResponse | null
  /** Public escrow config endpoint result, or null if unreachable. */
  escrow: EscrowConfig | null
  /**
   * Admin stats result, or null if either the admin key is unavailable or
   * the gateway is offline. Public page never reveals per-wallet rows from
   * this; only summary aggregates.
   */
  adminStats: AdminStatsResponse | null
}

// ─── Output ────────────────────────────────────────────────────────────────

export type GatewayLiveness = 'live' | 'degraded' | 'down' | 'unknown'

export interface MetricsSummary {
  /** Liveness derived from health endpoint reachability + status field. */
  liveness: GatewayLiveness
  /** Gateway version string from /health. Null if /health failed. */
  version: string | null

  /** Cumulative totals across the period sampled by adminStats. */
  totals: {
    /** Period covered by the totals, in days. Null if adminStats absent. */
    periodDays: number | null
    /** Total requests routed during the period. Null if unknown. */
    requests: number | null
    /** Total USDC volume settled during the period. Null if unknown. */
    usdcVolume: number | null
    /** Distinct wallets paying through the gateway during the period. */
    uniqueWallets: number | null
  }

  /** Catalog facts that are public regardless of gateway uptime. */
  catalog: {
    /** Models active in the routing pool. Null if /pricing failed. */
    activeModels: number | null
    /**
     * Distinct providers represented in the active model set.
     * Computed from /pricing because a provider might appear in the
     * health-payload list but currently have zero models routed to it.
     */
    activeProviders: number | null
    /** Platform fee percent (e.g. 5.0). Null if /pricing failed. */
    platformFeePercent: number | null
  }

  /** On-chain proof artifacts. */
  onChain: {
    /** Solana program id of the deployed escrow PDA. */
    escrowProgramId: string | null
    /** Network the escrow program is currently anchored to. */
    network: string | null
    /** USDC mint configured on the gateway. */
    usdcMint: string | null
  }

  /**
   * ISO-8601 timestamp of when this aggregation was produced.
   * Useful for grant pages embedding the number — they can show staleness.
   */
  generatedAt: string
}

// ─── Aggregation ───────────────────────────────────────────────────────────

const VOLUME_DECIMALS_GUARD = 1_000_000_000 // sanity ceiling for parsed USDC

/**
 * Parse a USDC string from the gateway. The gateway emits decimal strings
 * like "12.345678" with full SPL precision. We coerce to number for display
 * while guarding against absurd values that would imply a bug upstream.
 */
function parseUsdcString(raw: string | undefined): number | null {
  if (raw === undefined || raw === null) return null
  const n = Number(raw)
  if (!Number.isFinite(n)) return null
  if (n < 0) return null
  if (n > VOLUME_DECIMALS_GUARD) return null
  return n
}

function deriveLiveness(health: HealthResponse | null): GatewayLiveness {
  if (!health) return 'unknown'
  if (health.status === 'ok') return 'live'
  if (health.status === 'degraded') return 'degraded'
  if (health.status === 'down') return 'down'
  return 'unknown'
}

/**
 * Aggregate raw inputs into the public-safe summary shape.
 * Pure: returns a new object on every call; never mutates inputs.
 */
export function aggregateMetrics(
  inputs: MetricsInputs,
  now: Date = new Date(),
): MetricsSummary {
  const { health, pricing, escrow, adminStats } = inputs

  const periodDays = adminStats?.period_days ?? null
  const requests = adminStats?.summary.total_requests ?? null
  const usdcVolume = parseUsdcString(adminStats?.summary.total_cost_usdc)
  const uniqueWallets = adminStats?.summary.unique_wallets ?? null

  const activeModels = pricing?.models.length ?? null
  const activeProviders = pricing
    ? new Set(pricing.models.map((m) => m.provider)).size
    : null
  const platformFeePercent = pricing?.platform.fee_percent ?? null

  return {
    liveness: deriveLiveness(health),
    version: health?.version ?? null,
    totals: {
      periodDays,
      requests,
      usdcVolume,
      uniqueWallets,
    },
    catalog: {
      activeModels,
      activeProviders,
      platformFeePercent,
    },
    onChain: {
      escrowProgramId: escrow?.escrow_program_id ?? null,
      network: escrow?.network ?? null,
      usdcMint: escrow?.usdc_mint ?? null,
    },
    generatedAt: now.toISOString(),
  }
}

import { describe, it, expect } from 'vitest'
import { aggregateMetrics } from '@/lib/metrics-aggregator'
import type {
  AdminStatsResponse,
  EscrowConfig,
  HealthResponse,
  PricingResponse,
} from '@/types'

const FIXED_NOW = new Date('2026-05-04T14:21:00Z')

const fullHealth: HealthResponse = { status: 'ok', version: '0.5.0' }
const degradedHealth: HealthResponse = { status: 'degraded', version: '0.5.0' }
const downHealth: HealthResponse = { status: 'down', version: '0.5.0' }

const fullPricing: PricingResponse = {
  platform: {
    name: 'Solvela',
    chain: 'solana',
    token: 'USDC',
    usdc_mint: 'EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v',
    fee_percent: 5,
    settlement: 'escrow',
  },
  models: [
    {
      id: 'gpt-4o',
      display_name: 'GPT-4o',
      provider: 'openai',
      pricing: {
        input_per_million_usdc: 2.5,
        output_per_million_usdc: 10,
        platform_fee_percent: 5,
        currency: 'USDC',
      },
      capabilities: { streaming: true, tools: true, vision: true, reasoning: false, context_window: 128_000 },
      example_1k_token_request: {
        input_tokens: 1000,
        output_tokens: 1000,
        provider_cost_usdc: '0.0125',
        platform_fee_usdc: '0.000625',
        total_usdc: '0.013125',
      },
    },
    {
      id: 'claude-sonnet-4',
      display_name: 'Claude Sonnet 4',
      provider: 'anthropic',
      pricing: {
        input_per_million_usdc: 3,
        output_per_million_usdc: 15,
        platform_fee_percent: 5,
        currency: 'USDC',
      },
      capabilities: { streaming: true, tools: true, vision: true, reasoning: true, context_window: 200_000 },
      example_1k_token_request: {
        input_tokens: 1000,
        output_tokens: 1000,
        provider_cost_usdc: '0.018',
        platform_fee_usdc: '0.0009',
        total_usdc: '0.0189',
      },
    },
    {
      id: 'gpt-4o-mini',
      display_name: 'GPT-4o mini',
      provider: 'openai',
      pricing: {
        input_per_million_usdc: 0.15,
        output_per_million_usdc: 0.6,
        platform_fee_percent: 5,
        currency: 'USDC',
      },
      capabilities: { streaming: true, tools: true, vision: false, reasoning: false, context_window: 128_000 },
      example_1k_token_request: {
        input_tokens: 1000,
        output_tokens: 1000,
        provider_cost_usdc: '0.00075',
        platform_fee_usdc: '0.0000375',
        total_usdc: '0.0007875',
      },
    },
  ],
}

const fullEscrow: EscrowConfig = {
  escrow_program_id: '9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU',
  current_slot: 312_000_000,
  network: 'mainnet',
  usdc_mint: 'EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v',
  provider_wallet: 'PROVIDER_WALLET_PUBKEY',
}

const fullAdminStats: AdminStatsResponse = {
  period_days: 30,
  summary: {
    total_requests: 12_345,
    total_cost_usdc: '4321.67',
    total_input_tokens: 1_000_000,
    total_output_tokens: 500_000,
    unique_wallets: 47,
    cache_hit_rate: 0.34,
  },
  by_model: [],
  by_day: [],
  top_wallets: [],
}

describe('aggregateMetrics', () => {
  it('produces a fully populated summary when all inputs are present', () => {
    const result = aggregateMetrics(
      {
        health: fullHealth,
        pricing: fullPricing,
        escrow: fullEscrow,
        adminStats: fullAdminStats,
      },
      FIXED_NOW,
    )

    expect(result.liveness).toBe('live')
    expect(result.version).toBe('0.5.0')
    expect(result.totals).toEqual({
      periodDays: 30,
      requests: 12_345,
      usdcVolume: 4321.67,
      uniqueWallets: 47,
    })
    expect(result.catalog).toEqual({
      activeModels: 3,
      activeProviders: 2,
      platformFeePercent: 5,
    })
    expect(result.onChain.escrowProgramId).toBe(
      '9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU',
    )
    expect(result.onChain.network).toBe('mainnet')
    expect(result.generatedAt).toBe('2026-05-04T14:21:00.000Z')
  })

  it('reports unknown liveness when the health probe is missing', () => {
    const result = aggregateMetrics(
      { health: null, pricing: fullPricing, escrow: fullEscrow, adminStats: fullAdminStats },
      FIXED_NOW,
    )
    expect(result.liveness).toBe('unknown')
    expect(result.version).toBeNull()
  })

  it('passes through degraded and down liveness states', () => {
    expect(
      aggregateMetrics(
        { health: degradedHealth, pricing: null, escrow: null, adminStats: null },
        FIXED_NOW,
      ).liveness,
    ).toBe('degraded')

    expect(
      aggregateMetrics(
        { health: downHealth, pricing: null, escrow: null, adminStats: null },
        FIXED_NOW,
      ).liveness,
    ).toBe('down')
  })

  it('returns null totals when adminStats is unavailable', () => {
    const result = aggregateMetrics(
      { health: fullHealth, pricing: fullPricing, escrow: fullEscrow, adminStats: null },
      FIXED_NOW,
    )
    expect(result.totals).toEqual({
      periodDays: null,
      requests: null,
      usdcVolume: null,
      uniqueWallets: null,
    })
  })

  it('returns null catalog when pricing is unavailable', () => {
    const result = aggregateMetrics(
      { health: fullHealth, pricing: null, escrow: fullEscrow, adminStats: fullAdminStats },
      FIXED_NOW,
    )
    expect(result.catalog).toEqual({
      activeModels: null,
      activeProviders: null,
      platformFeePercent: null,
    })
  })

  it('refuses garbage USDC strings rather than producing fake totals', () => {
    const stats: AdminStatsResponse = {
      ...fullAdminStats,
      summary: { ...fullAdminStats.summary, total_cost_usdc: 'NaN' },
    }
    const result = aggregateMetrics(
      { health: fullHealth, pricing: fullPricing, escrow: fullEscrow, adminStats: stats },
      FIXED_NOW,
    )
    expect(result.totals.usdcVolume).toBeNull()
  })

  it('refuses negative USDC strings', () => {
    const stats: AdminStatsResponse = {
      ...fullAdminStats,
      summary: { ...fullAdminStats.summary, total_cost_usdc: '-12.34' },
    }
    const result = aggregateMetrics(
      { health: fullHealth, pricing: fullPricing, escrow: fullEscrow, adminStats: stats },
      FIXED_NOW,
    )
    expect(result.totals.usdcVolume).toBeNull()
  })

  it('counts active providers as the unique providers in the model set', () => {
    const onlyOpenAI: PricingResponse = {
      ...fullPricing,
      models: fullPricing.models.filter((m) => m.provider === 'openai'),
    }
    const result = aggregateMetrics(
      { health: fullHealth, pricing: onlyOpenAI, escrow: fullEscrow, adminStats: fullAdminStats },
      FIXED_NOW,
    )
    expect(result.catalog.activeProviders).toBe(1)
    expect(result.catalog.activeModels).toBe(2)
  })

  it('does not mutate inputs', () => {
    const stats: AdminStatsResponse = {
      ...fullAdminStats,
      summary: { ...fullAdminStats.summary },
    }
    const before = JSON.parse(JSON.stringify(stats))
    aggregateMetrics(
      { health: fullHealth, pricing: fullPricing, escrow: fullEscrow, adminStats: stats },
      FIXED_NOW,
    )
    expect(JSON.parse(JSON.stringify(stats))).toEqual(before)
  })

  it('uses the provided clock for generatedAt rather than wall time', () => {
    const result = aggregateMetrics(
      { health: null, pricing: null, escrow: null, adminStats: null },
      new Date('2030-01-01T00:00:00Z'),
    )
    expect(result.generatedAt).toBe('2030-01-01T00:00:00.000Z')
  })
})

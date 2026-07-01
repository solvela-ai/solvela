// Landing-page constants. Update these at each release.
// Kept compile-time; no live API reads on the marketing page.

export const VERSION = 'v0.5.0'
export const GITHUB_URL = 'https://github.com/solvela-ai/solvela'
export const DOCS_URL = 'https://docs.solvela.ai'
export const APP_URL = 'https://app.solvela.ai'
export const QUICKSTART_URL = 'https://docs.solvela.ai/docs/quickstart'
export const ESCROW_PROGRAM_ID = '9neDHouXgEgHZDde5Sp'

// Uptime and p50 latency were removed pending a public status page;
// claiming SLA-shaped numbers without one is the kind of statement
// enterprise procurement will challenge. Re-add with hard data
// behind them, not before.
//
// 'paid models' = the 25 paid models across the 5 marquee providers
// below (OpenAI/Anthropic/Google/xAI/DeepSeek). The free NVIDIA NIM
// Nemotron tier (15 zero-cost models) is surfaced as its own callout,
// not folded into this figure. Full live list: GET /v1/models.
export const METRICS = [
  { label: 'paid models', value: 25, suffix: '+', decimals: 0 },
  { label: 'platform fee', value: 5, suffix: '%', decimals: 0 },
]

export const PROVIDERS = [
  { id: 'openai', name: 'OpenAI', dot: '#10a37f' },
  { id: 'anthropic', name: 'Anthropic', dot: '#d97757' },
  { id: 'google', name: 'Google', dot: '#4285f4' },
  { id: 'xai', name: 'xAI', dot: '#DEDCD1' },
  { id: 'deepseek', name: 'DeepSeek', dot: '#536dfe' },
]

export const CURL_SNIPPET = `curl https://api.solvela.ai/v1/chat/completions \\
  -H "Content-Type: application/json" \\
  -H "PAYMENT-SIGNATURE: <base64-signed-tx>" \\
  -d '{ "model": "auto", "messages": [{"role":"user","content":"hello"}] }'`

// Landing-page constants. Update these at each release.
// Kept compile-time; no live API reads on the marketing page.

export const VERSION = 'v0.5.0'
export const GITHUB_URL = 'https://github.com/sky64/Solvela'
export const DOCS_URL = 'https://docs.solvela.ai'
export const APP_URL = 'https://app.solvela.ai'
export const QUICKSTART_URL = 'https://docs.solvela.ai/docs/quickstart'
export const ESCROW_PROGRAM_ID = '9neDHouXgEgHZDde5Sp'

export const METRICS = [
  { label: 'uptime', value: 99.98, suffix: '%', decimals: 2 },
  { label: 'p50 latency', value: 38, suffix: 'ms', decimals: 0 },
  { label: 'models', value: 26, suffix: '+', decimals: 0 },
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

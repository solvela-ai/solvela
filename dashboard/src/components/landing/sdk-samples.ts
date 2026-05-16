// SAFETY: `SAMPLES` below feeds `app/page.tsx`'s server-side Shiki pipeline,
// whose HTML output is rendered via `dangerouslySetInnerHTML` in
// `./sdk-cta-panel.tsx`. The injection is only safe because every `code`
// string here is a build-time literal in this file. Do NOT source `code`
// (or any other field) from runtime input — CMS, query params, fetch
// responses, user submissions, etc. — without first routing through a
// sanitizer. See issue #173 (L3 SDK).

import type { SupportedLang } from '@/lib/shiki/highlighter'

export type SampleStatus = 'live' | 'alpha' | 'soon'

export interface Sample {
  id: string
  label: string
  lang: SupportedLang
  install: string
  status?: SampleStatus
  code: string
}

export const SAMPLES: Sample[] = [
  {
    id: 'ts',
    label: 'typescript',
    lang: 'typescript',
    install: 'npm i @solvela/sdk  # republish pending — use curl today',
    status: 'soon',
    code: `// @solvela/sdk@0.2.x predates the current wire format and will
// not complete a real payment against api.solvela.ai. Republish is
// pending. Drive the gateway directly until the new SDK ships:

const res = await fetch('https://api.solvela.ai/v1/chat/completions', {
  method: 'POST',
  headers: { 'content-type': 'application/json' },
  body: JSON.stringify({
    model: 'auto',
    messages: [{ role: 'user', content: 'hi' }],
  }),
})
// 402 first, then sign + retry with the payment-signature header.
// Full handshake: docs.solvela.ai/quickstart`,
  },
  {
    id: 'vercel',
    label: 'vercel ai sdk',
    lang: 'typescript',
    install: 'npm i @solvela/ai-sdk-provider ai',
    status: 'alpha',
    code: `import { createSolvela } from '@solvela/ai-sdk-provider'
import { createLocalWalletAdapter } from '@solvela/ai-sdk-provider/adapters/local'
import { generateText } from 'ai'

const solvela = createSolvela({
  wallet: createLocalWalletAdapter(keypair),
})

const { text } = await generateText({
  model: solvela('auto'),
  prompt: 'summarize this transcript in one sentence.',
})`,
  },
  {
    id: 'py',
    label: 'python',
    lang: 'python',
    install: 'pip install solvela-sdk',
    status: 'live',
    code: `from solvela import Solvela

solvela = Solvela(keypair=wallet)

reply = solvela.chat.completions.create(
    model="auto",
    messages=[{"role": "user", "content": "hi"}],
)`,
  },
  {
    id: 'go',
    label: 'go',
    lang: 'go',
    install: 'go get github.com/solvela-ai/solvela/sdks/go',
    status: 'live',
    code: `client := solvela.New(solvela.WithKeypair(wallet))

reply, err := client.Chat.Completions.Create(ctx, solvela.ChatRequest{
    Model:    "auto",
    Messages: []solvela.Message{{Role: "user", Content: "hi"}},
})`,
  },
  {
    id: 'langchain',
    label: 'langchain',
    lang: 'typescript',
    install: 'npm i @solvela/langchain',
    status: 'soon',
    code: `import { ChatSolvela } from '@solvela/langchain'
import { createLocalWalletAdapter } from '@solvela/ai-sdk-provider/adapters/local'

const model = new ChatSolvela({
  wallet: createLocalWalletAdapter(keypair),
  model: 'auto',
})

const reply = await model.invoke('summarize this transcript in one sentence.')`,
  },
  {
    id: 'rust',
    label: 'rust cli',
    lang: 'bash',
    install: 'cargo install solvela-cli',
    status: 'live',
    code: `$ solvela chat --model auto "hi"
→ 402 · 0.0042 usdc  (fee 0.0002)
  sign? [y/N] y
← hello — i can help with that.`,
  },
  {
    id: 'mcp',
    label: 'mcp',
    lang: 'json',
    install: 'git clone solvela-ai/solvela && cd sdks/mcp && npm i && npm run build',
    status: 'alpha',
    code: `// works in claude code, cursor, and openclaw — drop into .mcp.json
// npm publish pending; point at your local build of dist/index.js
{
  "mcpServers": {
    "solvela": {
      "command": "node",
      "args": ["<repo>/sdks/mcp/dist/index.js"],
      "env": { "SOLANA_WALLET_KEY": "<base58-keypair-secret>" }
    }
  }
}`,
  },
]

export const WORKS_WITH: { label: string; status: SampleStatus }[] = [
  { label: 'vercel ai sdk', status: 'alpha' },
  { label: 'langchain', status: 'soon' },
  { label: 'openai-compat', status: 'live' },
  { label: 'mcp', status: 'live' },
  { label: 'a2a / agent-card', status: 'live' },
]

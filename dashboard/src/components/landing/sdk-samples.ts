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
    install: 'npm install @solvela/sdk',
    status: 'live',
    code: `import { SolvelaClient, ChatRequest, ChatMessage } from '@solvela/sdk'

const client = new SolvelaClient({
  config: { gatewayUrl: 'https://api.solvela.ai' },
})

// SDK reads SOLANA_PRIVATE_KEY from env by default — or pass an explicit
// wallet + signer in the constructor options.
const response = await client.chat(
  new ChatRequest('auto', [new ChatMessage('user', 'hi')]),
)
console.log(response.choices[0].message.content)`,
  },
  {
    id: 'vercel',
    label: 'vercel ai sdk',
    lang: 'typescript',
    install: 'git clone solvela-ai/solvela && cd sdks/ai-sdk-provider && npm i && npm run build',
    status: 'soon',
    code: `// @solvela/ai-sdk-provider npm publish pending — build from source
// for now (see install command above).

import { createSolvelaProvider } from '@solvela/ai-sdk-provider'
import { createLocalWalletAdapter } from '@solvela/ai-sdk-provider/adapters/local'
import { generateText } from 'ai'

const solvela = createSolvelaProvider({
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
    code: `import asyncio
from solvela import SolvelaClient, Wallet, KeypairSigner
from solvela.types import ChatRequest, ChatMessage, Role

async def main():
    wallet = Wallet.from_env('SOLANA_PRIVATE_KEY')
    signer = KeypairSigner(wallet=wallet)
    client = SolvelaClient(wallet=wallet, signer=signer)

    response = await client.chat(ChatRequest(
        model='auto',
        messages=[ChatMessage(role=Role.USER, content='hi')],
    ))
    print(response.choices[0].message.content)

asyncio.run(main())`,
  },
  {
    id: 'go',
    label: 'go',
    lang: 'go',
    install: 'go get github.com/solvela-ai/solvela/sdks/go',
    status: 'live',
    code: `wallet, _ := solvela.WalletFromEnv("SOLANA_PRIVATE_KEY")

// Wire your own Signer in production; the SDK ships an UnimplementedSigner
// stub so unsigned (preview) calls return *PaymentRequiredError.
client, _ := solvela.NewClient(wallet, signer,
    solvela.WithGatewayURL("https://api.solvela.ai"),
)
defer client.Close()

resp, _ := client.Chat(ctx, &solvela.ChatRequest{
    Model:    "auto",
    Messages: []solvela.ChatMessage{{Role: solvela.RoleUser, Content: "hi"}},
})`,
  },
  {
    id: 'langchain',
    label: 'langchain',
    lang: 'typescript',
    install: '# @solvela/langchain not yet published — design sketch below',
    status: 'soon',
    code: `// Forward-looking sketch: a thin LangChain wrapper over the AI SDK
// provider, surfaced under '@solvela/langchain' once that package ships.
// Tracker: docs.solvela.ai/sdks

import { ChatSolvela } from '@solvela/langchain'
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

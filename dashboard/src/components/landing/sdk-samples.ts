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
    install: 'npm i @solvela/sdk',
    status: 'live',
    code: `import { Solvela } from '@solvela/sdk'

const solvela = new Solvela({ keypair: wallet })

const reply = await solvela.chat.completions.create({
  model: 'auto',
  messages: [{ role: 'user', content: 'hi' }],
})

// 402 handshake + escrow claim handled for you.`,
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
    install: 'pip install solvela',
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
    install: 'go get github.com/solvela-ai/solvela-go',
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
    install: 'npx @solvela/mcp',
    status: 'live',
    code: `// works in claude code, cursor, and openclaw — drop into .mcp.json
{
  "mcpServers": {
    "solvela": {
      "command": "npx",
      "args": ["@solvela/mcp"],
      "env": { "SOLVELA_WALLET": "<keypair.json>" }
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

'use client'

import { useState } from 'react'
import { ArrowRight } from 'lucide-react'
import { cn } from '@/lib/utils'
import { CopyButton } from './copy-button'
import { CURL_SNIPPET, QUICKSTART_URL } from './config'

interface Sample {
  id: string
  label: string
  code: string
  install: string
}

const SAMPLES: Sample[] = [
  {
    id: 'ts',
    label: 'typescript',
    install: 'npm i @solvela/sdk',
    code: `import { Solvela } from '@solvela/sdk'

const solvela = new Solvela({ keypair: wallet })

const reply = await solvela.chat.completions.create({
  model: 'auto',
  messages: [{ role: 'user', content: 'hi' }],
})

// 402 handshake + escrow claim handled for you.`,
  },
  {
    id: 'py',
    label: 'python',
    install: 'pip install solvela',
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
    install: 'go get github.com/solvela/sdk-go',
    code: `client := solvela.New(solvela.WithKeypair(wallet))

reply, err := client.Chat.Completions.Create(ctx, solvela.ChatRequest{
    Model:    "auto",
    Messages: []solvela.Message{{Role: "user", Content: "hi"}},
})`,
  },
  {
    id: 'rust',
    label: 'rust cli',
    install: 'cargo install solvela-cli',
    code: `$ solvela chat --model auto "hi"
→ 402 · 0.0042 usdc  (fee 0.0002)
  sign? [y/N] y
← hello — i can help with that.`,
  },
  {
    id: 'mcp',
    label: 'mcp',
    install: 'npx @solvela/mcp',
    code: `{
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

export function SdkCtaPanel() {
  const [activeId, setActiveId] = useState<string>('ts')
  const active = SAMPLES.find((s) => s.id === activeId) ?? SAMPLES[0]

  return (
    <section className="border-t border-border/60 bg-[var(--popover)]">
      <div className="mx-auto max-w-[1280px] px-6 py-16 lg:py-24">
        <div className="flex flex-col gap-10">
          {/* headline row */}
          <div className="flex flex-col gap-3">
            <span className="eyebrow">your first 402</span>
            <h2
              className="font-display leading-[1.0] tracking-[-0.02em] text-foreground"
              style={{ fontSize: 'clamp(2rem, 4vw, 3.25rem)', fontWeight: 600 }}
            >
              A wallet is your API key.
              <br />
              <span className="text-muted-foreground">
                Sign, send, receive. That&apos;s it.
              </span>
            </h2>
          </div>

          {/* big terminal card */}
          <div className="terminal-card">
            <div className="terminal-card-titlebar">
              <span className="terminal-card-dots" aria-hidden>
                <span className="terminal-card-dot" />
                <span className="terminal-card-dot" />
                <span className="terminal-card-dot terminal-card-dot--accent" />
              </span>
              <span>solvela · quickstart</span>
              <span className="ml-auto hidden font-mono text-[10px] uppercase tracking-[0.2em] text-text-faint sm:inline">
                {active.install}
              </span>
            </div>

            {/* tabs */}
            <div className="flex overflow-x-auto border-b border-border/60">
              {SAMPLES.map((s) => (
                <button
                  key={s.id}
                  onClick={() => setActiveId(s.id)}
                  className={cn(
                    'relative px-4 py-3 font-mono text-[11px] uppercase tracking-[0.18em] transition-colors',
                    s.id === activeId
                      ? 'text-foreground'
                      : 'text-text-faint hover:text-muted-foreground',
                  )}
                >
                  {s.label}
                  {s.id === activeId && (
                    <span
                      aria-hidden
                      className="absolute bottom-0 left-3 right-3 h-[2px] bg-[var(--accent-salmon)]"
                    />
                  )}
                </button>
              ))}
            </div>

            {/* code body */}
            <div className="terminal-card-screen !p-0">
              <div className="flex items-center justify-between border-b border-border/60 px-5 py-2 font-mono text-[11px] text-text-faint">
                <span>{active.install}</span>
                <CopyButton text={active.code} label="copy" />
              </div>
              <pre className="m-0 overflow-x-auto border-0 !bg-transparent px-5 py-6 font-mono text-[13px] leading-[1.7] text-foreground">
                <code>{active.code}</code>
              </pre>
            </div>

            {/* footer row — curl + CTA */}
            <div className="flex flex-col gap-4 border-t border-border/60 px-5 py-5 sm:flex-row sm:items-center sm:justify-between">
              <div className="min-w-0 flex-1">
                <div className="mb-1 font-mono text-[10px] uppercase tracking-[0.18em] text-text-faint">
                  or, raw:
                </div>
                <div className="flex items-center gap-3">
                  <pre className="m-0 flex-1 overflow-x-auto border-0 !bg-transparent p-0 font-mono text-[12px] text-muted-foreground">
                    <code>curl https://api.solvela.ai/v1/chat/completions …</code>
                  </pre>
                  <CopyButton text={CURL_SNIPPET} label="copy curl" />
                </div>
              </div>
              <a
                href={QUICKSTART_URL}
                className="inline-flex items-center gap-2 self-start rounded-md bg-[var(--accent-salmon)] px-5 py-3 font-mono text-[12px] uppercase tracking-[0.16em] text-[#1F1E1D] transition-[transform,background] hover:bg-[#ff9a9a] active:translate-y-[1px] sm:self-auto"
              >
                open quickstart
                <ArrowRight className="h-3.5 w-3.5" />
              </a>
            </div>
          </div>
        </div>
      </div>
    </section>
  )
}

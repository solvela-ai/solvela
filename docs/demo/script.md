# Demo Video Script — 90 Seconds

> **Tone:** Calm, confident, evidence-first. You are showing, not selling. Read the script once cold, then once recording — do not over-rehearse.
>
> **VO style notes:** Drop pitch slightly at the end of declarative sentences. Pause one beat after each numbered demonstration. Do not laugh, do not apologize, do not say "um."
>
> **Run time target:** 90 seconds. Each section's target duration is in brackets.

---

## ACT 1 — The Hook [0:00 – 0:15]

**Scene:** Full-screen terminal. Cursor blinking. Solvela logo wordmark fades in top-left.

**Voiceover:**

> AI agents need to pay for the things they use — LLM calls, tools, data, other agents. Today they do that with API keys, monthly invoices, and trust. None of that scales to a world where agents transact every few seconds.

**On-screen text overlay (seconds 8–14):**

```
API keys → don't compose
Invoices → don't settle in real time
Trust → doesn't survive autonomy
```

---

## ACT 2 — What It Is [0:15 – 0:25]

**Scene:** Cut to README header in browser, scrolled to the architecture mermaid diagram. Hold for 4 seconds, then cut back to terminal.

**Voiceover:**

> Solvela is a Solana-native payment gateway for AI inference. Agents pay for LLM calls in USDC, on-chain, per request. No accounts. No keys. Just wallets.

---

## ACT 3 — The Live Demo [0:25 – 1:10]

This is the meat. Three demonstrations, each ~15 seconds. Type slowly enough that viewers can read, but do not narrate every keystroke.

### Demonstration 1 — The 402 [0:25 – 0:40]

**Scene:** Terminal, command typed live.

```bash
curl -s http://localhost:8402/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{"model":"auto","messages":[{"role":"user","content":"Hello"}]}' | jq
```

**Voiceover (over the curl execution):**

> A request without payment returns a 402. The response includes the cost breakdown, the recipient wallet, and the accepted payment schemes — direct transfer, or trustless escrow.

**On-screen text overlay during JSON output:**

> HTTP 402 — the original "Payment Required"

### Demonstration 2 — Pay & Get the Response [0:40 – 0:55]

**Scene:** Switch to the Python SDK or CLI. Run the `pay-and-call` example.

```bash
python -c "
from solvela import Client
c = Client(wallet='~/.solvela/wallet.json')
print(c.chat('Explain Solana in one sentence.', model='auto').content)
"
```

**Voiceover (during execution):**

> The SDK signs a USDC transfer, attaches it to the retry, and the gateway settles on-chain in roughly four hundred milliseconds. The agent never holds a credit card. The provider never sees the agent's identity.

### Demonstration 3 — Proof It Settled [0:55 – 1:10]

**Scene:** Open Solana Explorer (mainnet) in browser. Paste the txn signature returned in the SDK output. Show the USDC transfer to the gateway's recipient ATA.

**Voiceover:**

> Here is the transaction on Solana mainnet. USDC moves from the agent's wallet to the gateway. The platform fee splits to the protocol treasury. Every request is a settled, public, verifiable financial event.

---

## ACT 4 — Why It Matters [1:10 – 1:25]

**Scene:** Cut back to terminal. Pull up the mermaid architecture diagram from `docs/architecture.md`, full screen, zoomed to the smart router box.

**Voiceover:**

> The router classifies every request across fifteen dimensions in microseconds, then picks the best of OpenAI, Anthropic, Google, xAI, or DeepSeek. The escrow program is live on mainnet. The gateway sustains four hundred requests per second at sub-three-hundred millisecond p99.

**On-screen text overlay (lower third):**

```
Mainnet escrow:  9neD…HLU
Providers:       5
p99 @ 400 RPS:   <300ms
```

---

## ACT 5 — The Close [1:25 – 1:30]

**Scene:** GitHub repo page, scrolled to the badges and license table. Hold 3 seconds. Cut to a final card with the URL.

**Voiceover:**

> Open source. SDKs in Python, TypeScript, Go, and Rust. MCP server for Claude Code, Cursor, and Claude Desktop. Solvela on GitHub.

**Final card (full screen, 2 seconds):**

```
github.com/solvela-ai/solvela
```

---

## After the Cut

Once the v1 cut is exported and watchable end to end, do exactly **one** revision pass to fix:

1. Any moment where the terminal is unreadable (font too small, contrast too low, command clipped)
2. Any voiceover line that lands more than 0.5s off its scene
3. The first and last 3 seconds — these get the most replay attention and are worth the polish

Do not add music, transitions, or animations on the v1 pass. Ship the cut, then iterate if the v1 fails to convert. Most of the time it doesn't.

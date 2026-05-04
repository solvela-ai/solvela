# Demo Video — Shot List

Detailed scene-by-scene breakdown. Use this as your monitor reference while recording. Each shot lists the source (terminal, browser, editor), the exact target, and the cue.

| # | Time | Source | Target | Cue / Action |
|---|---|---|---|---|
| 1 | 0:00 – 0:05 | OBS scene `intro-terminal` | Empty terminal, prompt visible | Cursor blinking. No keystrokes yet. Wordmark fades in top-left. |
| 2 | 0:05 – 0:15 | Same scene | Same | VO over static frame. Text overlay appears at 0:08, fades at 0:14. |
| 3 | 0:15 – 0:19 | OBS scene `readme-arch` | Browser at `https://github.com/solvela-ai/solvela#architecture` | Scroll-snap to the architecture mermaid. Hold. |
| 4 | 0:19 – 0:25 | OBS scene `intro-terminal` | Terminal | Cut back. VO continues without pause. |
| 5 | 0:25 – 0:35 | OBS scene `demo-terminal` | Terminal at the project root, gateway running on `:8402` | Type the curl command from `script.md` Act 3 / Demo 1. Use `pv` or pacing if you can't hand-type at ~60ms per char. |
| 6 | 0:35 – 0:40 | Same | Terminal | `jq` formatted output visible. Highlight (with cursor or terminal reverse-video) the `accepts` field showing both `exact` and `escrow`. |
| 7 | 0:40 – 0:48 | Same | Terminal | Type the Python SDK invocation. Tab-complete is fine. |
| 8 | 0:48 – 0:55 | Same | Terminal | SDK runs. The completion text scrolls in. Last line printed is the txn signature. |
| 9 | 0:55 – 1:00 | OBS scene `solana-explorer` | Browser at `https://explorer.solana.com/tx/<TXN_SIG>` | Paste the signature from shot 8. Page loads. |
| 10 | 1:00 – 1:10 | Same | Same | Scroll-snap to the Token Balances Change section. Highlight the USDC mint and the two-account delta. |
| 11 | 1:10 – 1:18 | OBS scene `arch-diagram-zoom` | Browser at `docs/architecture.md` rendered, zoomed to router box | Hold. VO covers the routing claim. |
| 12 | 1:18 – 1:25 | Same | Same | Lower-third text overlay appears at 1:18, fades at 1:25. |
| 13 | 1:25 – 1:28 | OBS scene `github-badges` | Browser at `https://github.com/solvela-ai/solvela` | Top of page. Badges visible. License table just below. |
| 14 | 1:28 – 1:30 | OBS scene `final-card` | Static slide, dark bg, orange accent | "github.com/solvela-ai/solvela" centered. Wordmark below. |

## Terminal commands — copy/paste ready

Keep these in a scratch buffer. During recording you'll re-type from memory but it's good to have the canonical version handy if a take fails.

### Demonstration 1 — The 402

```bash
curl -s http://localhost:8402/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{"model":"auto","messages":[{"role":"user","content":"Hello"}]}' | jq
```

### Demonstration 2 — Pay & call

```python
python -c "
from solvela import Client
c = Client(wallet='~/.solvela/wallet.json')
r = c.chat('Explain Solana in one sentence.', model='auto')
print(r.content)
print('txn:', r.payment_signature)
"
```

### Demonstration 3 — Verify on-chain

```bash
# Open in default browser
xdg-open "https://explorer.solana.com/tx/$(pbpaste)"   # macOS
# or
open "https://explorer.solana.com/tx/$(pbpaste)"        # macOS alt
# or just paste manually into the browser
```

## Visual continuity rules

- Use the **same terminal window** for shots 1, 2, 4, 5–8. Switching windows mid-demo signals "this was edited together" and breaks the live-demo feeling.
- Keep the same **prompt prefix** the entire video. Don't `cd` into different directories between takes.
- Browser zoom: **125%** for GitHub and Solana Explorer, so text reads at 1080p.
- Cursor: hide the OS cursor in still shots. Show it only when typing.

## What to do if a take fails

A failed take usually means: a network blip on the curl, a wallet that ran out of USDC, a typo. Don't try to "save" the take in editing. Just retake it. The whole video is 90 seconds — re-recording one demonstration costs you ~20 seconds of fresh take vs. 10 minutes of editing patch work.

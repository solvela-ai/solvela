# solvela-python

Pay-as-you-go LLM calls in Python via Solana USDC and x402 — no API keys.

Canonical SDK: https://github.com/solvela-ai/solvela-python.

## Install

Not yet on PyPI (tracked in [STATUS.md](../../STATUS.md)). Install from GitHub:

```bash
pip install "git+https://github.com/solvela-ai/solvela-python.git"
```

## Quickstart

Create a Solana wallet, fund it on devnet (https://faucet.solana.com), then
export `SOLVELA_WALLET_KEYFILE=~/.config/solana/id.json` (or
`SOLANA_PRIVATE_KEY=<base58-secret>`) and run:

```python
from solvela import Solvela

client = Solvela(base_url="https://api.solvela.ai")  # reads wallet from env

resp = client.chat.completions.create(
    model="auto",  # smart router picks the cheapest capable model
    messages=[{"role": "user", "content": "Explain x402 in one sentence."}],
)
print(resp.choices[0].message.content)
print(f"Paid: ${resp.payment.amount_usdc:.6f} via {resp.payment.tx_signature}")
```

## Streaming

```python
stream = client.chat.completions.create(
    model="anthropic-claude-sonnet-4-6",
    messages=[{"role": "user", "content": "Write a haiku about USDC."}],
    stream=True,
)
for chunk in stream:
    print(chunk.choices[0].delta.content or "", end="", flush=True)
```

## Estimate cost before paying

```python
# List pricing for every model:
for m in client.models.list():
    print(m.id, m.input_cost_per_million, m.output_cost_per_million)

# Or fetch the 402 challenge without paying:
quote = client.chat.completions.estimate(model="auto", messages=msgs)
print(f"Estimated: ${quote.cost_breakdown.total_usdc:.6f}")
```

Free-tier example: `model="openai-gpt-oss-120b"` is $0 (still needs a
0-amount payment header for replay protection).

## Error handling

Errors come back as a structured envelope. `e.type` is one of
`invalid_request_error`, `upstream_error`, `payment_required`,
`rate_limit_error`.

```python
from solvela import SolvelaError
try:
    client.chat.completions.create(model="auto", messages=[...])
except SolvelaError as e:
    print(f"[{e.type}] {e.message} (code={e.code})")
```

## Links

- Standalone repo: https://github.com/solvela-ai/solvela-python
- Docs: https://docs.solvela.ai
- Dashboard: https://solvela.vercel.app
- Gateway source: https://github.com/sky4/solvela

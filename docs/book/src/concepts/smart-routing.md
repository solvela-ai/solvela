# Smart Routing

The smart router classifies requests across 15 weighted dimensions and selects the best model based on the routing profile. Scoring is purely rule-based, runs in under 1 microsecond, and makes zero external calls.

## The 15 Dimensions

Each dimension produces a signal between -1.0 and 1.0. Signals are multiplied by their weight and summed to produce a final score.

| # | Dimension | Weight | What It Detects |
|---|-----------|--------|-----------------|
| 1 | Token count | 0.08 | Short messages score lower; long messages score higher |
| 2 | Code presence | 0.15 | Code blocks, function signatures, syntax patterns |
| 3 | Reasoning markers | 0.18 | "prove", "theorem", "step by step", "analyze", "explain why" |
| 4 | Technical terms | 0.10 | "algorithm", "kubernetes", "distributed", "protocol" |
| 5 | Creative markers | 0.05 | Creative writing, storytelling, poetry requests |
| 6 | Simple indicators | 0.02 | Greetings, trivial questions (negative weight) |
| 7 | Multi-step patterns | 0.12 | "first...then", numbered steps, sequential instructions |
| 8 | Question complexity | 0.05 | Nested questions, compound queries |
| 9 | Agentic task markers | 0.04 | Tool use, function calling, agent-style prompts |
| 10 | Math/logic | 0.06 | Equations, proofs, logical operators |
| 11 | Language complexity | 0.04 | Vocabulary sophistication, sentence structure |
| 12 | Conversation depth | 0.03 | Number of messages in the conversation |
| 13 | Tool usage | 0.04 | Presence of `tools` or `functions` in the request |
| 14 | Output format complexity | 0.02 | Structured output, JSON schema, tables |
| 15 | Domain specificity | 0.02 | Domain-specific jargon, specialized topics |

## Tier Classification

The weighted sum maps to a complexity tier:

| Score Range | Tier |
|-------------|------|
| < 0.0 | Simple |
| 0.0 -- 0.2 | Medium |
| 0.2 -- 0.4 | Complex |
| >= 0.4 | Reasoning |

## Routing Profiles

Each profile maps tiers to specific models:

### Eco Profile

Cheapest capable model for each tier. Use when cost matters more than quality.

| Tier | Model |
|------|-------|
| Simple | `deepseek/deepseek-chat` |
| Medium | `google/gemini-2.5-flash-lite` |
| Complex | `deepseek/deepseek-chat` |
| Reasoning | `deepseek/deepseek-reasoner` |

### Auto Profile (Default)

Balanced cost and quality. The default when using the `auto` alias.

| Tier | Model |
|------|-------|
| Simple | `google/gemini-2.5-flash` |
| Medium | `xai/grok-code-fast-1` |
| Complex | `google/gemini-3.1-pro` |
| Reasoning | `xai/grok-4-fast-reasoning` |

### Premium Profile

Best available model regardless of cost.

| Tier | Model |
|------|-------|
| Simple | `openai/gpt-4o` |
| Medium | `anthropic/claude-sonnet-4.6` |
| Complex | `anthropic/claude-opus-4.6` |
| Reasoning | `openai/o3` |

### Free Profile

Only free-tier models. No payment required.

| Tier | Model |
|------|-------|
| All tiers | `openai/gpt-oss-120b` |

## Profile Aliases

The following model aliases are recognized and mapped to profiles:

| Alias | Profile |
|-------|---------|
| `eco`, `cheap`, `budget` | Eco |
| `auto`, `balanced`, `default` | Auto |
| `premium`, `best`, `quality` | Premium |
| `free`, `oss`, `open` | Free |

## Example Classifications

| Request | Score | Tier | Auto Model |
|---------|-------|------|------------|
| "Hello!" | -0.4 | Simple | `google/gemini-2.5-flash` |
| "Write a Python function to sort a list" | 0.15 | Medium | `xai/grok-code-fast-1` |
| "Design a distributed consensus algorithm for a multi-region database" | 0.35 | Complex | `google/gemini-3.1-pro` |
| "Prove that P != NP using the diagonalization argument, step by step" | 0.55 | Reasoning | `xai/grok-4-fast-reasoning` |

## Using Smart Routing

### Via API

```bash
curl -X POST http://localhost:8402/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{"model": "auto", "messages": [{"role": "user", "content": "Hello"}]}'
```

### Via Python SDK

```python
from rustyclawrouter import LLMClient

client = LLMClient(api_url="http://localhost:8402")
response = client.smart_chat("Explain quantum entanglement", profile="eco")
```

### Via CLI

```bash
cargo run -p cli -- chat --model auto "Write a sorting algorithm"
```

## Debug Headers

When `X-RCR-Debug: true` is set, the response includes routing information:

```
X-RCR-Model-Resolved: google/gemini-2.5-flash
X-RCR-Route-Profile: auto
X-RCR-Route-Tier: simple
X-RCR-Route-Score: -0.3500
```

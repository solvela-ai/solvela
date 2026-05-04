# B-Roll Capture List (Optional)

Supplementary footage you can capture **after** the v1 cut is shipped, in case you decide to do a polish pass. None of this is required for v1. Skip this entire file unless v1 has shipped, the conversion is below expectations, and you want to A/B test a richer cut.

## Why optional

The v1 cut is a single linear demo. It works. Adding B-roll buys you ~10% more polish at ~3x the production cost. The marginal return rarely justifies the time on a v1, but it can matter for a v2 if you're cutting a longer-form pitch (3+ minutes) for a specific acquirer.

## Categories

### 1. Architecture animations [optional, ~2 hours work]

A 4–6 second animated reveal of the architecture mermaid would replace shot 11 (`arch-diagram-zoom` static hold) with motion. Tools:

- **Excalidraw → Lottie** if you want hand-drawn aesthetic.
- **After Effects** if you have it. Otherwise **Motion Canvas** (open source, code-driven animations, very on-brand for a developer tool).
- **Manim** if you want a CS-paper aesthetic (we have a `manim-video` skill for this).

What to animate: client request entering the gateway, gateway calling Solana, Solana confirming, gateway calling provider, response returning. ~5 second loop.

### 2. Code zoom-ins [optional, ~30 minutes]

Cut to a static, syntax-highlighted slice of the actual production code for ~3 seconds during the relevant claim. Examples:

- The 15-dimension scorer's match statement (during shot 11) — backs the "fifteen dimensions in microseconds" claim with visible evidence.
- The escrow program's `claim` instruction (during shot 11) — backs the "escrow live on mainnet" claim.

Use **Carbon** or **ray.so** to render the code into an image with the matching dark + orange theme. Ken-Burns slow zoom in DaVinci.

### 3. Solana Explorer scroll-throughs [optional, ~15 minutes]

Instead of a static hold on the Token Balances Change section in shot 10, do a slow scroll from the top of the txn page down to the balance changes. Conveys "this is a real, full transaction record, not a screenshot" more strongly. Use OBS's scene transition or just a slow trackpad scroll.

### 4. README scroll-through [optional, ~10 minutes]

A 3-second slow-scroll past the README's badge row + LICENSING table during the close shot. Reinforces the "open source, real license, real SDKs" closing message. Cheap and high-signal.

### 5. SDK matrix flash [optional, ~20 minutes]

A 2-second flash card showing the four SDK logos (Python, TypeScript, Go, Rust) plus the MCP logo, animated in. Goes into the close at 1:25–1:27. Tools: Figma → export PNG sequence → import to editor.

## What NOT to capture

These are tempting but they don't pay off:

- **Office or workspace shots** — Nobody cares where you work. They care that the product works.
- **Stock developer footage** (hands typing on a keyboard, generic code on a screen) — instantly reads as "padding to hit a length target."
- **Talking-head intro** — Adds 5–10 seconds and shifts attention from the product to the founder. Save the founder for the longer-form acquirer pitch deck, not the cold demo.
- **Slow zooms over the whole architecture diagram** — Eats 8–10 seconds and conveys less than the static zoomed-in version.

## Capture order

If you do go through with B-roll for a v2, capture in this order so you don't redo work:

1. Architecture animation (longest pole, most reusable across other materials)
2. Code zoom-ins (cheap, high signal, can be reused in pitch decks)
3. SDK matrix flash (cheap, reusable)
4. Solana Explorer scroll (free)
5. README scroll (free)

Then re-edit the v1 timeline with the new clips slotted into the existing scene markers. Do not re-shoot the live demo footage — your v1 take is already canon.

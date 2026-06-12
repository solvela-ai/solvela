'use client'

// Animated router / cache flow diagram for the Solvela landing surface.
//
// Two passes loop forever:
//   PASS A (cache hit):  request → exact-cache check → HIT → response returns.
//   PASS B (route):      request → exact-cache check → MISS → 15-dimension
//                        scorer → tier classification → profile column picks
//                        a model → provider.
//
// Driving model (mirrors the house pattern in escrow-diagram-animated.tsx /
// escrow-sequence.tsx, but self-contained because this component owns its own
// clock):
//   - `beat` PROVIDED  → fully controlled; render exactly that beat, no timers.
//   - `beat` UNDEFINED → self-running setTimeout chain, paused via
//                        IntersectionObserver when scrolled out of view, and
//                        rendered as a single COMPLETE static frame when the
//                        user prefers reduced motion.
//
// Ground truth:
//   crates/router/src/scorer.rs   — 15 weighted dimensions (names + weights)
//   crates/router/src/profiles.rs — tiers (simple/medium/complex/reasoning),
//                                    profiles (eco/auto/premium/free)
//   crates/gateway/src/cache/exact.rs — exact-match short-circuit before the
//                                    provider call; SHA-256 key ⇒ zero false
//                                    positives.

import { useEffect, useRef, useState } from 'react'

import { diagramPalette as C } from '@/lib/diagram-colors'

/**
 * Beats. The loop runs 0 → 9 then wraps. The two passes share the request and
 * cache-check frames so the diagram reads as one pipeline answering twice.
 *
 *   0  idle (request waiting)
 *   1  request → cache check                    (PASS A begins)
 *   2  cache HIT — response returns immediately
 *   3  hold on the returned response
 *   4  request → cache check                    (PASS B begins)
 *   5  cache MISS — continue down the pipeline
 *   6  15-dimension scorer evaluates
 *   7  tier classified
 *   8  profile column picks the model
 *   9  provider answers
 */
export type RouterBeat = 0 | 1 | 2 | 3 | 4 | 5 | 6 | 7 | 8 | 9

const LAST_BEAT = 9 as const

// Milliseconds per beat when self-driving. Tuned so the eye can read each
// stage; the two cache frames are quicker, the scorer/profile frames linger.
const BEAT_MS: Record<RouterBeat, number> = {
  0: 700,
  1: 1100,
  2: 1100,
  3: 1500,
  4: 1100,
  5: 900,
  6: 1500,
  7: 1100,
  8: 1400,
  9: 1700,
}

interface Props {
  /** When provided the diagram is fully controlled (no internal timers). */
  beat?: number
}

export function RouterCacheDiagram({ beat: controlled }: Props) {
  const isControlled = controlled !== undefined

  const [autoBeat, setAutoBeat] = useState<RouterBeat>(0)
  const [loopKey, setLoopKey] = useState(0)
  const [inView, setInView] = useState(false)
  const [reduced, setReduced] = useState(false)
  const wrapperRef = useRef<HTMLDivElement>(null)

  // Reduced-motion: pick up once, render the complete static frame (beat 9 view
  // augmented by `reduced` so BOTH passes' end-states are shown at once).
  useEffect(() => {
    if (isControlled) return
    const mq = window.matchMedia('(prefers-reduced-motion: reduce)')
    if (mq.matches) setReduced(true)
  }, [isControlled])

  // Pause when out of view — resume from the current beat (don't reset).
  useEffect(() => {
    if (isControlled || reduced) return
    const node = wrapperRef.current
    if (!node) return
    const io = new IntersectionObserver(
      (entries) => {
        for (const e of entries) setInView(e.isIntersecting)
      },
      { rootMargin: '200px 0px' }
    )
    io.observe(node)
    return () => io.disconnect()
  }, [isControlled, reduced])

  // Self-running clock: one timeout per beat, cleaned up on every state change.
  useEffect(() => {
    if (isControlled || reduced || !inView) return
    const id = setTimeout(() => {
      setAutoBeat((b) => {
        const next = (b >= LAST_BEAT ? 0 : b + 1) as RouterBeat
        return next
      })
      // Bump the loop key whenever we wrap so SMIL packets remount and replay.
      if (autoBeat >= LAST_BEAT) setLoopKey((k) => k + 1)
    }, BEAT_MS[autoBeat])
    return () => clearTimeout(id)
  }, [autoBeat, inView, isControlled, reduced])

  const beat: RouterBeat = isControlled
    ? (clampBeat(controlled) as RouterBeat)
    : autoBeat

  return (
    <div ref={wrapperRef} className="h-full w-full">
      <DiagramSvg beat={beat} loopKey={isControlled ? 0 : loopKey} staticAll={reduced} />
    </div>
  )
}

function clampBeat(n: number): number {
  if (Number.isNaN(n)) return 0
  if (n < 0) return 0
  if (n > LAST_BEAT) return LAST_BEAT
  return Math.floor(n)
}

/* ── Geometry ────────────────────────────────────────────────────────────
   A single horizontal pipeline that stays legible at 320px because the SVG
   scales to the container width while preserving aspect ratio. Coordinates
   live in a 720×420 viewBox. The cache node branches UP to the response chip
   (hit) and DOWN-RIGHT into the scorer (miss). */

const VB_W = 720
const VB_H = 420

// X anchors for the pipeline columns.
const X_REQ = 70
const X_CACHE = 214
const X_SCORER = 372
const X_TIER = 522
const X_PROFILE = 522
const X_PROVIDER = 650

// Y anchors.
const Y_MAIN = 232 // the main left-to-right spine
const Y_HIT = 92 // the response chip sits above the spine

/* ── The 15 scorer dimensions (verbatim from scorer.rs WEIGHTS comments) ──
   We surface real names + their real weights as a compact tick grid. No
   latency is shown — the scorer is deterministic with zero external calls. */

interface Dim {
  label: string
  weight: number
}

const DIMENSIONS: Dim[] = [
  { label: 'token count', weight: 0.08 },
  { label: 'code presence', weight: 0.15 },
  { label: 'reasoning markers', weight: 0.18 },
  { label: 'technical terms', weight: 0.1 },
  { label: 'creative markers', weight: 0.05 },
  { label: 'simple indicators', weight: 0.02 },
  { label: 'multi-step', weight: 0.12 },
  { label: 'question complexity', weight: 0.05 },
  { label: 'agentic markers', weight: 0.04 },
  { label: 'math / logic', weight: 0.06 },
  { label: 'language complexity', weight: 0.04 },
  { label: 'conversation depth', weight: 0.03 },
  { label: 'tool usage', weight: 0.04 },
  { label: 'output format', weight: 0.02 },
  { label: 'domain specificity', weight: 0.02 },
]

const TIERS = ['simple', 'medium', 'complex', 'reasoning'] as const
const PROFILES = ['eco', 'auto', 'premium', 'free'] as const

// In the route pass we land on the "complex" tier and the "auto" profile,
// which together resolve to google/gemini-3.1-pro (profiles.rs).
const PICKED_TIER = 'complex'
const PICKED_PROFILE = 'auto'
const PICKED_MODEL = 'gemini-3.1-pro'

interface SvgProps {
  beat: RouterBeat
  loopKey: number
  /** Reduced-motion: render every stage of both passes simultaneously. */
  staticAll: boolean
}

function DiagramSvg({ beat, loopKey, staticAll }: SvgProps) {
  // Phase flag. In static mode everything is "reached" so the full pipeline
  // and both end-states render at once with no motion.
  const passB = staticAll || beat >= 4

  const cacheActive = staticAll || beat === 1 || beat === 4
  const hitShown = staticAll || beat >= 2
  const missShown = staticAll || beat >= 5
  const scorerActive = staticAll || beat === 6
  const scorerReached = staticAll || beat >= 6
  const tierReached = staticAll || beat >= 7
  const profileReached = staticAll || beat >= 8
  const providerReached = staticAll || beat >= 9

  const ariaLabel =
    'Solvela request pipeline. A request first hits the exact-match cache. ' +
    'On a cache hit the stored response returns immediately with zero false ' +
    'positives. On a miss the request flows into a deterministic ' +
    '15-dimension scorer with zero external calls, which classifies it into a ' +
    'complexity tier; the chosen routing profile then picks a model and the ' +
    'provider answers.'

  return (
    <svg
      viewBox={`0 0 ${VB_W} ${VB_H}`}
      role="img"
      aria-label={ariaLabel}
      className="h-full w-full"
      preserveAspectRatio="xMidYMid meet"
    >
      <defs>
        <linearGradient id="rc-node" x1="0" x2="0" y1="0" y2="1">
          <stop offset="0" stopColor={C.nodeFrontMid} />
          <stop offset="1" stopColor={C.nodeFrontDark} />
        </linearGradient>
        <linearGradient id="rc-cache" x1="0" x2="0" y1="0" y2="1">
          <stop offset="0" stopColor={C.nodeTopSurface} />
          <stop offset="1" stopColor={C.nodeFrontDark} />
        </linearGradient>

        {/* Connector paths (packets ride these). */}
        <path id="rc-path-req-cache" d={`M ${X_REQ + 40} ${Y_MAIN} L ${X_CACHE - 46} ${Y_MAIN}`} fill="none" />
        <path id="rc-path-hit" d={`M ${X_CACHE} ${Y_MAIN - 44} C ${X_CACHE} ${Y_HIT + 60}, ${X_CACHE} ${Y_HIT + 40}, ${X_CACHE + 8} ${Y_HIT + 24}`} fill="none" />
        <path id="rc-path-miss" d={`M ${X_CACHE + 46} ${Y_MAIN} L ${X_SCORER - 58} ${Y_MAIN}`} fill="none" />
        <path id="rc-path-scorer-tier" d={`M ${X_SCORER + 58} ${Y_MAIN} L ${X_TIER - 44} ${Y_MAIN}`} fill="none" />
        <path id="rc-path-profile-provider" d={`M ${X_PROFILE + 44} ${Y_MAIN + 40} C ${X_PROVIDER - 30} ${Y_MAIN + 40}, ${X_PROVIDER - 20} ${Y_MAIN}, ${X_PROVIDER} ${Y_MAIN}`} fill="none" />
      </defs>

      {/* ── Static guide rails (faint) ─────────────────────────────────── */}
      <g fill="none" strokeWidth="1.5">
        <use href="#rc-path-req-cache" stroke={C.nodeStroke} opacity={cacheActive || hitShown || missShown ? 0.5 : 0.2} />
        <use
          href="#rc-path-hit"
          stroke={C.accentGold}
          strokeDasharray="4 4"
          opacity={hitShown ? 0.55 : 0.18}
        />
        <use
          href="#rc-path-miss"
          stroke={C.nodeStroke}
          strokeDasharray="4 4"
          opacity={missShown ? 0.55 : 0.18}
        />
        <use href="#rc-path-scorer-tier" stroke={C.nodeStroke} opacity={tierReached ? 0.5 : 0.18} />
        <use href="#rc-path-profile-provider" stroke={C.accentSalmon} opacity={providerReached ? 0.5 : 0.18} />
      </g>

      {/* ── Request node ───────────────────────────────────────────────── */}
      <StageBox
        cx={X_REQ}
        cy={Y_MAIN}
        w={80}
        h={52}
        fill="url(#rc-node)"
        label="request"
        sub="POST /v1"
        dot={C.neutralText}
        active={beat === 0 || beat === 1 || beat === 4}
      />

      {/* ── Exact-cache node ───────────────────────────────────────────── */}
      <StageBox
        cx={X_CACHE}
        cy={Y_MAIN}
        w={92}
        h={60}
        fill="url(#rc-cache)"
        label="exact cache"
        sub="SHA-256 key"
        dot={C.accentGold}
        active={cacheActive}
        highlight={cacheActive}
      />

      {/* ── Cache-hit response chip (above the spine) ──────────────────── */}
      <ResponseChip cx={X_CACHE + 78} cy={Y_HIT} shown={hitShown} dim={passB && !staticAll && beat >= 4} />

      {/* ── 15-dimension scorer (weight grid) ──────────────────────────── */}
      <ScorerGrid
        cx={X_SCORER}
        cy={Y_MAIN}
        active={scorerActive}
        reached={scorerReached}
      />

      {/* ── Tier classification column ─────────────────────────────────── */}
      <PickColumn
        x={X_TIER}
        topY={Y_MAIN - 96}
        title="tier"
        items={TIERS as unknown as string[]}
        picked={PICKED_TIER}
        reached={tierReached}
        tone={C.neutralText}
      />

      {/* ── Profile column (picks the model) ───────────────────────────── */}
      <PickColumn
        x={X_PROFILE}
        topY={Y_MAIN + 28}
        title="profile"
        items={PROFILES as unknown as string[]}
        picked={PICKED_PROFILE}
        reached={profileReached}
        tone={C.accentGold}
      />

      {/* Picked-model caption between profile and provider. */}
      {(profileReached || providerReached) && (
        <text
          x={X_PROFILE + 56}
          y={Y_MAIN - 4}
          fontFamily="'JetBrains Mono', monospace"
          fontSize="10"
          fill={C.accentGold}
          opacity={providerReached ? 1 : 0.7}
        >
          {PICKED_MODEL}
        </text>
      )}

      {/* ── Provider node ──────────────────────────────────────────────── */}
      <StageBox
        cx={X_PROVIDER}
        cy={Y_MAIN}
        w={86}
        h={56}
        fill="url(#rc-node)"
        label="provider"
        sub="LLM call"
        dot={C.accentSalmon}
        active={providerReached}
        highlight={beat === 9}
      />

      {/* ── Animated packets (transient; remount each loop via loopKey) ── */}
      {!staticAll && beat === 1 && (
        <g key={`reqA-${loopKey}`}>
          <Packet pathId="#rc-path-req-cache" color={C.neutralText} dur="0.9s" />
        </g>
      )}
      {!staticAll && beat === 2 && (
        <g key={`hit-${loopKey}`}>
          <Packet pathId="#rc-path-hit" color={C.accentGold} dur="0.85s" />
        </g>
      )}
      {!staticAll && beat === 4 && (
        <g key={`reqB-${loopKey}`}>
          <Packet pathId="#rc-path-req-cache" color={C.neutralText} dur="0.9s" />
        </g>
      )}
      {!staticAll && beat === 5 && (
        <g key={`miss-${loopKey}`}>
          <Packet pathId="#rc-path-miss" color={C.neutralText} dur="0.9s" />
        </g>
      )}
      {!staticAll && beat === 7 && (
        <g key={`tier-${loopKey}`}>
          <Packet pathId="#rc-path-scorer-tier" color={C.neutralText} dur="0.85s" />
        </g>
      )}
      {!staticAll && beat === 9 && (
        <g key={`prov-${loopKey}`}>
          <Packet pathId="#rc-path-profile-provider" color={C.accentSalmon} dur="0.85s" />
        </g>
      )}

      {/* ── Bottom legend ──────────────────────────────────────────────── */}
      <g transform={`translate(${VB_W / 2}, ${VB_H - 20})`} textAnchor="middle">
        <text
          x="0"
          y="0"
          fontFamily="'JetBrains Mono', monospace"
          fontSize="10"
          letterSpacing="1.5"
          fill={C.neutralText}
          opacity="0.6"
        >
          {staticAll
            ? 'cache hit · zero false positives    │    miss → deterministic scorer · zero external calls'
            : passB
              ? 'miss → deterministic scorer · zero external calls'
              : 'cache hit · zero false positives'}
        </text>
      </g>
    </svg>
  )
}

/* ── Sub-components ────────────────────────────────────────────────────── */

interface StageBoxProps {
  cx: number
  cy: number
  w: number
  h: number
  fill: string
  label: string
  sub: string
  dot: string
  active?: boolean
  highlight?: boolean
}

function StageBox({ cx, cy, w, h, fill, label, sub, dot, active, highlight }: StageBoxProps) {
  const x = cx - w / 2
  const y = cy - h / 2
  const stroke = active ? C.accentGold : C.nodeStroke
  return (
    <g style={highlight ? { filter: `drop-shadow(0 0 5px ${C.accentGold})` } : undefined}>
      <rect
        x={x}
        y={y}
        width={w}
        height={h}
        rx={6}
        fill={fill}
        stroke={stroke}
        strokeWidth={active ? 1.4 : 1}
      />
      <circle cx={x + 11} cy={y + 12} r="2.5" fill={dot} />
      <text
        x={cx}
        y={cy - 2}
        textAnchor="middle"
        fontFamily="'JetBrains Mono', monospace"
        fontSize="12"
        fill={C.headingText}
        letterSpacing="0.5"
      >
        {label}
      </text>
      <text
        x={cx}
        y={cy + 14}
        textAnchor="middle"
        fontFamily="'JetBrains Mono', monospace"
        fontSize="9"
        fill={C.neutralText}
        opacity="0.6"
        letterSpacing="0.5"
      >
        {sub}
      </text>
    </g>
  )
}

interface ResponseChipProps {
  cx: number
  cy: number
  shown: boolean
  /** Fade the hit chip while pass B runs so it doesn't compete for attention. */
  dim: boolean
}

function ResponseChip({ cx, cy, shown, dim }: ResponseChipProps) {
  const opacity = shown ? (dim ? 0.32 : 1) : 0.12
  const w = 132
  const h = 44
  const x = cx - w / 2
  const y = cy - h / 2
  return (
    <g opacity={opacity} style={{ transition: 'opacity 300ms ease' }}>
      <rect
        x={x}
        y={y}
        width={w}
        height={h}
        rx={6}
        fill={C.nodeBlackish}
        stroke={C.accentGold}
        strokeWidth={shown && !dim ? 1.4 : 1}
      />
      <text
        x={cx}
        y={cy - 3}
        textAnchor="middle"
        fontFamily="'JetBrains Mono', monospace"
        fontSize="11"
        fill={C.accentGold}
        letterSpacing="0.5"
      >
        cached response
      </text>
      <text
        x={cx}
        y={cy + 12}
        textAnchor="middle"
        fontFamily="'JetBrains Mono', monospace"
        fontSize="8"
        fill={C.neutralText}
        opacity="0.7"
        letterSpacing="1"
      >
        returns immediately
      </text>
    </g>
  )
}

interface ScorerGridProps {
  cx: number
  cy: number
  active: boolean
  reached: boolean
}

function ScorerGrid({ cx, cy, active, reached }: ScorerGridProps) {
  // 5 columns × 3 rows of weight ticks. Each tick's height encodes its real
  // weight from scorer.rs. The grid sits inside a labelled panel.
  const cols = 5
  const rows = 3
  const cellW = 19
  const cellH = 26
  const gridW = cols * cellW
  const gridH = rows * cellH
  const panelPadX = 14
  const panelPadTop = 30
  const panelPadBottom = 16
  const panelW = gridW + panelPadX * 2
  const panelH = gridH + panelPadTop + panelPadBottom
  const panelX = cx - panelW / 2
  const panelY = cy - panelH / 2

  const gridX = panelX + panelPadX
  const gridY = panelY + panelPadTop

  const maxW = Math.max(...DIMENSIONS.map((d) => d.weight))
  const barMax = cellH - 6

  return (
    <g style={active ? { filter: `drop-shadow(0 0 6px ${C.accentGold})` } : undefined}>
      <rect
        x={panelX}
        y={panelY}
        width={panelW}
        height={panelH}
        rx={7}
        fill={C.nodeFrontDark}
        stroke={active ? C.accentGold : C.nodeStroke}
        strokeWidth={active ? 1.4 : 1}
      />
      <text
        x={cx}
        y={panelY + 18}
        textAnchor="middle"
        fontFamily="'JetBrains Mono', monospace"
        fontSize="11"
        fill={C.headingText}
        letterSpacing="0.5"
      >
        15-dim scorer
      </text>

      {DIMENSIONS.map((d, i) => {
        const col = i % cols
        const row = Math.floor(i / cols)
        const bx = gridX + col * cellW + cellW / 2
        const baseY = gridY + row * cellH + cellH - 4
        const barH = 4 + (d.weight / maxW) * barMax
        return (
          <g key={d.label}>
            <title>{`${d.label} · weight ${d.weight}`}</title>
            <rect
              x={bx - 5}
              y={baseY - barH}
              width={10}
              height={barH}
              rx={1.5}
              fill={reached ? C.accentGold : C.neutralText}
              opacity={reached ? 0.85 : 0.4}
            />
          </g>
        )
      })}

      <text
        x={cx}
        y={panelY + panelH - 5}
        textAnchor="middle"
        fontFamily="'JetBrains Mono', monospace"
        fontSize="8"
        fill={C.neutralText}
        opacity="0.6"
        letterSpacing="1"
      >
        deterministic
      </text>
    </g>
  )
}

interface PickColumnProps {
  x: number
  topY: number
  title: string
  items: string[]
  picked: string
  reached: boolean
  tone: string
}

function PickColumn({ x, topY, title, items, picked, reached, tone }: PickColumnProps) {
  const rowH = 17
  const w = 86
  return (
    <g opacity={reached ? 1 : 0.4} style={{ transition: 'opacity 300ms ease' }}>
      <text
        x={x}
        y={topY - 6}
        textAnchor="middle"
        fontFamily="'JetBrains Mono', monospace"
        fontSize="9"
        fill={C.neutralText}
        opacity="0.6"
        letterSpacing="1.5"
      >
        {title.toUpperCase()}
      </text>
      {items.map((item, i) => {
        const y = topY + i * rowH
        const isPicked = reached && item === picked
        return (
          <g key={item}>
            <rect
              x={x - w / 2}
              y={y}
              width={w}
              height={rowH - 3}
              rx={3}
              fill={isPicked ? C.nodeBlackish : 'none'}
              stroke={isPicked ? tone : C.nodeStroke}
              strokeWidth={isPicked ? 1.3 : 0.75}
              opacity={isPicked ? 1 : 0.55}
            />
            <text
              x={x}
              y={y + rowH - 7}
              textAnchor="middle"
              fontFamily="'JetBrains Mono', monospace"
              fontSize="10"
              fill={isPicked ? tone : C.neutralText}
              opacity={isPicked ? 1 : 0.55}
            >
              {item}
            </text>
          </g>
        )
      })}
    </g>
  )
}

interface PacketProps {
  pathId: string
  color: string
  dur: string
}

function Packet({ pathId, color, dur }: PacketProps) {
  return (
    <circle r="5" fill={color}>
      <animateMotion dur={dur} begin="0s" fill="freeze" rotate="auto">
        <mpath href={pathId} />
      </animateMotion>
    </circle>
  )
}

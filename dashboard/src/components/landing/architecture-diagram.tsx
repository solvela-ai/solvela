'use client'

// System architecture diagram for the Solvela landing page.
//
// Unlike <EscrowDiagramAnimated> (a narrated, beat-driven sequence), this is a
// REFERENCE diagram: mostly static, true to the codebase topology. The only
// motion is a subtle ambient "packet drift" along the main request path
// (client → gateway → provider), which is:
//   • paused while the diagram is out of view (IntersectionObserver), and
//   • absent entirely under prefers-reduced-motion.
//
// Everything else is plain SVG and renders server-side. The 'use client'
// directive is only here because the ambient drift needs the hooks below; the
// static structure does not depend on the client at all.
//
// Topology + labels are grounded in:
//   crates/{gateway,x402,router,protocol,cli} + programs/escrow + CLAUDE.md.
// No invented services — every label maps to a real crate, module, store,
// provider, or on-chain account.

import { useEffect, useRef, useState } from 'react'

import { diagramPalette as C } from '@/lib/diagram-colors'

/* ── Public API ──────────────────────────────────────────────────────
 * <ArchitectureDiagram compact?={boolean} />
 *   compact — stack the planes vertically (clients → gateway → providers,
 *   settlement below) for narrow viewports. Defaults to false (wide layout).
 *   The wide SVG is itself fully responsive down to 320px via viewBox scaling;
 *   `compact` is offered for callers that prefer a taller, single-column read.
 * ─────────────────────────────────────────────────────────────────── */

interface ArchitectureDiagramProps {
  compact?: boolean
}

const ARIA_LABEL =
  'Solvela system architecture. Clients (TypeScript, Python, Go, Rust, and MCP SDKs, plus A2A agents via agent-card) call the Solvela gateway, an Axum server containing x402 payment verification, a 15-dimension router, an exact response cache, and a prompt guard. The gateway uses optional Redis (cache, replay, rate limit) and optional PostgreSQL (spend, orgs, audit) side stores, and proxies to five providers: OpenAI, Anthropic, Google, xAI, and DeepSeek. Payments settle on Solana mainnet: the exact scheme is a deferred USDC-SPL transfer, while the escrow scheme deposits to an on-chain escrow program PDA and claims plus refunds in one transaction; a fee-payer pool sponsors transactions.'

export function ArchitectureDiagram({ compact = false }: ArchitectureDiagramProps) {
  const wrapperRef = useRef<HTMLDivElement>(null)
  const [drift, setDrift] = useState(false)

  // Ambient drift runs only when in view AND reduced-motion is not requested.
  useEffect(() => {
    const node = wrapperRef.current
    if (!node) return
    if (window.matchMedia('(prefers-reduced-motion: reduce)').matches) return

    const io = new IntersectionObserver(
      (entries) => {
        for (const e of entries) setDrift(e.isIntersecting)
      },
      { rootMargin: '120px 0px' }
    )
    io.observe(node)
    return () => io.disconnect()
  }, [])

  return (
    <div ref={wrapperRef} className="h-full w-full">
      {compact ? (
        <CompactSvg drift={drift} />
      ) : (
        <WideSvg drift={drift} />
      )}
    </div>
  )
}

/* ── Wide (default) layout ───────────────────────────────────────────
 * 960×620 viewBox. Three vertical planes (clients · gateway · providers)
 * with a settlement plane spanning the bottom.
 * ─────────────────────────────────────────────────────────────────── */

function WideSvg({ drift }: { drift: boolean }) {
  return (
    <svg
      viewBox="0 0 960 620"
      role="img"
      aria-label={ARIA_LABEL}
      className="h-full w-full"
      preserveAspectRatio="xMidYMid meet"
    >
      <Defs />

      {/* ── Edges (drawn under nodes) ─────────────────────────────── */}
      <g fill="none">
        {/* request bus: clients → gateway */}
        <path
          id="arch-req"
          d="M 232 196 C 290 196, 318 224, 360 224"
          stroke={C.neutralText}
          strokeWidth="1.5"
          opacity="0.5"
        />
        {/* response/proxy bus: gateway → providers */}
        <path
          id="arch-proxy"
          d="M 600 224 C 648 224, 676 196, 728 196"
          stroke={C.neutralText}
          strokeWidth="1.5"
          opacity="0.5"
        />
        {/* gateway → redis (right side store) */}
        <line x1="480" y1="312" x2="480" y2="352" stroke={C.nodeStroke} strokeWidth="1" strokeDasharray="4 4" opacity="0.6" />
        {/* redis → postgres are siblings; gateway → postgres */}
        <line x1="690" y1="312" x2="690" y2="352" stroke={C.nodeStroke} strokeWidth="1" strokeDasharray="4 4" opacity="0.6" />

        {/* settlement: exact (deferred transfer) — gold */}
        <path
          id="arch-exact"
          d="M 410 318 C 380 420, 360 470, 320 506"
          stroke={C.accentGold}
          strokeWidth="1.5"
          strokeDasharray="2 5"
          opacity="0.7"
        />
        {/* settlement: escrow deposit/claim+refund — salmon */}
        <path
          id="arch-escrow"
          d="M 520 318 C 560 420, 600 470, 660 506"
          stroke={C.accentSalmon}
          strokeWidth="1.5"
          strokeDasharray="2 5"
          opacity="0.7"
        />
        <ArrowHead x={357} y={224} dir="right" color={C.neutralText} />
        <ArrowHead x={731} y={196} dir="right" color={C.neutralText} />
      </g>

      {/* edge captions */}
      <EdgeLabel x={296} y={188} text="POST /v1/chat/completions" />
      <EdgeLabel x={664} y={188} text="OpenAI ⇄ native" />

      {/* ── Plane labels ──────────────────────────────────────────── */}
      <PlaneLabel x={120} y={62} text="clients" />
      <PlaneLabel x={480} y={62} text="solvela gateway · axum" />
      <PlaneLabel x={840} y={62} text="providers" />

      {/* ── Left plane: clients ───────────────────────────────────── */}
      <ClientStack x={40} y={96} />
      {/* A2A agents */}
      <NodeBox
        x={40}
        y={256}
        w={192}
        h={56}
        title="A2A agents"
        sub=".well-known/agent-card.json"
        dot={C.accentGold}
      />

      {/* ── Center plane: gateway ─────────────────────────────────── */}
      <GatewayBox x={360} y={96} drift={drift} />

      {/* side stores */}
      <StoreBox
        x={384}
        y={352}
        title="redis"
        lines={['cache', 'replay', 'rate']}
        note="optional · degrades"
      />
      <StoreBox
        x={594}
        y={352}
        title="postgres"
        lines={['spend', 'orgs', 'audit']}
        note="optional · degrades"
      />

      {/* ── Right plane: providers ────────────────────────────────── */}
      <ProviderStack x={728} y={96} />

      {/* ── Settlement plane ──────────────────────────────────────── */}
      <SettlementPlane x={40} y={486} w={880} h={104} />

      {/* drift packets on the main request path (in view + motion only) */}
      {drift && (
        <g aria-hidden="true">
          <DriftPacket pathId="#arch-req" color={C.neutralText} dur="3.2s" begin="0s" />
          <DriftPacket pathId="#arch-req" color={C.neutralText} dur="3.2s" begin="1.6s" />
          <DriftPacket pathId="#arch-proxy" color={C.neutralText} dur="3.2s" begin="0.8s" />
          <DriftPacket pathId="#arch-proxy" color={C.neutralText} dur="3.2s" begin="2.4s" />
        </g>
      )}
    </svg>
  )
}

/* ── Compact layout ──────────────────────────────────────────────────
 * 380×900 viewBox. Single column: clients → gateway (+ stores) →
 * providers, settlement plane at the bottom. Same labels, stacked.
 * ─────────────────────────────────────────────────────────────────── */

function CompactSvg({ drift }: { drift: boolean }) {
  return (
    <svg
      viewBox="0 0 380 980"
      role="img"
      aria-label={ARIA_LABEL}
      className="h-full w-full"
      preserveAspectRatio="xMidYMid meet"
    >
      <Defs />

      {/* vertical request spine */}
      <g fill="none">
        <path
          id="arch-c-req"
          d="M 190 214 L 190 250"
          stroke={C.neutralText}
          strokeWidth="1.5"
          opacity="0.5"
        />
        <path
          id="arch-c-proxy"
          d="M 190 560 L 190 596"
          stroke={C.neutralText}
          strokeWidth="1.5"
          opacity="0.5"
        />
        <ArrowHead x={190} y={249} dir="down" color={C.neutralText} />
        <ArrowHead x={190} y={595} dir="down" color={C.neutralText} />

        {/* settlement links */}
        <path
          d="M 150 568 C 130 660, 120 720, 120 760"
          stroke={C.accentGold}
          strokeWidth="1.5"
          strokeDasharray="2 5"
          opacity="0.7"
        />
        <path
          d="M 230 568 C 250 660, 260 720, 260 760"
          stroke={C.accentSalmon}
          strokeWidth="1.5"
          strokeDasharray="2 5"
          opacity="0.7"
        />
      </g>

      <PlaneLabel x={190} y={36} text="clients" />
      <SdkChips x={30} y={56} w={320} />
      <NodeBox
        x={30}
        y={148}
        w={320}
        h={56}
        title="A2A agents"
        sub=".well-known/agent-card.json"
        dot={C.accentGold}
        center
      />

      <PlaneLabel x={190} y={282} text="solvela gateway · axum" />
      <GatewayBox x={30} y={300} drift={drift} compact />

      <StoreBox x={40} y={476} title="redis" lines={['cache · replay · rate']} note="optional" wideLine />
      <StoreBox x={206} y={476} title="postgres" lines={['spend · orgs · audit']} note="optional" wideLine />

      <PlaneLabel x={190} y={628} text="providers" />
      <ProviderChips x={30} y={648} w={320} />

      <SettlementPlane x={30} y={744} w={320} h={206} compact />

      {drift && (
        <g aria-hidden="true">
          <DriftPacket pathId="#arch-c-req" color={C.neutralText} dur="2.6s" begin="0s" />
          <DriftPacket pathId="#arch-c-proxy" color={C.neutralText} dur="2.6s" begin="1.3s" />
        </g>
      )}
    </svg>
  )
}

/* ── Defs ────────────────────────────────────────────────────────── */

function Defs() {
  return (
    <defs>
      <linearGradient id="arch-node" x1="0" x2="0" y1="0" y2="1">
        <stop offset="0" stopColor={C.nodeFrontMid} />
        <stop offset="1" stopColor={C.nodeFrontDark} />
      </linearGradient>
      <linearGradient id="arch-gateway" x1="0" x2="0" y1="0" y2="1">
        <stop offset="0" stopColor={C.nodeTopSurface} />
        <stop offset="1" stopColor={C.nodeRightDark} />
      </linearGradient>
      <linearGradient id="arch-plane" x1="0" x2="0" y1="0" y2="1">
        <stop offset="0" stopColor={C.nodeFrontDark} />
        <stop offset="1" stopColor={C.nodeBlackish} />
      </linearGradient>
    </defs>
  )
}

/* ── Plane / structural pieces ───────────────────────────────────── */

function PlaneLabel({ x, y, text }: { x: number; y: number; text: string }) {
  return (
    <text
      x={x}
      y={y}
      textAnchor="middle"
      fontFamily="'JetBrains Mono', monospace"
      fontSize="11"
      fill={C.neutralText}
      opacity="0.6"
      letterSpacing="2"
    >
      {text.toUpperCase()}
    </text>
  )
}

function EdgeLabel({ x, y, text }: { x: number; y: number; text: string }) {
  return (
    <text
      x={x}
      y={y}
      textAnchor="middle"
      fontFamily="'JetBrains Mono', monospace"
      fontSize="9"
      fill={C.neutralText}
      opacity="0.55"
      letterSpacing="0.5"
    >
      {text}
    </text>
  )
}

interface NodeBoxProps {
  x: number
  y: number
  w: number
  h: number
  title: string
  sub?: string
  dot?: string
  center?: boolean
}

function NodeBox({ x, y, w, h, title, sub, dot, center }: NodeBoxProps) {
  const tx = center ? x + w / 2 : x + 16
  const anchor = center ? 'middle' : 'start'
  return (
    <g>
      <rect
        x={x}
        y={y}
        width={w}
        height={h}
        rx="6"
        fill="url(#arch-node)"
        stroke={C.nodeStroke}
        strokeWidth="1"
      />
      {dot && !center && <circle cx={x + 14} cy={y + 16} r="3" fill={dot} />}
      <text
        x={center ? tx : x + (dot ? 28 : 16)}
        y={y + (sub ? 24 : h / 2 + 4)}
        textAnchor={anchor}
        fontFamily="'JetBrains Mono', monospace"
        fontSize="13"
        fill={C.headingText}
        letterSpacing="0.5"
      >
        {title}
      </text>
      {sub && (
        <text
          x={center ? tx : x + (dot ? 28 : 16)}
          y={y + 42}
          textAnchor={anchor}
          fontFamily="'JetBrains Mono', monospace"
          fontSize="9.5"
          fill={C.neutralText}
          opacity="0.6"
        >
          {sub}
        </text>
      )}
    </g>
  )
}

/* ── Clients ─────────────────────────────────────────────────────── */

const SDKS = ['ts', 'python', 'go', 'rust', 'mcp'] as const

function ClientStack({ x, y }: { x: number; y: number }) {
  return (
    <g>
      <rect
        x={x}
        y={y}
        width={192}
        height={142}
        rx="6"
        fill="url(#arch-node)"
        stroke={C.nodeStroke}
        strokeWidth="1"
      />
      <text
        x={x + 16}
        y={y + 26}
        fontFamily="'JetBrains Mono', monospace"
        fontSize="13"
        fill={C.headingText}
        letterSpacing="0.5"
      >
        SDKs
      </text>
      <text
        x={x + 16}
        y={y + 42}
        fontFamily="'JetBrains Mono', monospace"
        fontSize="9"
        fill={C.neutralText}
        opacity="0.55"
      >
        x402 + signed tx, no api keys
      </text>
      <g>
        {SDKS.map((s, i) => (
          <Chip key={s} x={x + 16 + (i % 3) * 56} y={y + 58 + Math.floor(i / 3) * 36} label={s} />
        ))}
      </g>
    </g>
  )
}

function SdkChips({ x, y, w }: { x: number; y: number; w: number }) {
  return (
    <g>
      <rect x={x} y={y} width={w} height={76} rx="6" fill="url(#arch-node)" stroke={C.nodeStroke} strokeWidth="1" />
      <text
        x={x + w / 2}
        y={y + 24}
        textAnchor="middle"
        fontFamily="'JetBrains Mono', monospace"
        fontSize="13"
        fill={C.headingText}
      >
        SDKs
      </text>
      <g>
        {SDKS.map((s, i) => (
          <Chip key={s} x={x + 14 + i * 61} y={y + 40} label={s} />
        ))}
      </g>
    </g>
  )
}

function Chip({ x, y, label }: { x: number; y: number; label: string }) {
  const w = Math.max(34, label.length * 8 + 14)
  return (
    <g>
      <rect
        x={x}
        y={y}
        width={w}
        height={22}
        rx="4"
        fill={C.nodeBlackish}
        stroke={C.nodeStroke}
        strokeWidth="0.75"
      />
      <text
        x={x + w / 2}
        y={y + 15}
        textAnchor="middle"
        fontFamily="'JetBrains Mono', monospace"
        fontSize="11"
        fill={C.neutralText}
      >
        {label}
      </text>
    </g>
  )
}

/* ── Gateway ─────────────────────────────────────────────────────── */

const GATEWAY_MODULES: { title: string; sub: string }[] = [
  { title: 'x402 verify', sub: 'payment middleware' },
  { title: '15-dim router', sub: 'eco · auto · premium' },
  { title: 'exact cache', sub: 'wallet-agnostic' },
  { title: 'prompt guard', sub: 'injection · pii' },
]

function GatewayBox({
  x,
  y,
  drift,
  compact,
}: {
  x: number
  y: number
  drift: boolean
  compact?: boolean
}) {
  const w = compact ? 320 : 240
  const h = compact ? 158 : 216
  return (
    <g>
      <rect
        x={x}
        y={y}
        width={w}
        height={h}
        rx="8"
        fill="url(#arch-gateway)"
        stroke={drift ? C.accentSalmon : C.nodeStroke}
        strokeWidth={drift ? 1.3 : 1}
      />
      {/* inner modules */}
      {compact ? (
        <g>
          {GATEWAY_MODULES.map((m, i) => (
            <ModuleCell
              key={m.title}
              x={x + 14 + (i % 2) * 150}
              y={y + 30 + Math.floor(i / 2) * 58}
              w={138}
              {...m}
            />
          ))}
        </g>
      ) : (
        <g>
          {GATEWAY_MODULES.map((m, i) => (
            <ModuleCell key={m.title} x={x + 16} y={y + 24 + i * 46} w={w - 32} {...m} />
          ))}
        </g>
      )}
    </g>
  )
}

function ModuleCell({
  x,
  y,
  w,
  title,
  sub,
}: {
  x: number
  y: number
  w: number
  title: string
  sub: string
}) {
  return (
    <g>
      <rect
        x={x}
        y={y}
        width={w}
        height={40}
        rx="5"
        fill={C.nodeBlackish}
        stroke={C.nodeStroke}
        strokeWidth="0.75"
      />
      <circle cx={x + 12} cy={y + 14} r="2.5" fill={C.accentGold} opacity="0.85" />
      <text
        x={x + 24}
        y={y + 17}
        fontFamily="'JetBrains Mono', monospace"
        fontSize="11.5"
        fill={C.headingText}
      >
        {title}
      </text>
      <text
        x={x + 24}
        y={y + 31}
        fontFamily="'JetBrains Mono', monospace"
        fontSize="8.5"
        fill={C.neutralText}
        opacity="0.55"
      >
        {sub}
      </text>
    </g>
  )
}

/* ── Side stores ─────────────────────────────────────────────────── */

function StoreBox({
  x,
  y,
  title,
  lines,
  note,
  wideLine,
}: {
  x: number
  y: number
  title: string
  lines: string[]
  note: string
  wideLine?: boolean
}) {
  const w = wideLine ? 150 : 162
  const h = 96
  return (
    <g>
      <rect
        x={x}
        y={y}
        width={w}
        height={h}
        rx="6"
        fill="url(#arch-node)"
        stroke={C.nodeStroke}
        strokeWidth="1"
        strokeDasharray="6 4"
      />
      <text
        x={x + 14}
        y={y + 24}
        fontFamily="'JetBrains Mono', monospace"
        fontSize="13"
        fill={C.headingText}
        letterSpacing="0.5"
      >
        {title}
      </text>
      {lines.map((l, i) => (
        <text
          key={l}
          x={x + 14}
          y={y + 44 + i * 15}
          fontFamily="'JetBrains Mono', monospace"
          fontSize="10"
          fill={C.neutralText}
          opacity="0.75"
        >
          {l}
        </text>
      ))}
      <text
        x={x + 14}
        y={y + h - 12}
        fontFamily="'JetBrains Mono', monospace"
        fontSize="8.5"
        fill={C.accentGold}
        opacity="0.75"
        letterSpacing="0.5"
      >
        {note}
      </text>
    </g>
  )
}

/* ── Providers ───────────────────────────────────────────────────── */

const PROVIDER_NODES = [
  { id: 'openai', name: 'openai', dot: '#10a37f' },
  { id: 'anthropic', name: 'anthropic', dot: '#d97757' },
  { id: 'google', name: 'google', dot: '#4285f4' },
  { id: 'xai', name: 'xai', dot: C.neutralText },
  { id: 'deepseek', name: 'deepseek', dot: '#536dfe' },
] as const

function ProviderStack({ x, y }: { x: number; y: number }) {
  return (
    <g>
      {PROVIDER_NODES.map((p, i) => (
        <g key={p.id}>
          <rect
            x={x}
            y={y + i * 48}
            width={192}
            height={40}
            rx="5"
            fill="url(#arch-node)"
            stroke={C.nodeStroke}
            strokeWidth="1"
          />
          <circle cx={x + 16} cy={y + i * 48 + 20} r="3.5" fill={p.dot} />
          <text
            x={x + 30}
            y={y + i * 48 + 25}
            fontFamily="'JetBrains Mono', monospace"
            fontSize="13"
            fill={C.headingText}
            letterSpacing="0.5"
          >
            {p.name}
          </text>
        </g>
      ))}
    </g>
  )
}

function ProviderChips({ x, y, w }: { x: number; y: number; w: number }) {
  return (
    <g>
      <rect x={x} y={y} width={w} height={76} rx="6" fill="url(#arch-node)" stroke={C.nodeStroke} strokeWidth="1" />
      <g>
        {PROVIDER_NODES.slice(0, 3).map((p, i) => (
          <ProviderPill key={p.id} x={x + 14 + i * 100} y={y + 14} p={p} />
        ))}
        {PROVIDER_NODES.slice(3).map((p, i) => (
          <ProviderPill key={p.id} x={x + 14 + i * 100} y={y + 44} p={p} />
        ))}
      </g>
    </g>
  )
}

function ProviderPill({
  x,
  y,
  p,
}: {
  x: number
  y: number
  p: { name: string; dot: string }
}) {
  return (
    <g>
      <circle cx={x + 8} cy={y + 11} r="3.5" fill={p.dot} />
      <text
        x={x + 18}
        y={y + 15}
        fontFamily="'JetBrains Mono', monospace"
        fontSize="11"
        fill={C.neutralText}
      >
        {p.name}
      </text>
    </g>
  )
}

/* ── Settlement plane ────────────────────────────────────────────── */

function SettlementPlane({
  x,
  y,
  w,
  h,
  compact,
}: {
  x: number
  y: number
  w: number
  h: number
  compact?: boolean
}) {
  return (
    <g>
      <rect
        x={x}
        y={y}
        width={w}
        height={h}
        rx="8"
        fill="url(#arch-plane)"
        stroke={C.nodeStroke}
        strokeWidth="1"
      />
      <text
        x={x + 18}
        y={y + 24}
        fontFamily="'JetBrains Mono', monospace"
        fontSize="11"
        fill={C.neutralText}
        opacity="0.6"
        letterSpacing="2"
      >
        SOLANA MAINNET · USDC-SPL
      </text>

      {compact ? (
        <g>
          <FlowChip
            x={x + 16}
            y={y + 40}
            w={w - 32}
            color={C.accentGold}
            title="exact"
            sub="deferred transfer"
          />
          <FlowChip
            x={x + 16}
            y={y + 90}
            w={w - 32}
            color={C.accentSalmon}
            title="escrow"
            sub="deposit → claim + refund"
          />
          <NodeMini
            x={x + 16}
            y={y + 140}
            w={(w - 44) / 2}
            title="escrow program"
            sub="PDA"
            dashed
          />
          <NodeMini
            x={x + 16 + (w - 44) / 2 + 12}
            y={y + 140}
            w={(w - 44) / 2}
            title="fee-payer pool"
            sub="sponsors tx"
          />
        </g>
      ) : (
        <g>
          {/* exact (gold) */}
          <FlowChip
            x={x + 24}
            y={y + 42}
            w={280}
            color={C.accentGold}
            title="exact"
            sub="deferred USDC-SPL transfer"
          />
          {/* escrow (salmon) */}
          <FlowChip
            x={x + 320}
            y={y + 42}
            w={300}
            color={C.accentSalmon}
            title="escrow"
            sub="deposit → claim + refund (1 tx)"
          />
          {/* on-chain accounts on the right */}
          <NodeMini x={x + 644} y={y + 36} w={210} title="escrow program" sub="PDA · anchor" dashed />
          <NodeMini x={x + 644} y={y + 70} w={210} title="fee-payer pool" sub="sponsors tx" />
        </g>
      )}
    </g>
  )
}

function FlowChip({
  x,
  y,
  w,
  color,
  title,
  sub,
}: {
  x: number
  y: number
  w: number
  color: string
  title: string
  sub: string
}) {
  return (
    <g>
      <rect
        x={x}
        y={y}
        width={w}
        height={40}
        rx="5"
        fill={C.nodeBlackish}
        stroke={color}
        strokeWidth="1"
        opacity="0.95"
      />
      <circle cx={x + 14} cy={y + 20} r="4" fill={color} />
      <text
        x={x + 28}
        y={y + 17}
        fontFamily="'JetBrains Mono', monospace"
        fontSize="12"
        fill={C.headingText}
        letterSpacing="0.5"
      >
        {title}
      </text>
      <text
        x={x + 28}
        y={y + 31}
        fontFamily="'JetBrains Mono', monospace"
        fontSize="9"
        fill={C.neutralText}
        opacity="0.7"
      >
        {sub}
      </text>
    </g>
  )
}

function NodeMini({
  x,
  y,
  w,
  title,
  sub,
  dashed,
}: {
  x: number
  y: number
  w: number
  title: string
  sub: string
  dashed?: boolean
}) {
  return (
    <g>
      <rect
        x={x}
        y={y}
        width={w}
        height={28}
        rx="4"
        fill={C.nodeFrontDark}
        stroke={dashed ? C.accentGold : C.nodeStroke}
        strokeWidth="1"
        strokeDasharray={dashed ? '5 4' : undefined}
      />
      <text
        x={x + 12}
        y={y + 13}
        fontFamily="'JetBrains Mono', monospace"
        fontSize="10.5"
        fill={C.headingText}
      >
        {title}
      </text>
      <text
        x={x + 12}
        y={y + 24}
        fontFamily="'JetBrains Mono', monospace"
        fontSize="8.5"
        fill={C.neutralText}
        opacity="0.6"
      >
        {sub}
      </text>
    </g>
  )
}

/* ── Small SVG helpers ───────────────────────────────────────────── */

function ArrowHead({
  x,
  y,
  dir,
  color,
}: {
  x: number
  y: number
  dir: 'right' | 'down'
  color: string
}) {
  const pts =
    dir === 'right'
      ? `${x},${y - 4} ${x + 6},${y} ${x},${y + 4}`
      : `${x - 4},${y - 6} ${x + 4},${y - 6} ${x},${y}`
  return <polygon points={pts} fill={color} opacity="0.7" />
}

function DriftPacket({
  pathId,
  color,
  dur,
  begin,
}: {
  pathId: string
  color: string
  dur: string
  begin: string
}) {
  return (
    <circle r="2.5" fill={color} opacity="0.85">
      <animateMotion dur={dur} begin={begin} repeatCount="indefinite" rotate="auto">
        <mpath href={pathId} />
      </animateMotion>
    </circle>
  )
}

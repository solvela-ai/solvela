'use client'

// Animated A2A discovery + payment flow diagram.
//
// Two actors face each other: a requesting AI agent (left) and the Solvela
// gateway (right). Five beats trace the canonical A2A x402 handshake, every
// label sourced verbatim from crates/gateway/src/a2a/{agent_card,types,handler}.rs:
//
//   0  GET /.well-known/agent-card.json  → AgentCard (capabilities + x402·ap2 ext)
//   1  message/send (text prompt)   → Task: input-required + x402.payment.required
//   2  agent signs USDC payment     (brief Solana chain-tick moment)
//   3  message/send (taskId + x402.payment.payload)
//   4  Task: completed + artifacts + x402.payment.receipts (tx_signature)
//
// House style mirrors escrow-diagram-animated.tsx: gradient cube surfaces,
// SMIL <animateMotion> packets that ride named <path>s, beat-gated transient
// groups remounted via a loopKey. Packets carry their label so a request/
// response method rides the wire rather than overprinting a cube face — the
// escrow diagram's sublabel-collision bug is avoided by keeping ALL captions
// outside the cube footprints.
//
// API:
//   <A2ADiagram />               self-runs on a timed loop, paused off-screen
//   <A2ADiagram beat={n} />      parent-driven (future scroll scenes)
// Reduced motion: renders the final `completed` frame, statically.

import { useEffect, useRef, useState } from 'react'

import { diagramPalette as C } from '@/lib/diagram-colors'

export type A2ABeat = 0 | 1 | 2 | 3 | 4

const LAST_BEAT: A2ABeat = 4

// Dwell per beat (ms). The signing beat (2) is the shortest — an abstract
// chain tick, not a transfer. The resolved frame (4) holds longest so the
// receipt is readable.
const BEAT_MS: Record<A2ABeat, number> = {
  0: 1500,
  1: 1600,
  2: 1100,
  3: 1600,
  4: 2400,
}

const LOOPS_BEFORE_HOLD = 3

interface Props {
  /** When provided, the parent drives the beat. When omitted, self-runs. */
  beat?: number
}

export function A2ADiagram({ beat: beatProp }: Props) {
  const selfDriven = beatProp === undefined

  const [beat, setBeat] = useState<A2ABeat>(0)
  const [loopKey, setLoopKey] = useState(0)
  const [loopCount, setLoopCount] = useState(0)
  const [isHolding, setIsHolding] = useState(false)
  const [inView, setInView] = useState(false)
  const [reduced, setReduced] = useState(false)
  const wrapperRef = useRef<HTMLDivElement>(null)

  // Reduced-motion preference: snap to the resolved frame and never advance.
  useEffect(() => {
    if (typeof window === 'undefined') return
    const mq = window.matchMedia('(prefers-reduced-motion: reduce)')
    const apply = () => {
      if (mq.matches) {
        setReduced(true)
        setBeat(LAST_BEAT)
        setIsHolding(true)
      } else {
        setReduced(false)
      }
    }
    apply()
    mq.addEventListener('change', apply)
    return () => mq.removeEventListener('change', apply)
  }, [])

  // Pause when scrolled out of view — resume from the current beat, no reset.
  useEffect(() => {
    if (!selfDriven) return
    const node = wrapperRef.current
    if (!node) return
    const io = new IntersectionObserver(
      (entries) => {
        for (const e of entries) setInView(e.isIntersecting)
      },
      { rootMargin: '200px 0px' },
    )
    io.observe(node)
    return () => io.disconnect()
  }, [selfDriven])

  // Self-run beat machine — one timeout per beat. Each new beat bumps loopKey
  // so the SMIL packets remount and replay from scratch.
  useEffect(() => {
    if (!selfDriven || reduced || !inView || isHolding) return
    const id = setTimeout(() => {
      if (beat === LAST_BEAT) {
        const next = loopCount + 1
        if (next >= LOOPS_BEFORE_HOLD) {
          setIsHolding(true)
        } else {
          setLoopCount(next)
          setBeat(0)
          setLoopKey((k) => k + 1)
        }
      } else {
        setBeat((b) => (b + 1) as A2ABeat)
        setLoopKey((k) => k + 1)
      }
    }, BEAT_MS[beat])
    return () => clearTimeout(id)
  }, [selfDriven, reduced, inView, isHolding, beat, loopCount])

  // Resolve the beat actually rendered. Parent-driven mode clamps the prop;
  // reduced motion always shows the resolved frame.
  const activeBeat: A2ABeat = reduced
    ? LAST_BEAT
    : selfDriven
      ? beat
      : (Math.max(0, Math.min(LAST_BEAT, Math.floor(beatProp ?? 0))) as A2ABeat)

  // loopKey only matters in self-run mode; parent-driven mode uses the beat
  // itself as the remount key so each parent step replays its packets.
  const effLoopKey = selfDriven ? loopKey : activeBeat

  return (
    <div ref={wrapperRef} className="h-full w-full">
      <A2ADiagramSvg beat={activeBeat} loopKey={effLoopKey} />
    </div>
  )
}

/* ── The SVG ─────────────────────────────────────────────────────────── */

interface SvgProps {
  beat: A2ABeat
  loopKey: number
}

// viewBox is generous in height so captions sit in clear bands above and below
// the two actors — nothing overprints a cube face at any width down to 320px.
function A2ADiagramSvg({ beat, loopKey }: SvgProps) {
  // The AgentCard tags read as "lit" once discovery has happened. They light
  // from the moment of the response in beat 0 and stay lit thereafter.
  const cardReturned = true
  const taskState: 'input-required' | 'completed' | null =
    beat >= 4 ? 'completed' : beat >= 1 ? 'input-required' : null

  return (
    <svg
      viewBox="0 0 640 380"
      role="img"
      aria-label={A11Y_LABEL}
      className="h-full w-full"
      preserveAspectRatio="xMidYMid meet"
    >
      <defs>
        <linearGradient id="a2a-agent" x1="0" x2="0" y1="0" y2="1">
          <stop offset="0" stopColor={C.nodeFrontMid} />
          <stop offset="1" stopColor={C.nodeFrontDark} />
        </linearGradient>
        <linearGradient id="a2a-gateway" x1="0" x2="0" y1="0" y2="1">
          <stop offset="0" stopColor={C.nodeTopSurface} />
          <stop offset="1" stopColor={C.nodeRightDark} />
        </linearGradient>

        {/* Named wire paths. Requests run agent→gateway (left→right) along the
            upper lane; responses run gateway→agent along the lower lane. All
            coordinates live in the 640×380 viewBox. */}
        <path id="a2a-req" d="M 168 150 C 250 132, 388 132, 470 150" fill="none" />
        <path id="a2a-res" d="M 470 214 C 388 232, 250 232, 168 214" fill="none" />
        {/* The signing tick is local to the agent — a short downward stub. */}
        <path id="a2a-sign" d="M 96 196 C 96 214, 96 226, 96 240" fill="none" />
      </defs>

      {/* Faint wire skeleton so the two lanes read even between packets. */}
      <g aria-hidden="true" fill="none" strokeDasharray="2 6" opacity="0.16">
        <use href="#a2a-req" stroke={C.neutralText} strokeWidth="1" />
        <use href="#a2a-res" stroke={C.neutralText} strokeWidth="1" />
      </g>

      {/* Lane direction hints — tiny static glyphs, decorative. */}
      <g aria-hidden="true" opacity="0.5">
        <text
          x="320"
          y="118"
          textAnchor="middle"
          fontFamily="'JetBrains Mono', monospace"
          fontSize="9"
          letterSpacing="2"
          fill={C.neutralText}
        >
          REQUEST →
        </text>
        <text
          x="320"
          y="258"
          textAnchor="middle"
          fontFamily="'JetBrains Mono', monospace"
          fontSize="9"
          letterSpacing="2"
          fill={C.neutralText}
        >
          ← RESPONSE
        </text>
      </g>

      {/* Actors */}
      <g aria-hidden="true">
        <Cube
          x={40}
          y={150}
          label="agent"
          fill="url(#a2a-agent)"
          dot={C.neutralText}
          highlight={beat === 0 || beat === 2 || beat === 3}
          pulse={beat === 2}
        />
        <Cube
          x={452}
          y={150}
          label="solvela"
          fill="url(#a2a-gateway)"
          dot={C.accentGold}
          highlight={beat === 1 || beat === 4}
        />
      </g>

      {/* Gateway status block — AgentCard tags, then the live Task state.
          Sits BELOW the gateway cube, clear of its face (cube bottom ≈ y226). */}
      <g transform="translate(506, 300)" aria-hidden="true">
        <text
          x="0"
          y="0"
          textAnchor="middle"
          fontFamily="'JetBrains Mono', monospace"
          fontSize="9"
          letterSpacing="1.5"
          fill={C.neutralText}
          opacity={cardReturned ? 0.85 : 0.3}
        >
          agent-card
        </text>
        {/* extension tags: x402 · ap2 */}
        <g transform="translate(0, 16)">
          <ExtTag x={-34} label="x402" lit={cardReturned} />
          <ExtTag x={18} label="ap2" lit={cardReturned} />
        </g>
      </g>

      {/* Task-state pill — the authoritative A2A lifecycle state, anchored under
          the gateway block so it never collides with the cube or the tags. */}
      <g transform="translate(506, 350)" aria-hidden="true">
        <TaskPill state={taskState} />
      </g>

      {/* Agent-side caption — what the agent holds after settlement. */}
      <g transform="translate(96, 300)" aria-hidden="true">
        <text
          x="0"
          y="0"
          textAnchor="middle"
          fontFamily="'JetBrains Mono', monospace"
          fontSize="9"
          letterSpacing="1.5"
          fill={C.neutralText}
          opacity="0.55"
        >
          {beat >= 4 ? 'ARTIFACTS' : beat === 2 ? 'SIGNING' : 'WALLET'}
        </text>
        <text
          x="0"
          y="18"
          textAnchor="middle"
          fontFamily="'Source Serif 4', Georgia, serif"
          fontSize="14"
          fontWeight="500"
          fill={beat >= 4 ? C.accentGold : C.neutralText}
        >
          {beat >= 4 ? 'received' : beat === 2 ? 'usdc-spl' : 'usdc'}
        </text>
      </g>

      {/* ── Beat-gated transient packets ──────────────────────────────── */}

      {/* Beat 0: discovery — GET request rides up, AgentCard rides back. */}
      {beat === 0 && (
        <g key={`b0-${loopKey}`} aria-hidden="true">
          <WirePacket
            pathId="#a2a-req"
            label="GET /.well-known/agent-card.json"
            color={C.neutralText}
            dur="1.0s"
          />
          <WirePacket
            pathId="#a2a-res"
            label="AgentCard"
            color={C.accentGold}
            dur="0.95s"
            begin="0.5s"
          />
        </g>
      )}

      {/* Beat 1: message/send (text prompt) up; input-required Task back. */}
      {beat === 1 && (
        <g key={`b1-${loopKey}`} aria-hidden="true">
          <WirePacket
            pathId="#a2a-req"
            label="message/send"
            color={C.neutralText}
            dur="1.0s"
          />
          <WirePacket
            pathId="#a2a-res"
            label="x402.payment.required"
            color={C.accentSalmon}
            dur="0.95s"
            begin="0.55s"
          />
        </g>
      )}

      {/* Beat 2: agent signs — a brief Solana chain tick, abstract. */}
      {beat === 2 && (
        <g key={`b2-${loopKey}`} aria-hidden="true">
          {/* one-shot ring pulse around the agent */}
          <circle
            cx={96}
            cy={188}
            r="18"
            fill="none"
            stroke={C.accentGold}
            strokeWidth="1.5"
          >
            <animate attributeName="r" from="18" to="60" dur="0.95s" fill="freeze" />
            <animate
              attributeName="opacity"
              from="0.7"
              to="0"
              dur="0.95s"
              fill="freeze"
            />
          </circle>
          {/* chain tick travels down the local sign stub */}
          <ChainTick pathId="#a2a-sign" dur="0.85s" />
          <text
            x="96"
            y="256"
            textAnchor="middle"
            fontFamily="'JetBrains Mono', monospace"
            fontSize="10"
            letterSpacing="0.5"
            fill={C.accentGold}
          >
            sign · solana
            <animate
              attributeName="opacity"
              from="0"
              to="1"
              dur="0.4s"
              fill="freeze"
            />
          </text>
        </g>
      )}

      {/* Beat 3: message/send (taskId + x402.payment.payload) up. */}
      {beat === 3 && (
        <g key={`b3-${loopKey}`} aria-hidden="true">
          <WirePacket
            pathId="#a2a-req"
            label="message/send · taskId"
            color={C.neutralText}
            dur="1.0s"
          />
          <WirePacket
            pathId="#a2a-req"
            label="x402.payment.payload"
            color={C.accentGold}
            dur="1.0s"
            begin="0.18s"
            offsetY={16}
          />
        </g>
      )}

      {/* Beat 4: completed Task back — artifacts + receipt (tx signature). */}
      {beat === 4 && (
        <g key={`b4-${loopKey}`} aria-hidden="true">
          <WirePacket
            pathId="#a2a-res"
            label="completed · artifacts"
            color={C.neutralText}
            dur="1.0s"
          />
          <WirePacket
            pathId="#a2a-res"
            label="x402.payment.receipts"
            color={C.accentGold}
            dur="1.0s"
            begin="0.18s"
            offsetY={16}
          />
        </g>
      )}
    </svg>
  )
}

const A11Y_LABEL =
  'Agent-to-agent discovery and payment flow. A requesting agent fetches the ' +
  'Solvela AgentCard at GET /.well-known/agent-card.json, with x402 and ap2 ' +
  'extensions. It calls message/send, receiving a Task in the input-required ' +
  'state with x402.payment.required metadata. The agent signs a USDC payment ' +
  'on Solana, then calls message/send again with the taskId and ' +
  'x402.payment.payload. Solvela returns a completed Task with artifacts and ' +
  'x402.payment.receipts containing the transaction signature.'

/* ── Packet components ───────────────────────────────────────────────── */

interface WirePacketProps {
  pathId: string
  label: string
  color: string
  dur: string
  begin?: string
  /** Vertical offset for the label so two concurrent packets don't overlap. */
  offsetY?: number
}

// A dot that rides a wire path, trailing its method label. The label rides the
// same path (so it tracks the dot) and is offset above the wire — never landing
// on a cube face. Begin can be staggered for request/response pairs.
function WirePacket({
  pathId,
  label,
  color,
  dur,
  begin = '0s',
  offsetY = 0,
}: WirePacketProps) {
  return (
    <g>
      <circle r="5" fill={color}>
        <animateMotion dur={dur} begin={begin} fill="freeze">
          <mpath href={pathId} />
        </animateMotion>
      </circle>
      <text
        y={-11 - offsetY}
        textAnchor="middle"
        fontFamily="'JetBrains Mono', monospace"
        fontSize="11"
        fontWeight="500"
        fill={color}
      >
        {label}
        <animateMotion dur={dur} begin={begin} fill="freeze">
          <mpath href={pathId} />
        </animateMotion>
      </text>
    </g>
  )
}

// The Solana chain-tick: a small square (block) that ticks down the sign stub.
function ChainTick({ pathId, dur }: { pathId: string; dur: string }) {
  return (
    <rect x="-4" y="-4" width="8" height="8" rx="1.5" fill={C.accentGold}>
      <animateMotion dur={dur} begin="0s" fill="freeze" rotate="auto">
        <mpath href={pathId} />
      </animateMotion>
    </rect>
  )
}

/* ── Static label helpers ────────────────────────────────────────────── */

// A small extension-capability tag (x402 / ap2) drawn under the AgentCard label.
function ExtTag({ x, label, lit }: { x: number; label: string; lit: boolean }) {
  const w = label.length * 7 + 14
  return (
    <g transform={`translate(${x}, 0)`}>
      <rect
        x="0"
        y="-9"
        width={w}
        height="16"
        rx="3"
        fill="none"
        stroke={lit ? C.accentGold : C.nodeStroke}
        strokeWidth="1"
        opacity={lit ? 0.9 : 0.4}
      />
      <text
        x={w / 2}
        y="2"
        textAnchor="middle"
        fontFamily="'JetBrains Mono', monospace"
        fontSize="9"
        letterSpacing="0.5"
        fill={lit ? C.accentGold : C.neutralText}
        opacity={lit ? 1 : 0.5}
      >
        {label}
      </text>
    </g>
  )
}

// The A2A Task lifecycle pill. `null` = no task yet (pre-discovery hold).
function TaskPill({ state }: { state: 'input-required' | 'completed' | null }) {
  if (state === null) {
    return (
      <text
        x="0"
        y="2"
        textAnchor="middle"
        fontFamily="'JetBrains Mono', monospace"
        fontSize="9"
        letterSpacing="1.5"
        fill={C.neutralText}
        opacity="0.3"
      >
        no task
      </text>
    )
  }
  const done = state === 'completed'
  const color = done ? C.accentGold : C.accentSalmon
  const w = state.length * 7 + 26
  return (
    <g>
      <rect
        x={-w / 2}
        y="-11"
        width={w}
        height="20"
        rx="4"
        fill="none"
        stroke={color}
        strokeWidth="1.2"
        opacity="0.9"
      />
      <circle cx={-w / 2 + 11} cy="-1" r="2.5" fill={color} />
      <text
        x={6}
        y="3"
        textAnchor="middle"
        fontFamily="'JetBrains Mono', monospace"
        fontSize="10"
        letterSpacing="0.5"
        fill={color}
      >
        {state}
      </text>
    </g>
  )
}

/* ── Isometric cube actor (mirrors escrow-diagram-animated) ──────────── */

interface CubeProps {
  x: number
  y: number
  label: string
  fill: string
  dot: string
  highlight?: boolean
  pulse?: boolean
}

function Cube({ x, y, label, fill, dot, highlight, pulse }: CubeProps) {
  const w = 96
  const d = 46
  const h = 56

  const stroke = highlight ? C.accentGold : C.nodeStroke
  const strokeWidth = highlight ? 1.4 : 1

  const ax = x
  const ay = y + h

  const tfl = [ax, ay - h]
  const tfr = [ax + w, ay - h]
  const tbr = [ax + w + d * 0.6, ay - h - d * 0.5]
  const tbl = [ax + d * 0.6, ay - h - d * 0.5]
  const rbr = [ax + w + d * 0.6, ay - d * 0.5]

  return (
    <g style={pulse ? { filter: `drop-shadow(0 0 6px ${C.accentGold})` } : undefined}>
      <polygon
        points={`${ax},${ay} ${ax + w},${ay} ${tfr[0]},${tfr[1]} ${tfl[0]},${tfl[1]}`}
        fill={fill}
        stroke={stroke}
        strokeWidth={strokeWidth}
      />
      <polygon
        points={`${ax + w},${ay} ${rbr[0]},${rbr[1]} ${tbr[0]},${tbr[1]} ${tfr[0]},${tfr[1]}`}
        fill={C.nodeRightDark}
        stroke={stroke}
        strokeWidth={strokeWidth}
        opacity="0.88"
      />
      <polygon
        points={`${tfl[0]},${tfl[1]} ${tfr[0]},${tfr[1]} ${tbr[0]},${tbr[1]} ${tbl[0]},${tbl[1]}`}
        fill={C.nodeTopSurface}
        stroke={stroke}
        strokeWidth={strokeWidth}
      />
      <circle cx={ax + 13} cy={ay - h + 13} r="3" fill={dot} />
      <text
        x={ax + 13}
        y={ay - 20}
        fontFamily="'JetBrains Mono', monospace"
        fontSize="14"
        fill={C.headingText}
        letterSpacing="1"
      >
        {label}
      </text>
    </g>
  )
}

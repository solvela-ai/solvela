'use client'

// Animated escrow-flow diagram. Driven by the `beat` prop from <EscrowSequence>.
// Packets are rendered only during their active beat and animated via SMIL
// <animateMotion> — native to SVG, no CSS motion-path quirks, no JS tween loop.
// A `loopKey` prop forces remounts on each new loop so the animation replays.

import { diagramPalette as C } from '@/lib/diagram-colors'

export type EscrowBeat = 0 | 1 | 2 | 3 | 4

interface Props {
  beat: EscrowBeat
  loopKey: number
}

// Balance strings chosen so every digit is meaningful:
//   starting balance:            12.4000
//   after deposit (−0.0042):     12.3958
//   after refund  (+0.0004):     12.3962
const AGENT_START = '12.4000'
const AGENT_AFTER_DEPOSIT = '12.3958'
const AGENT_AFTER_REFUND = '12.3962'
const DEPOSIT_AMOUNT = '0.0042'
const CLAIM_AMOUNT = '0.0038'
const REFUND_AMOUNT = '0.0004'

export function EscrowDiagramAnimated({ beat, loopKey }: Props) {
  const agentBalance =
    beat === 0 ? AGENT_START : beat >= 4 ? AGENT_AFTER_REFUND : AGENT_AFTER_DEPOSIT

  const escrowBalance = beat === 0 || beat >= 4 ? '0.0000' : DEPOSIT_AMOUNT
  const providerCredit = beat >= 4 ? CLAIM_AMOUNT : '0.0000'

  return (
    <svg
      viewBox="0 0 640 340"
      role="img"
      aria-labelledby="escrow-anim-title escrow-anim-desc"
      className="h-full w-full"
      preserveAspectRatio="xMidYMid meet"
    >
      <title id="escrow-anim-title">Escrow payment flow</title>
      <desc id="escrow-anim-desc">
        The agent deposits {DEPOSIT_AMOUNT} USDC into an on-chain escrow. The
        provider streams a response. In the same transaction, the escrow claims
        the delivered {CLAIM_AMOUNT} to the provider and refunds the unused
        {' '}
        {REFUND_AMOUNT} to the agent.
      </desc>

      <defs>
        <linearGradient id="esc-wallet" x1="0" x2="0" y1="0" y2="1">
          <stop offset="0" stopColor={C.nodeFrontMid} />
          <stop offset="1" stopColor={C.nodeFrontDark} />
        </linearGradient>
        <linearGradient id="esc-escrow" x1="0" x2="0" y1="0" y2="1">
          <stop offset="0" stopColor={C.nodeTopSurface} />
          <stop offset="1" stopColor={C.nodeFrontDark} />
        </linearGradient>
        <linearGradient id="esc-provider" x1="0" x2="0" y1="0" y2="1">
          <stop offset="0" stopColor={C.nodeFrontMid} />
          <stop offset="1" stopColor={C.nodeRightDark} />
        </linearGradient>

        {/* Named paths — packets ride these via <mpath>. All coordinates are
            in the 640×340 viewBox. */}
        <path
          id="path-deposit"
          d="M 160 160 C 200 176, 240 190, 290 196"
          fill="none"
        />
        <path
          id="path-stream"
          d="M 496 120 C 430 140, 260 140, 160 112"
          fill="none"
        />
        <path
          id="path-claim"
          d="M 388 196 C 430 186, 458 172, 488 154"
          fill="none"
        />
        <path
          id="path-refund"
          d="M 288 232 C 250 248, 214 248, 172 228"
          fill="none"
        />
      </defs>

      {/* Dotted guide paths — very faint, give viewers a hint of the flow
          skeleton without competing with active packets. */}
      <g fill="none" strokeDasharray="2 5" opacity="0.18">
        <use href="#path-deposit" stroke={C.neutralText} strokeWidth="1" />
        <use href="#path-claim" stroke={C.accentGold} strokeWidth="1" />
        <use href="#path-refund" stroke={C.accentSalmon} strokeWidth="1" />
      </g>

      {/* Nodes */}
      <Cube
        x={62}
        y={88}
        label="agent wallet"
        fill="url(#esc-wallet)"
        dot={C.neutralText}
        highlight={beat === 4}
      />
      <Cube
        x={268}
        y={138}
        label="escrow pda"
        sublabel="9neDHouX…"
        fill="url(#esc-escrow)"
        dot={C.accentGold}
        dashed
        highlight={beat === 1 || beat === 3}
        pulse={beat === 3}
      />
      <Cube
        x={470}
        y={88}
        label="provider"
        fill="url(#esc-provider)"
        dot={C.accentSalmon}
        highlight={beat === 2 || beat === 4}
      />

      {/* Balance labels below each cube */}
      <BalanceLabel x={116} y={238} label="balance" value={agentBalance} />
      <BalanceLabel
        x={322}
        y={288}
        label="held"
        value={escrowBalance}
        valueColor={beat === 1 || beat === 2 || beat === 3 ? C.accentGold : C.neutralText}
      />
      <BalanceLabel
        x={524}
        y={238}
        label="credited"
        value={providerCredit}
        valueColor={providerCredit === '0.0000' ? C.neutralText : C.accentGold}
      />

      {/* Packets — transient; rendered only during their active beat.
          The loopKey on the wrapping group forces a remount each loop so
          the SMIL animations replay from scratch. */}
      {beat === 1 && (
        <g key={`deposit-${loopKey}`}>
          <AmountPacket
            pathId="#path-deposit"
            amount={DEPOSIT_AMOUNT}
            color={C.neutralText}
            dur="1.2s"
          />
        </g>
      )}
      {beat === 2 && (
        <g key={`stream-${loopKey}`} opacity="0.7">
          <StreamParticle pathId="#path-stream" dur="0.9s" begin="0s" />
          <StreamParticle pathId="#path-stream" dur="0.9s" begin="0.15s" />
          <StreamParticle pathId="#path-stream" dur="0.9s" begin="0.30s" />
          <StreamParticle pathId="#path-stream" dur="0.9s" begin="0.45s" />
        </g>
      )}
      {beat === 3 && (
        <g key={`split-${loopKey}`}>
          {/* Same-tx ring pulse around escrow — one-shot */}
          <circle cx={322} cy={200} r="20" fill="none" stroke={C.accentGold} strokeWidth="1.5">
            <animate
              attributeName="r"
              from="20"
              to="72"
              dur="0.95s"
              begin="0s"
              fill="freeze"
            />
            <animate
              attributeName="opacity"
              from="0.75"
              to="0"
              dur="0.95s"
              begin="0s"
              fill="freeze"
            />
          </circle>

          {/* Claim (gold) and Refund (salmon) fire simultaneously. */}
          <AmountPacket
            pathId="#path-claim"
            amount={CLAIM_AMOUNT}
            color={C.accentGold}
            dur="0.85s"
          />
          <AmountPacket
            pathId="#path-refund"
            amount={REFUND_AMOUNT}
            color={C.accentSalmon}
            dur="0.85s"
          />
        </g>
      )}
    </svg>
  )
}

/* ── Packet components ───────────────────────────────────────────── */

interface AmountPacketProps {
  pathId: string
  amount: string
  color: string
  dur: string
}

function AmountPacket({ pathId, amount, color, dur }: AmountPacketProps) {
  return (
    <g>
      <circle r="5.5" fill={color}>
        <animateMotion dur={dur} begin="0s" fill="freeze" rotate="auto">
          <mpath href={pathId} />
        </animateMotion>
      </circle>
      <text
        y="-10"
        textAnchor="middle"
        fontFamily="'JetBrains Mono', monospace"
        fontSize="11"
        fontWeight="500"
        fill={color}
      >
        {amount}
        <animateMotion dur={dur} begin="0s" fill="freeze">
          <mpath href={pathId} />
        </animateMotion>
      </text>
    </g>
  )
}

interface StreamParticleProps {
  pathId: string
  dur: string
  begin: string
}

function StreamParticle({ pathId, dur, begin }: StreamParticleProps) {
  return (
    <circle r="1.75" fill={C.neutralText}>
      <animateMotion dur={dur} begin={begin} fill="remove">
        <mpath href={pathId} />
      </animateMotion>
    </circle>
  )
}

/* ── Static label helpers ────────────────────────────────────────── */

interface BalanceLabelProps {
  x: number
  y: number
  label: string
  value: string
  valueColor?: string
}

function BalanceLabel({ x, y, label, value, valueColor = C.neutralText }: BalanceLabelProps) {
  return (
    <g transform={`translate(${x}, ${y})`}>
      <text
        x="0"
        y="0"
        textAnchor="middle"
        fontFamily="'JetBrains Mono', monospace"
        fontSize="9"
        fill={C.neutralText}
        opacity="0.55"
        letterSpacing="1.5"
      >
        {label.toUpperCase()}
      </text>
      <text
        x="0"
        y="14"
        textAnchor="middle"
        fontFamily="'Source Serif 4', Georgia, serif"
        fontSize="16"
        fontWeight="500"
        fill={valueColor}
      >
        {value}
      </text>
    </g>
  )
}

interface CubeProps {
  x: number
  y: number
  label: string
  sublabel?: string
  fill: string
  dot: string
  dashed?: boolean
  highlight?: boolean
  pulse?: boolean
}

function Cube({ x, y, label, sublabel, fill, dot, dashed, highlight, pulse }: CubeProps) {
  const w = 108
  const d = 54
  const h = 62

  const baseStroke = dashed ? C.accentGold : C.nodeStroke
  const stroke = highlight ? C.accentGold : baseStroke
  const strokeDash = dashed ? '5 4' : undefined
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
        strokeDasharray={strokeDash}
      />
      <polygon
        points={`${ax + w},${ay} ${rbr[0]},${rbr[1]} ${tbr[0]},${tbr[1]} ${tfr[0]},${tfr[1]}`}
        fill={C.nodeRightDark}
        stroke={stroke}
        strokeWidth={strokeWidth}
        strokeDasharray={strokeDash}
        opacity="0.88"
      />
      <polygon
        points={`${tfl[0]},${tfl[1]} ${tfr[0]},${tfr[1]} ${tbr[0]},${tbr[1]} ${tbl[0]},${tbl[1]}`}
        fill={C.nodeTopSurface}
        stroke={stroke}
        strokeWidth={strokeWidth}
        strokeDasharray={strokeDash}
      />
      <circle cx={ax + 14} cy={ay - h + 14} r="3" fill={dot} />
      <text
        x={ax + 14}
        y={ay - 26}
        fontFamily="'JetBrains Mono', monospace"
        fontSize="14"
        fill={C.headingText}
        letterSpacing="1"
      >
        {label}
      </text>
      {sublabel && (
        <text
          x={ax + 14}
          y={ay - 10}
          fontFamily="'JetBrains Mono', monospace"
          fontSize="11"
          fill={C.neutralText}
          opacity="0.7"
          letterSpacing="1"
        >
          {sublabel}
        </text>
      )}
    </g>
  )
}

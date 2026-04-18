// Pure SVG isometric diagram with draw-in animation.
// Arrows carry pathLength=1 so stroke-dash is normalized regardless of path length.
// Labels positioned to sit >= 16px from any cube edge to avoid edge-tension.

export function EscrowDiagram() {
  return (
    <svg
      viewBox="0 0 640 340"
      fill="none"
      xmlns="http://www.w3.org/2000/svg"
      aria-hidden
      className="h-full w-full"
      preserveAspectRatio="xMidYMid meet"
    >
      <defs>
        <linearGradient id="iso-wallet" x1="0" x2="0" y1="0" y2="1">
          <stop offset="0" stopColor="#30302E" />
          <stop offset="1" stopColor="#262624" />
        </linearGradient>
        <linearGradient id="iso-escrow" x1="0" x2="0" y1="0" y2="1">
          <stop offset="0" stopColor="#3A3936" />
          <stop offset="1" stopColor="#262624" />
        </linearGradient>
        <linearGradient id="iso-provider" x1="0" x2="0" y1="0" y2="1">
          <stop offset="0" stopColor="#30302E" />
          <stop offset="1" stopColor="#1F1E1D" />
        </linearGradient>
        <marker
          id="arrow-salmon"
          viewBox="0 0 10 10"
          refX="9"
          refY="5"
          markerWidth="6"
          markerHeight="6"
          orient="auto-start-reverse"
        >
          <path d="M 0 0 L 10 5 L 0 10 z" fill="#FE8181" />
        </marker>
        <marker
          id="arrow-gold"
          viewBox="0 0 10 10"
          refX="9"
          refY="5"
          markerWidth="6"
          markerHeight="6"
          orient="auto-start-reverse"
        >
          <path d="M 0 0 L 10 5 L 0 10 z" fill="#C8A240" />
        </marker>
        <marker
          id="arrow-neutral"
          viewBox="0 0 10 10"
          refX="9"
          refY="5"
          markerWidth="6"
          markerHeight="6"
          orient="auto-start-reverse"
        >
          <path d="M 0 0 L 10 5 L 0 10 z" fill="#DEDCD1" opacity="0.75" />
        </marker>
      </defs>

      {/* grid floor — light, receding */}
      <g opacity="0.14" stroke="#DEDCD1" strokeWidth="0.5">
        {Array.from({ length: 14 }).map((_, i) => (
          <line
            key={`h-${i}`}
            x1={i * 48}
            y1={0}
            x2={i * 48 - 360}
            y2={340}
          />
        ))}
        {Array.from({ length: 14 }).map((_, i) => (
          <line
            key={`v-${i}`}
            x1={i * 48 + 120}
            y1={0}
            x2={i * 48 + 480}
            y2={340}
          />
        ))}
      </g>

      {/* ---- cubes ----
          agent:    top-left
          escrow:   center, lower (emphasized)
          provider: top-right
      */}
      <Cube x={62} y={88} label="agent wallet" fill="url(#iso-wallet)" dot="#DEDCD1" />
      <Cube
        x={268}
        y={138}
        label="escrow pda"
        sublabel="9neDHouX…"
        fill="url(#iso-escrow)"
        dot="#C8A240"
        dashed
      />
      <Cube x={470} y={88} label="provider" fill="url(#iso-provider)" dot="#FE8181" />

      {/* ---- arrows ---- */}

      {/* 1. DEPOSIT — agent → escrow */}
      <g>
        <path
          className="iso-arrow iso-arrow-1"
          pathLength="1"
          d="M 170 166 C 210 180, 240 190, 280 198"
          stroke="#DEDCD1"
          strokeWidth="1.5"
          strokeDasharray="4 4"
          opacity="0.7"
          markerEnd="url(#arrow-neutral)"
        />
        <text
          className="iso-label iso-label-1"
          x="220"
          y="178"
          textAnchor="middle"
          fontFamily="'JetBrains Mono', monospace"
          fontSize="10.5"
          fill="#DEDCD1"
          opacity="0.85"
          letterSpacing="2"
        >
          DEPOSIT
        </text>
      </g>

      {/* 2. CLAIM — escrow → provider */}
      <g>
        <path
          className="iso-arrow iso-arrow-2"
          pathLength="1"
          d="M 410 200 C 438 190, 458 175, 478 160"
          stroke="#C8A240"
          strokeWidth="1.8"
          markerEnd="url(#arrow-gold)"
        />
        <text
          className="iso-label iso-label-2"
          x="448"
          y="178"
          textAnchor="middle"
          fontFamily="'JetBrains Mono', monospace"
          fontSize="10.5"
          fill="#C8A240"
          letterSpacing="2"
        >
          CLAIM
        </text>
      </g>

      {/* 3. REFUND — escrow → agent (unclaimed returns) */}
      <g>
        <path
          className="iso-arrow iso-arrow-3"
          pathLength="1"
          d="M 268 238 C 232 258, 208 258, 172 234"
          stroke="#FE8181"
          strokeWidth="1.8"
          markerEnd="url(#arrow-salmon)"
        />
        <text
          className="iso-label iso-label-3"
          x="220"
          y="272"
          textAnchor="middle"
          fontFamily="'JetBrains Mono', monospace"
          fontSize="10.5"
          fill="#FE8181"
          letterSpacing="2"
        >
          REFUND
        </text>
      </g>

      {/* 4. STREAM — provider → agent (large overhead arc) */}
      <g>
        <path
          className="iso-arrow iso-arrow-4"
          pathLength="1"
          d="M 500 120 C 440 60, 230 50, 138 108"
          stroke="#DEDCD1"
          strokeWidth="1"
          strokeDasharray="2 4"
          opacity="0.55"
          markerEnd="url(#arrow-neutral)"
        />
        <text
          className="iso-label iso-label-4"
          x="320"
          y="58"
          textAnchor="middle"
          fontFamily="'JetBrains Mono', monospace"
          fontSize="10.5"
          fill="#DEDCD1"
          opacity="0.7"
          letterSpacing="2"
        >
          STREAM
        </text>
      </g>
    </svg>
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
}

function Cube({ x, y, label, sublabel, fill, dot, dashed }: CubeProps) {
  const w = 108 // along x
  const d = 54  // depth (foreshortened)
  const h = 62  // height

  const stroke = dashed ? '#C8A240' : '#4a4a48'
  const strokeDash = dashed ? '5 4' : undefined

  const ax = x
  const ay = y + h

  const tfl = [ax, ay - h]
  const tfr = [ax + w, ay - h]
  const tbr = [ax + w + d * 0.6, ay - h - d * 0.5]
  const tbl = [ax + d * 0.6, ay - h - d * 0.5]

  const rbr = [ax + w + d * 0.6, ay - d * 0.5]

  return (
    <g>
      {/* front face */}
      <polygon
        points={`${ax},${ay} ${ax + w},${ay} ${tfr[0]},${tfr[1]} ${tfl[0]},${tfl[1]}`}
        fill={fill}
        stroke={stroke}
        strokeWidth="1"
        strokeDasharray={strokeDash}
      />
      {/* right face */}
      <polygon
        points={`${ax + w},${ay} ${rbr[0]},${rbr[1]} ${tbr[0]},${tbr[1]} ${tfr[0]},${tfr[1]}`}
        fill="#1F1E1D"
        stroke={stroke}
        strokeWidth="1"
        strokeDasharray={strokeDash}
        opacity="0.88"
      />
      {/* top face */}
      <polygon
        points={`${tfl[0]},${tfl[1]} ${tfr[0]},${tfr[1]} ${tbr[0]},${tbr[1]} ${tbl[0]},${tbl[1]}`}
        fill="#3A3936"
        stroke={stroke}
        strokeWidth="1"
        strokeDasharray={strokeDash}
      />
      {/* indicator dot */}
      <circle cx={ax + 14} cy={ay - h + 14} r="3" fill={dot} />
      {/* label */}
      <text
        x={ax + 14}
        y={ay - 24}
        fontFamily="'JetBrains Mono', monospace"
        fontSize="11"
        fill="#FAF9F5"
        letterSpacing="1"
      >
        {label}
      </text>
      {sublabel && (
        <text
          x={ax + 14}
          y={ay - 10}
          fontFamily="'JetBrains Mono', monospace"
          fontSize="9"
          fill="#DEDCD1"
          opacity="0.65"
          letterSpacing="1"
        >
          {sublabel}
        </text>
      )}
    </g>
  )
}

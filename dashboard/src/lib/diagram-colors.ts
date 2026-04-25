// Shared palette for inline SVG diagrams (escrow flow, org tree, etc.).
// SVG attribute values (fill, stroke, stop-color) don't resolve CSS variables,
// so these constants mirror the token palette for diagram-only use.
// Keep aligned with globals.css `:root,.dark` values.

export const diagramPalette = {
  nodeTopSurface: '#3A3936',
  nodeFrontMid: '#30302E',
  nodeFrontDark: '#262624',
  nodeRightDark: '#1F1E1D',
  nodeBlackish: '#141413', // matches --popover — used for emphasis fills
  nodeStroke: '#4a4a48',
  accentGold: '#C8A240',
  accentSalmon: '#FE8181',
  neutralText: '#DEDCD1',
  headingText: '#FAF9F5',
} as const

export type DiagramPalette = typeof diagramPalette

/**
 * Shared renderer for dynamic Open Graph images (1200×630).
 *
 * Deliberately uses no external network fetches at request time and no
 * web-font file load: next/font packages fetch from Google at *build* time
 * and ship no .woff2 we can read with fs at runtime, and pulling a font over
 * the network inside the image route would make OG generation flaky. The
 * brand reads through layout, scale, and the salmon/gold palette instead of
 * font fidelity — reliability beats a serif here.
 *
 * Palette mirrors src/app/globals.css:
 *   bg #262624 · heading #FAF9F5 · body #DEDCD1 · salmon #FE8181 · gold #C8A240
 */
import { ImageResponse } from 'next/og'

export const OG_SIZE = { width: 1200, height: 630 }
export const OG_CONTENT_TYPE = 'image/png'

const BG = '#262624'
const HEADING = '#FAF9F5'
const SALMON = '#FE8181'
const GOLD = '#C8A240'
const FAINT = '#807d72'

interface OgArgs {
  /** Mono eyebrow, rendered lowercase in salmon. */
  eyebrow: string
  /** Large editorial title. */
  title: string
  /** Optional kicker shown above the wordmark footer (e.g. a doc section). */
  kicker?: string
}

/**
 * Build the 1200×630 ImageResponse. Returned by each route's GET handler.
 */
export function renderOgImage({ eyebrow, title, kicker }: OgArgs): ImageResponse {
  // Scale the title down for long strings so doc titles don't overflow.
  const titleSize = title.length > 46 ? 64 : title.length > 30 ? 76 : 92

  return new ImageResponse(
    (
      <div
        style={{
          width: '100%',
          height: '100%',
          display: 'flex',
          flexDirection: 'column',
          justifyContent: 'space-between',
          backgroundColor: BG,
          // 56px grid, faded — echoes the landing hero backdrop.
          backgroundImage:
            'linear-gradient(90deg, rgba(222,220,209,0.05) 1px, transparent 1px), linear-gradient(rgba(222,220,209,0.05) 1px, transparent 1px)',
          backgroundSize: '56px 56px',
          padding: '72px 80px',
          position: 'relative',
        }}
      >
        {/* salmon-tipped top rule */}
        <div
          style={{
            position: 'absolute',
            top: 0,
            left: 0,
            right: 0,
            height: 6,
            display: 'flex',
            backgroundColor: SALMON,
          }}
        />

        {/* eyebrow */}
        <div
          style={{
            display: 'flex',
            alignItems: 'center',
            fontSize: 26,
            letterSpacing: 6,
            textTransform: 'uppercase',
            color: SALMON,
            fontFamily: 'monospace',
          }}
        >
          {eyebrow}
        </div>

        {/* title block */}
        <div style={{ display: 'flex', flexDirection: 'column', gap: 18, maxWidth: 1000 }}>
          {kicker ? (
            <div
              style={{
                display: 'flex',
                fontSize: 28,
                letterSpacing: 3,
                textTransform: 'uppercase',
                color: GOLD,
                fontFamily: 'monospace',
              }}
            >
              {kicker}
            </div>
          ) : null}
          <div
            style={{
              display: 'flex',
              fontSize: titleSize,
              lineHeight: 1.02,
              fontWeight: 700,
              letterSpacing: -2,
              color: HEADING,
            }}
          >
            {title}
          </div>
        </div>

        {/* footer wordmark: red dot + solvela + domain */}
        <div
          style={{
            display: 'flex',
            alignItems: 'center',
            justifyContent: 'space-between',
            borderTop: `1px solid ${GOLD}40`,
            paddingTop: 28,
          }}
        >
          <div style={{ display: 'flex', alignItems: 'center', gap: 16 }}>
            <div
              style={{
                width: 18,
                height: 18,
                borderRadius: 9,
                backgroundColor: SALMON,
                display: 'flex',
              }}
            />
            <div style={{ display: 'flex', fontSize: 40, fontWeight: 600, color: HEADING }}>
              solvela
            </div>
          </div>
          <div
            style={{
              display: 'flex',
              fontSize: 24,
              letterSpacing: 2,
              color: FAINT,
              fontFamily: 'monospace',
            }}
          >
            solana-native · usdc settled
          </div>
        </div>
      </div>
    ),
    { ...OG_SIZE },
  )
}

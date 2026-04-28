/**
 * Solvela Docs — Theme Configuration
 *
 * Colors from ColorsForDocs.ini + Brutalism layout.
 */

export const siteConfig = {
  name: 'Solvela',
  description: 'Solana-native AI agent payment gateway — x402 protocol, USDC-SPL micropayments',
  url: 'https://docs.solvela.ai',

  logo: {
    src: '/logo.svg',
    alt: 'Solvela',
    width: 40,
    height: 40,
  },

  links: {
    github: 'https://github.com/solvela-ai/solvela',
    discord: '',
    twitter: '',
    support: '',
  },

  footer: {
    companyName: 'Solvela',
    links: [
      { label: 'Solvela', href: 'https://solvela.ai' },
      { label: 'GitHub', href: 'https://github.com/solvela-ai/solvela' },
    ],
  },
}

export const themeConfig = {
  colors: {
    light: {
      accent: '#262624',
      accentForeground: '#F5F4F0',
      accentMuted: 'rgba(38, 38, 36, 0.06)',
    },
    dark: {
      accent: '#DEDCD1',
      accentForeground: '#262624',
      accentMuted: 'rgba(222, 220, 209, 0.08)',
    },
  },

  codeBlock: {
    light: {
      background: '#EAEAE6',
      titleBar: '#E0E0DC',
    },
    dark: {
      background: '#30302E',
      titleBar: '#1F1E1D',
    },
  },

  ogImage: {
    gradient: '#262624',
    titleColor: '#FAF9F5',
    sectionColor: '#DEDCD180',
    logoUrl: 'https://solvela.ai/logo.png',
  },
}

// Export CSS variable values for use in Tailwind
export function getCSSVariables(mode: 'light' | 'dark') {
  const colors = themeConfig.colors[mode]
  return {
    '--accent': colors.accent,
    '--accent-foreground': colors.accentForeground,
    '--accent-muted': colors.accentMuted,
  }
}

/**
 * Get the site URL dynamically
 * Priority: NEXT_PUBLIC_SITE_URL > VERCEL_PROJECT_PRODUCTION_URL > VERCEL_URL > siteConfig.url
 */
export function getSiteUrl(): string {
  if (process.env.NEXT_PUBLIC_SITE_URL) {
    return process.env.NEXT_PUBLIC_SITE_URL
  }
  if (process.env.VERCEL_PROJECT_PRODUCTION_URL) {
    return `https://${process.env.VERCEL_PROJECT_PRODUCTION_URL}`
  }
  if (process.env.VERCEL_URL) {
    return `https://${process.env.VERCEL_URL}`
  }
  return siteConfig.url
}

import type { Metadata } from 'next'
import { DM_Sans, JetBrains_Mono, Archivo, Source_Serif_4 } from 'next/font/google'
import { ThemeProvider } from './providers/theme-provider'
import { siteConfig } from '@/lib/theme-config'
import './globals.css'

const sans = DM_Sans({
  subsets: ['latin'],
  weight: ['400', '500', '600', '700'],
  variable: '--font-sans',
  display: 'swap',
})

const mono = JetBrains_Mono({
  subsets: ['latin'],
  weight: ['400', '500'],
  variable: '--font-mono',
  display: 'swap',
})

const display = Archivo({
  subsets: ['latin'],
  weight: ['600', '700', '900'],
  variable: '--font-display',
  display: 'swap',
})

const serif = Source_Serif_4({
  subsets: ['latin'],
  weight: ['400', '500'],
  variable: '--font-serif',
  display: 'swap',
})

export const metadata: Metadata = {
  title: {
    default: siteConfig.name,
    template: `%s | ${siteConfig.name}`,
  },
  description: siteConfig.description,
}

export default function RootLayout({
  children,
}: {
  children: React.ReactNode
}) {
  return (
    <html lang="en" className={`${sans.variable} ${mono.variable} ${display.variable} ${serif.variable}`} suppressHydrationWarning>
      <body suppressHydrationWarning>
        <ThemeProvider
          attribute="class"
          defaultTheme="dark"
          enableSystem
          disableTransitionOnChange
        >
          {children}
        </ThemeProvider>
      </body>
    </html>
  )
}

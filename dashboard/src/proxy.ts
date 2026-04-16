import { NextResponse } from 'next/server'
import type { NextRequest } from 'next/server'

export function proxy(request: NextRequest) {
  const host = request.headers.get('host') || ''
  const { pathname } = request.nextUrl

  // Skip Next.js internals, actual API routes, and common static files on all hosts —
  // the proxy should not rewrite these, they serve from canonical paths.
  // Add new /src/app/api/<route>/route.ts paths here explicitly.
  if (
    pathname.startsWith('/_next/') ||
    pathname === '/api/search' ||
    pathname.startsWith('/api/search/') ||
    pathname === '/favicon.ico' ||
    pathname === '/robots.txt' ||
    pathname === '/sitemap.xml'
  ) {
    return NextResponse.next()
  }

  // docs.solvela.ai → /docs/*
  if (host === 'docs.solvela.ai') {
    if (pathname === '/docs' || pathname.startsWith('/docs/')) {
      return NextResponse.next()
    }
    const url = request.nextUrl.clone()
    url.pathname = pathname === '/' ? '/docs' : `/docs${pathname}`
    return NextResponse.rewrite(url)
  }

  // app.solvela.ai → /dashboard/*
  if (host === 'app.solvela.ai') {
    if (pathname === '/dashboard' || pathname.startsWith('/dashboard/')) {
      return NextResponse.next()
    }
    const url = request.nextUrl.clone()
    url.pathname = pathname === '/' ? '/dashboard' : `/dashboard${pathname}`
    return NextResponse.rewrite(url)
  }

  // solvela.ai and all other hosts — pass through
  return NextResponse.next()
}

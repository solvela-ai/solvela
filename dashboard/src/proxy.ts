import { NextResponse } from 'next/server'
import type { NextRequest } from 'next/server'

export function proxy(request: NextRequest) {
  const host = request.headers.get('host') || ''
  const { pathname } = request.nextUrl

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

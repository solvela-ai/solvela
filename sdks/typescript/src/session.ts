import { createHash } from 'node:crypto';

import type { ChatMessage } from './types.js';

export interface SessionInfo {
  sessionId: string;
  model: string;
  requestCount: number;
  requestHashes: Set<string>;
  lastAccess: number;
}

interface InternalSession {
  model: string;
  requestCount: number;
  requestHashes: Set<string>;
  lastAccess: number;
}

export class SessionStore {
  private sessions = new Map<string, InternalSession>();

  constructor(private readonly ttl: number = 1800) {}

  getOrCreate(sessionId: string, defaultModel: string): SessionInfo {
    const now = Date.now() / 1000;
    let session = this.sessions.get(sessionId);

    if (!session) {
      session = {
        model: defaultModel,
        requestCount: 0,
        requestHashes: new Set(),
        lastAccess: now,
      };
      this.sessions.set(sessionId, session);
    } else {
      session.lastAccess = now;
    }

    return {
      sessionId,
      model: session.model,
      requestCount: session.requestCount,
      requestHashes: session.requestHashes,
      lastAccess: session.lastAccess,
    };
  }

  recordRequest(sessionId: string, requestHash: string): void {
    const session = this.sessions.get(sessionId);
    if (session) {
      session.requestCount += 1;
      session.requestHashes.add(requestHash);
      session.lastAccess = Date.now() / 1000;
    }
  }

  cleanupExpired(): void {
    const now = Date.now() / 1000;
    for (const [id, session] of this.sessions) {
      if (now - session.lastAccess > this.ttl) {
        this.sessions.delete(id);
      }
    }
  }

  static deriveSessionId(messages: ChatMessage[]): string {
    const raw = messages.map((m) => `${m.role}:${m.content ?? ''}`).join('|');
    return createHash('sha256').update(raw).digest('hex').substring(0, 16);
  }
}
